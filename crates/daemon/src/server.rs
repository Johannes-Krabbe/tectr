//! The websocket server. Each connection handles its own subscriptions.
//!
//! A session's events can reach a connection from two places: stored history
//! and the live broadcast. Tracking the last sequence number sent per session
//! makes that safe, because anything already sent is skipped. A client that
//! falls behind the broadcast is refilled from the database.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use protocol::{
    ClientMessage, CommandError, CommandOutput, MIN_PROTOCOL_VERSION, PROTOCOL_VERSION,
    ServerMessage, SessionId, negotiate_version,
};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinSet;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;

use crate::core::{Core, Signal};

const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const REPLAY_PAGE_SIZE: usize = 500;

type Socket = WebSocketStream<TcpStream>;

pub async fn accept_loop(
    core: Arc<Core>,
    listener: TcpListener,
    mut shutdown: watch::Receiver<bool>,
) {
    // Dropping the set when this returns closes every connection.
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    let core = core.clone();
                    connections.spawn(async move {
                        if let Err(error) = serve(core, stream).await {
                            tracing::debug!(%peer, "connection ended: {error:#}");
                        }
                    });
                }
                Err(error) => tracing::warn!("failed to accept a connection: {error}"),
            },
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
            _ = shutdown.changed() => return,
        }
    }
}

/// Browsers let any web page open a websocket to loopback, and always send an
/// `Origin` header when they do. Native clients don't send one. Without this
/// check, any site you visit could start agents on your machine.
#[allow(clippy::result_large_err)]
fn reject_browsers(request: &Request, response: Response) -> Result<Response, ErrorResponse> {
    if request.headers().contains_key("origin") {
        let mut rejection = ErrorResponse::new(Some("browsers can't connect".to_owned()));
        *rejection.status_mut() = StatusCode::FORBIDDEN;
        return Err(rejection);
    }
    Ok(response)
}

async fn serve(core: Arc<Core>, stream: TcpStream) -> Result<()> {
    let mut socket = tokio_tungstenite::accept_hdr_async(stream, reject_browsers)
        .await
        .context("websocket handshake failed")?;

    let hello = tokio::time::timeout(HELLO_TIMEOUT, next_message(&mut socket))
        .await
        .context("no hello in time")??;
    let Some(ClientMessage::Hello {
        min_version,
        max_version,
        client,
    }) = hello
    else {
        bail!("the first message wasn't a hello");
    };
    let Some(version) = negotiate_version(min_version, max_version) else {
        send(
            &mut socket,
            &ServerMessage::VersionRejected {
                min_version: MIN_PROTOCOL_VERSION,
                max_version: PROTOCOL_VERSION,
            },
        )
        .await?;
        socket.close(None).await?;
        return Ok(());
    };
    tracing::debug!(client, version, "client connected");
    send(
        &mut socket,
        &ServerMessage::Welcome {
            version,
            machine: core.machine(),
            daemon_version: env!("CARGO_PKG_VERSION").to_owned(),
        },
    )
    .await?;

    let (state, signals) = core.snapshot_and_subscribe();
    send(&mut socket, &ServerMessage::Snapshot { state }).await?;

    let mut connection = Connection {
        core,
        socket,
        signals,
        subscriptions: HashMap::new(),
    };
    let (responses, mut finished) =
        mpsc::unbounded_channel::<(u64, Result<CommandOutput, CommandError>)>();

    loop {
        tokio::select! {
            message = next_message(&mut connection.socket) => match message? {
                None => return Ok(()),
                Some(ClientMessage::Request { id, command }) => {
                    // Commands such as creating a worktree take time, and the
                    // event stream shouldn't stall while they run.
                    let core = connection.core.clone();
                    let responses = responses.clone();
                    tokio::spawn(async move {
                        let _ = responses.send((id, core.handle(command).await));
                    });
                }
                Some(ClientMessage::Subscribe { session_id, after_seq }) => {
                    connection.subscribe(session_id, after_seq).await?;
                }
                Some(ClientMessage::Unsubscribe { session_id }) => {
                    connection.subscriptions.remove(&session_id);
                }
                Some(ClientMessage::Hello { .. } | ClientMessage::Unknown) => {}
            },
            signal = connection.signals.recv() => match signal {
                Ok(signal) => connection.forward(signal).await?,
                Err(broadcast::error::RecvError::Lagged(_)) => connection.resynchronize().await?,
                Err(broadcast::error::RecvError::Closed) => return Ok(()),
            },
            Some((id, result)) = finished.recv() => {
                connection.send_response(id, result).await?;
            }
        }
    }
}

struct Connection {
    core: Arc<Core>,
    socket: Socket,
    signals: broadcast::Receiver<Signal>,
    /// For each followed session, the last sequence number sent.
    subscriptions: HashMap<SessionId, u64>,
}

impl Connection {
    async fn subscribe(&mut self, session_id: SessionId, after_seq: u64) -> Result<()> {
        self.subscriptions.insert(session_id.clone(), after_seq);
        self.catch_up(&session_id).await
    }

    async fn forward(&mut self, signal: Signal) -> Result<()> {
        match signal {
            Signal::State(change) => {
                let change = (*change).clone();
                send(&mut self.socket, &ServerMessage::StateChanged { change }).await
            }
            Signal::Event(event) => {
                let id = &event.session.session_id;
                let is_new = self
                    .subscriptions
                    .get(id)
                    .is_some_and(|last_sent| event.seq > *last_sent);
                if is_new {
                    let message = ServerMessage::SessionEvent {
                        event: (*event).clone(),
                    };
                    send(&mut self.socket, &message).await?;
                    self.subscriptions.insert(id.clone(), event.seq);
                }
                Ok(())
            }
        }
    }

    /// This client missed live updates, so it gets a fresh snapshot, and its
    /// sessions are refilled from the database.
    async fn resynchronize(&mut self) -> Result<()> {
        tracing::debug!("a client fell behind, resynchronizing");
        let (state, signals) = self.core.snapshot_and_subscribe();
        self.signals = signals;
        send(&mut self.socket, &ServerMessage::Snapshot { state }).await?;
        let sessions: Vec<SessionId> = self.subscriptions.keys().cloned().collect();
        for id in &sessions {
            self.catch_up(id).await?;
        }
        Ok(())
    }

    /// Sends the response only after everything the command caused. The command
    /// has finished, so all of that is already queued here. A client can then
    /// rely on its state being current as soon as a command returns.
    async fn send_response(
        &mut self,
        id: u64,
        result: Result<CommandOutput, CommandError>,
    ) -> Result<()> {
        loop {
            match self.signals.try_recv() {
                Ok(signal) => self.forward(signal).await?,
                Err(broadcast::error::TryRecvError::Lagged(_)) => self.resynchronize().await?,
                Err(
                    broadcast::error::TryRecvError::Empty | broadcast::error::TryRecvError::Closed,
                ) => break,
            }
        }
        send(&mut self.socket, &ServerMessage::Response { id, result }).await
    }

    /// Sends everything stored after the last event sent, a page at a time.
    async fn catch_up(&mut self, session_id: &SessionId) -> Result<()> {
        let Some(mut last_sent) = self.subscriptions.get(session_id).copied() else {
            return Ok(());
        };
        loop {
            let page = self
                .core
                .events_after(session_id, last_sent, REPLAY_PAGE_SIZE)?;
            if page.is_empty() {
                return Ok(());
            }
            for event in page {
                last_sent = event.seq;
                send(&mut self.socket, &ServerMessage::SessionEvent { event }).await?;
                self.subscriptions.insert(session_id.clone(), last_sent);
            }
        }
    }
}

/// Returns the next protocol message, or `None` once the client has gone.
async fn next_message(socket: &mut Socket) -> Result<Option<ClientMessage>> {
    while let Some(frame) = socket.next().await {
        match frame? {
            Message::Text(text) => {
                return Ok(Some(
                    serde_json::from_str(&text).context("invalid client message")?,
                ));
            }
            Message::Close(_) => return Ok(None),
            _ => {}
        }
    }
    Ok(None)
}

async fn send(socket: &mut Socket, message: &ServerMessage) -> Result<()> {
    let text = serde_json::to_string(message)?;
    socket.send(Message::text(text)).await?;
    Ok(())
}
