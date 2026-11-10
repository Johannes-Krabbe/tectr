//! The websocket server. Each connection handles its own subscriptions. Loopback
//! connections arrive over TCP, and paired remote clients over a QUIC stream from
//! `crate::remote`, with the same protocol on both.
//!
//! A session's events can reach a connection from two places: stored history
//! and the live broadcast. Tracking the last sequence number sent per session
//! makes that safe, because anything already sent is skipped. A client that
//! falls behind the broadcast is refilled from the database.
//!
//! Terminals send their frames to the connections showing them, which pass them on as
//! binary messages. Keeping up is up to each terminal, through what clients acknowledge.
//!
//! A client pings every couple of seconds. One that goes quiet for longer than
//! `Config::client_silence` has gone or has given up on this daemon, and so has one that
//! was kept waiting while this machine slept. Either way the connection ends before
//! anything more is read from it, so what a client sent and stopped waiting for never
//! runs later. See [`protocol::liveness`].

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::{FutureExt, SinkExt, StreamExt};
use protocol::frame::{self, Kind};
use protocol::liveness::SleepWatch;
use protocol::{
    ClientMessage, Command, CommandError, CommandOutput, ErrorCode, MIN_PROTOCOL_VERSION,
    PROTOCOL_VERSION, ServerMessage, SessionId, StateChange, TerminalId, negotiate_version,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinSet;
use tokio::time::Instant;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;

use crate::core::{Core, Signal};
use crate::terminal::Subscriber;

const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const REPLAY_PAGE_SIZE: usize = 500;

/// A byte stream a websocket can run over: TCP, or a QUIC stream.
pub(crate) trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

type Socket = WebSocketStream<Box<dyn Io>>;

/// Who is on the other end of a connection.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Peer {
    /// A client on this machine.
    Loopback,
    /// A paired client on another machine, by its public key.
    Remote { key: String },
}

impl Peer {
    /// The client went off the allowed list, so this connection has to end.
    fn removed_by(&self, change: &StateChange) -> bool {
        match (self, change) {
            (Peer::Remote { key }, StateChange::PairedClientRemoved { key: removed }) => {
                key == removed
            }
            _ => false,
        }
    }
}

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
    if crate::http::peek_path(&stream, HELLO_TIMEOUT)
        .await
        .is_some()
    {
        return crate::http::serve(core, stream).await;
    }
    // Each keystroke's echo is a small write, which shouldn't wait to be combined with others.
    let _ = stream.set_nodelay(true);
    let stream: Box<dyn Io> = Box::new(stream);
    let socket = tokio_tungstenite::accept_hdr_async(stream, reject_browsers)
        .await
        .context("websocket handshake failed")?;
    serve_socket(core, socket, Peer::Loopback).await
}

/// Serves a paired client on another machine. `crate::remote` has checked its key.
pub(crate) async fn serve_remote(
    core: Arc<Core>,
    stream: impl Io + 'static,
    key: String,
) -> Result<()> {
    let stream: Box<dyn Io> = Box::new(stream);
    let socket = tokio::time::timeout(
        HELLO_TIMEOUT,
        tokio_tungstenite::accept_hdr_async(stream, reject_browsers),
    )
    .await
    .context("no websocket handshake in time")?
    .context("websocket handshake failed")?;
    serve_socket(core, socket, Peer::Remote { key }).await
}

async fn serve_socket(core: Arc<Core>, mut socket: Socket, peer: Peer) -> Result<()> {
    let hello = tokio::time::timeout(HELLO_TIMEOUT, next_message(&mut socket))
        .await
        .context("no hello in time")??;
    let Some(Incoming::Message(ClientMessage::Hello {
        min_version,
        max_version,
        client,
    })) = hello
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
    // The key was checked before the handshake, and could have been removed since.
    if let Peer::Remote { key } = &peer
        && !state.paired_clients.iter().any(|client| &client.key == key)
    {
        bail!("no longer paired");
    }
    send(&mut socket, &ServerMessage::Snapshot { state }).await?;

    let (terminal_frames, mut frames) = mpsc::unbounded_channel();
    let (responses, mut finished) = mpsc::unbounded_channel();
    let silence = core.client_silence();
    let mut connection = Connection {
        core,
        socket,
        signals,
        subscriptions: HashMap::new(),
        terminals: HashMap::new(),
        terminal_frames,
        responses,
        peer,
        sleep: SleepWatch::start(),
    };

    let mut heard = Instant::now();
    loop {
        tokio::select! {
            message = next_message(&mut connection.socket) => {
                let Some(incoming) = message? else { return Ok(()) };
                heard = Instant::now();
                connection.incoming(incoming, silence).await?;
            }
            Some(frame) = frames.recv() => {
                connection.socket.send(Message::binary(frame)).await?;
            }
            signal = connection.signals.recv() => match signal {
                Ok(signal) => connection.forward(signal).await?,
                Err(broadcast::error::RecvError::Lagged(_)) => connection.resynchronize().await?,
                Err(broadcast::error::RecvError::Closed) => return Ok(()),
            },
            Some((id, result)) = finished.recv() => {
                connection.send_response(id, result).await?;
            }
            () = tokio::time::sleep_until(heard + silence) => {
                // A long send can keep this loop from reading for longer than the limit,
                // so what's already here gets one look before the client counts as gone.
                match next_message(&mut connection.socket).now_or_never() {
                    Some(message) => {
                        let Some(incoming) = message? else { return Ok(()) };
                        heard = Instant::now();
                        connection.incoming(incoming, silence).await?;
                    }
                    None => bail!("the client went quiet"),
                }
            }
        }
    }
}

type Finished = (u64, Result<CommandOutput, CommandError>);

struct Connection {
    core: Arc<Core>,
    socket: Socket,
    signals: broadcast::Receiver<Signal>,
    /// For each followed session, the last sequence number sent.
    subscriptions: HashMap<SessionId, u64>,
    /// The terminals this client shows, with what each knows it as.
    terminals: HashMap<TerminalId, u64>,
    terminal_frames: mpsc::UnboundedSender<Vec<u8>>,
    /// Where commands put their results once they finish.
    responses: mpsc::UnboundedSender<Finished>,
    peer: Peer,
    sleep: SleepWatch,
}

impl Drop for Connection {
    fn drop(&mut self) {
        for (id, subscriber) in self.terminals.drain() {
            self.core.detach_terminal(&id, subscriber);
        }
    }
}

impl Connection {
    /// Handles one thing the client sent.
    async fn incoming(&mut self, incoming: Incoming, silence: Duration) -> Result<()> {
        // Timers stood still while this machine slept, but a client on another machine
        // counted the silence and gave up on this connection. What it sent before that,
        // this included, is only arriving now and mustn't run. A client on this machine
        // slept too, and is still waiting.
        if self.peer != Peer::Loopback && self.sleep.slept() >= silence {
            bail!("this machine slept for longer than the client waits");
        }
        match incoming {
            Incoming::Alive
            | Incoming::Message(ClientMessage::Hello { .. } | ClientMessage::Unknown) => {}
            Incoming::Frame(data) => self.frame(&data),
            Incoming::Message(ClientMessage::Request { id, command })
                if self.peer != Peer::Loopback
                    && matches!(command, Command::StartPairing | Command::CancelPairing) =>
            {
                let refusal = CommandError {
                    code: ErrorCode::InvalidInput,
                    message: "pairing only starts on the device itself".to_owned(),
                };
                self.send_response(id, Err(refusal)).await?;
            }
            Incoming::Message(ClientMessage::Request { id, command }) => {
                // Commands such as creating a worktree take time, and the
                // event stream shouldn't stall while they run.
                let core = self.core.clone();
                let responses = self.responses.clone();
                tokio::spawn(async move {
                    let _ = responses.send((id, core.handle(command).await));
                });
            }
            Incoming::Message(ClientMessage::Subscribe {
                session_id,
                after_seq,
            }) => {
                self.subscribe(session_id, after_seq).await?;
            }
            Incoming::Message(ClientMessage::Unsubscribe { session_id }) => {
                self.subscriptions.remove(&session_id);
            }
            Incoming::Message(ClientMessage::AttachTerminal {
                terminal_id,
                cols,
                rows,
                colors,
            }) => self.attach_terminal(terminal_id, cols, rows, colors),
            Incoming::Message(ClientMessage::DetachTerminal { terminal_id }) => {
                self.detach_terminal(&terminal_id);
            }
            Incoming::Message(ClientMessage::ResizeTerminal {
                terminal_id,
                cols,
                rows,
            }) => {
                self.core.resize_terminal(&terminal_id, cols, rows);
            }
        }
        Ok(())
    }

    fn attach_terminal(
        &mut self,
        id: TerminalId,
        cols: u16,
        rows: u16,
        colors: Option<protocol::TerminalColors>,
    ) {
        self.detach_terminal(&id);
        let subscriber = Subscriber {
            id: self.core.next_subscriber(),
            frames: self.terminal_frames.clone(),
        };
        let subscriber_id = subscriber.id;
        match self
            .core
            .attach_terminal(&id, subscriber, (cols, rows), colors)
        {
            Ok(()) => {
                self.terminals.insert(id, subscriber_id);
            }
            Err(error) => tracing::debug!(%id, "couldn't show a terminal: {error:#}"),
        }
    }

    fn detach_terminal(&mut self, id: &TerminalId) {
        if let Some(subscriber) = self.terminals.remove(id) {
            self.core.detach_terminal(id, subscriber);
        }
    }

    /// Typing and acknowledgements. Frames for a terminal this client doesn't show are
    /// dropped, so nobody types into a terminal they can't see.
    fn frame(&mut self, data: &[u8]) {
        let Some(frame) = frame::decode(data) else {
            return;
        };
        let id = TerminalId::from(frame.terminal_id);
        let Some(&subscriber) = self.terminals.get(&id) else {
            return;
        };
        match frame.kind {
            Kind::Input => self.core.terminal_input(&id, frame.payload.to_vec()),
            Kind::Ack => {
                if let Some(bytes) = frame::read_ack(frame.payload) {
                    self.core.terminal_ack(&id, subscriber, bytes);
                }
            }
            Kind::Output | Kind::Reset => {}
        }
    }

    async fn subscribe(&mut self, session_id: SessionId, after_seq: u64) -> Result<()> {
        self.subscriptions.insert(session_id.clone(), after_seq);
        self.catch_up(&session_id).await
    }

    async fn forward(&mut self, signal: Signal) -> Result<()> {
        match signal {
            Signal::State(change) => {
                if self.peer.removed_by(&change) {
                    bail!("no longer paired");
                }
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
        // A removal could be among the changes this client missed.
        if let Peer::Remote { key } = &self.peer
            && !state.paired_clients.iter().any(|client| &client.key == key)
        {
            bail!("no longer paired");
        }
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

enum Incoming {
    Message(ClientMessage),
    /// A binary terminal frame.
    Frame(Vec<u8>),
    /// A ping, which the websocket answers by itself: nothing but a sign the client is there.
    Alive,
}

/// Returns the next thing the client sent, or `None` once it has gone.
async fn next_message(socket: &mut Socket) -> Result<Option<Incoming>> {
    let Some(frame) = socket.next().await else {
        return Ok(None);
    };
    Ok(match frame? {
        Message::Text(text) => Some(Incoming::Message(
            serde_json::from_str(&text).context("invalid client message")?,
        )),
        Message::Binary(data) => Some(Incoming::Frame(data.to_vec())),
        Message::Close(_) => None,
        _ => Some(Incoming::Alive),
    })
}

async fn send(socket: &mut Socket, message: &ServerMessage) -> Result<()> {
    let text = serde_json::to_string(message)?;
    socket.send(Message::text(text)).await?;
    Ok(())
}
