//! The engine shared by every tectr app. The UI sends it commands and reads
//! state from it, and never touches a socket.
//!
//! The engine is built around a list of daemons. It keeps one connection to each
//! with automatic reconnects, merges what they report, and resumes each open
//! session's events from the last sequence number it saw.
//!
//! A daemon's URL says how to reach it: `ws://` for one on this machine, and
//! `iroh://<key>` for a paired one on another machine. Both carry the same protocol.
//!
//! Not here yet: the disk cache of last known state, per-frame batching of
//! updates (the desktop app does its own for now), and the `suspend()`/`resume()`
//! calls for iOS.

mod client_store;

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub use client_store::{ClientStore, this_machine_name};
use futures_util::{SinkExt, StreamExt};
use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointId, SecretKey};
use protocol::{
    Approval, ClientMessage, Command, CommandError, CommandOutput, DaemonState,
    MIN_PROTOCOL_VERSION, Machine, MachineId, PAIRING_ALPN, PROTOCOL_VERSION, PairRequest,
    PairResponse, PairingCode, REMOTE_ALPN, ServerMessage, SessionEvent, SessionRef,
};
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

const MIN_BACKOFF: Duration = Duration::from_millis(100);
const MAX_BACKOFF: Duration = Duration::from_secs(10);
const UPDATE_BUFFER: usize = 4096;
/// Finding a remote daemon can take a lookup and a relay before the first byte.
const PAIR_TIMEOUT: Duration = Duration::from_secs(30);
/// A pairing answer is a machine's ID and name, so anything longer isn't one.
const MAX_PAIR_RESPONSE: usize = 64 * 1024;

/// What the engine knows about one daemon. The state stays after a disconnect,
/// so a UI can still show it along with when the machine was last seen.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Device {
    pub url: String,
    pub connection: Connection,
    pub state: Option<DaemonState>,
    /// Milliseconds since the Unix epoch.
    pub last_seen_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Connection {
    /// The first attempt hasn't finished yet.
    Connecting,
    Online,
    Offline {
        reason: String,
    },
    /// The daemon and this client have no protocol version in common.
    Incompatible {
        daemon_min: u32,
        daemon_max: u32,
    },
}

/// Tells a UI what to redraw. Read the new values with [`Engine::devices`] and
/// [`Engine::events_after`].
#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    DeviceChanged { url: String },
    SessionEvent { session: SessionRef, seq: u64 },
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("machine {0} is offline")]
    Offline(MachineId),
    /// The connection dropped before the answer came. The command may or may
    /// not have run, so the engine doesn't retry it.
    #[error("the connection dropped before the daemon answered")]
    Disconnected,
    #[error("{}", .0.message)]
    Command(CommandError),
    #[error("{0}")]
    Pairing(String),
}

/// What this client needs to reach daemons on other machines.
pub struct Remote {
    /// What this client proves to a daemon, and what the daemon's allowed list holds.
    pub key: SecretKey,
    pub network: Network,
}

/// How the engine reaches the network for remote daemons.
#[derive(Debug, Clone, Default)]
pub enum Network {
    /// n0's public relays and address lookup, so a daemon is found by its key alone.
    #[default]
    Public,
    /// Loopback only, with no relay, for tests. Daemons are found only at the addresses
    /// added to the lookup.
    Local(iroh::address_lookup::MemoryLookup),
}

struct RemoteSetup {
    key: SecretKey,
    network: Network,
    /// Bound on first use, so a client with only local daemons never touches the network.
    endpoint: tokio::sync::OnceCell<Endpoint>,
}

impl RemoteSetup {
    async fn endpoint(&self) -> Result<&Endpoint, String> {
        self.endpoint
            .get_or_try_init(|| async {
                let builder = match &self.network {
                    Network::Public => Endpoint::builder(presets::N0),
                    Network::Local(lookup) => Endpoint::builder(presets::Minimal)
                        .clear_ip_transports()
                        .bind_addr("127.0.0.1:0")
                        .map_err(|error| error.to_string())?
                        .address_lookup(lookup.clone()),
                };
                builder
                    .secret_key(self.key.clone())
                    .bind()
                    .await
                    .map_err(|error| format!("couldn't start remote connections: {error}"))
            })
            .await
    }
}

/// A byte stream a websocket can run over: TCP, or a QUIC stream.
trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

struct DaemonEntry {
    device: Device,
    outgoing: Option<mpsc::UnboundedSender<ClientMessage>>,
    waiting: HashMap<u64, oneshot::Sender<Result<CommandOutput, CommandError>>>,
}

struct Shared {
    daemons: BTreeMap<String, DaemonEntry>,
    /// Sessions the UI is following, with all their events so far.
    open: HashMap<SessionRef, Vec<SessionEvent>>,
}

struct Inner {
    shared: Mutex<Shared>,
    updates: broadcast::Sender<Update>,
    next_request: AtomicU64,
    client_name: String,
    remote: Option<RemoteSetup>,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.shared
            .lock()
            .expect("a panic poisoned the engine state")
    }

    fn device_changed(&self, url: &str) {
        let _ = self.updates.send(Update::DeviceChanged {
            url: url.to_owned(),
        });
    }
}

/// Must be created inside a Tokio runtime. Dropping it closes all connections.
pub struct Engine {
    inner: Arc<Inner>,
    /// Each daemon's connection task, by URL.
    tasks: Mutex<HashMap<String, JoinHandle<()>>>,
}

impl Drop for Engine {
    fn drop(&mut self) {
        for (_, task) in self
            .tasks
            .lock()
            .expect("a panic poisoned the task list")
            .drain()
        {
            task.abort();
        }
    }
}

impl Engine {
    /// An engine for daemons on this machine only.
    pub fn new(client_name: impl Into<String>) -> Self {
        Self::build(client_name.into(), None)
    }

    /// An engine that can also pair with and reach daemons on other machines.
    pub fn with_remote(client_name: impl Into<String>, remote: Remote) -> Self {
        let setup = RemoteSetup {
            key: remote.key,
            network: remote.network,
            endpoint: tokio::sync::OnceCell::new(),
        };
        Self::build(client_name.into(), Some(setup))
    }

    fn build(client_name: String, remote: Option<RemoteSetup>) -> Self {
        let (updates, _) = broadcast::channel(UPDATE_BUFFER);
        Self {
            inner: Arc::new(Inner {
                shared: Mutex::new(Shared {
                    daemons: BTreeMap::new(),
                    open: HashMap::new(),
                }),
                updates,
                next_request: AtomicU64::new(1),
                client_name,
                remote,
            }),
            tasks: Mutex::new(HashMap::new()),
        }
    }

    /// Adds a daemon and starts connecting to it. Adding one twice does nothing.
    pub fn add_daemon(&self, url: &str) {
        let mut shared = self.inner.lock();
        if shared.daemons.contains_key(url) {
            return;
        }
        shared.daemons.insert(
            url.to_owned(),
            DaemonEntry {
                device: Device {
                    url: url.to_owned(),
                    connection: Connection::Connecting,
                    state: None,
                    last_seen_ms: None,
                },
                outgoing: None,
                waiting: HashMap::new(),
            },
        );
        drop(shared);
        let task = tokio::spawn(maintain_connection(self.inner.clone(), url.to_owned()));
        self.tasks
            .lock()
            .expect("a panic poisoned the task list")
            .insert(url.to_owned(), task);
    }

    /// Disconnects from a daemon and forgets everything it reported.
    pub fn remove_daemon(&self, url: &str) {
        if let Some(task) = self
            .tasks
            .lock()
            .expect("a panic poisoned the task list")
            .remove(url)
        {
            task.abort();
        }
        // Dropping its waiting requests fails them with `Disconnected`.
        self.inner.lock().daemons.remove(url);
        self.inner.device_changed(url);
    }

    /// Pairs with a daemon on another machine using the code it showed, then adds it.
    /// `name` is what the daemon lists this client as. Returns the daemon's URL.
    pub async fn pair(&self, code: &str, name: &str) -> Result<String, EngineError> {
        let pairing = |message: &str| EngineError::Pairing(message.to_owned());
        let code = PairingCode::parse(code).ok_or_else(|| pairing("that isn't a pairing code"))?;
        let daemon: EndpointId = code
            .daemon_key
            .parse()
            .map_err(|_| pairing("that pairing code is damaged"))?;
        let remote = self
            .inner
            .remote
            .as_ref()
            .ok_or_else(|| pairing("this client can't reach other machines"))?;
        let endpoint = remote.endpoint().await.map_err(EngineError::Pairing)?;
        let request = PairRequest {
            secret: code.secret,
            name: name.to_owned(),
        };
        let exchange = async {
            let connection = endpoint
                .connect(daemon, PAIRING_ALPN)
                .await
                .map_err(|error| format!("couldn't reach the device: {error}"))?;
            let (mut send, mut recv) = connection.open_bi().await.map_err(describe_closed)?;
            let request = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
            send.write_all(&request).await.map_err(describe_write)?;
            send.finish().map_err(|error| error.to_string())?;
            let answer = recv
                .read_to_end(MAX_PAIR_RESPONSE)
                .await
                .map_err(describe_read)?;
            connection.close(0u32.into(), b"");
            serde_json::from_slice::<PairResponse>(&answer)
                .map_err(|_| "the device sent something odd back".to_owned())
        };
        let response = tokio::time::timeout(PAIR_TIMEOUT, exchange)
            .await
            .map_err(|_| pairing("the device didn't answer in time"))?
            .map_err(EngineError::Pairing)?;
        match response {
            PairResponse::Paired { .. } => {
                let url = protocol::remote_daemon_url(&daemon.to_string());
                self.add_daemon(&url);
                Ok(url)
            }
            PairResponse::Refused { reason } => Err(EngineError::Pairing(reason)),
            PairResponse::Unknown => Err(pairing(
                "the device answered in a way this client doesn't know",
            )),
        }
    }

    /// All daemons, each with its part of the device and workspace tree.
    pub fn devices(&self) -> Vec<Device> {
        self.inner
            .lock()
            .daemons
            .values()
            .map(|d| d.device.clone())
            .collect()
    }

    /// Every pending request across all daemons.
    pub fn approvals(&self) -> Vec<(MachineId, Approval)> {
        self.inner
            .lock()
            .daemons
            .values()
            .filter_map(|daemon| daemon.device.state.as_ref())
            .flat_map(|state| {
                state
                    .approvals
                    .iter()
                    .map(|approval| (state.machine.id.clone(), approval.clone()))
            })
            .collect()
    }

    /// Subscribe before reading state, so that no change falls between the two.
    /// A receiver that lags has only missed wake-ups: the state is still here.
    pub fn updates(&self) -> broadcast::Receiver<Update> {
        self.inner.updates.subscribe()
    }

    /// Waits until every daemon has either connected or failed its first attempt.
    pub async fn wait_until_settled(&self) {
        let mut updates = self.updates();
        loop {
            let settled = self
                .inner
                .lock()
                .daemons
                .values()
                .all(|daemon| daemon.device.connection != Connection::Connecting);
            if settled {
                return;
            }
            if let Err(broadcast::error::RecvError::Closed) = updates.recv().await {
                return;
            }
        }
    }

    /// Runs a command on a machine. It fails right away if the machine is offline.
    pub async fn request(
        &self,
        machine_id: &MachineId,
        command: Command,
    ) -> Result<CommandOutput, EngineError> {
        let id = self.inner.next_request.fetch_add(1, Ordering::Relaxed);
        let answer = {
            let mut shared = self.inner.lock();
            let daemon = shared
                .daemons
                .values_mut()
                .find(|daemon| {
                    daemon.outgoing.is_some()
                        && daemon.device.state.as_ref().map(|state| &state.machine.id)
                            == Some(machine_id)
                })
                .ok_or_else(|| EngineError::Offline(machine_id.clone()))?;
            let (answer_tx, answer_rx) = oneshot::channel();
            daemon.waiting.insert(id, answer_tx);
            if let Some(outgoing) = &daemon.outgoing {
                let _ = outgoing.send(ClientMessage::Request { id, command });
            }
            answer_rx
        };
        answer
            .await
            .map_err(|_| EngineError::Disconnected)?
            .map_err(EngineError::Command)
    }

    /// Starts following a session. Its events stay available, and survive
    /// reconnects, until [`Engine::close_session`].
    pub fn open_session(&self, session: &SessionRef) {
        let mut shared = self.inner.lock();
        if shared.open.contains_key(session) {
            return;
        }
        shared.open.insert(session.clone(), vec![]);
        for daemon in shared.daemons.values() {
            let on_this_daemon = daemon.device.state.as_ref().map(|state| &state.machine.id)
                == Some(&session.machine_id);
            if let (true, Some(outgoing)) = (on_this_daemon, &daemon.outgoing) {
                let _ = outgoing.send(ClientMessage::Subscribe {
                    session_id: session.session_id.clone(),
                    after_seq: 0,
                });
            }
        }
    }

    pub fn close_session(&self, session: &SessionRef) {
        let mut shared = self.inner.lock();
        shared.open.remove(session);
        for daemon in shared.daemons.values() {
            let on_this_daemon = daemon.device.state.as_ref().map(|state| &state.machine.id)
                == Some(&session.machine_id);
            if let (true, Some(outgoing)) = (on_this_daemon, &daemon.outgoing) {
                let _ = outgoing.send(ClientMessage::Unsubscribe {
                    session_id: session.session_id.clone(),
                });
            }
        }
    }

    /// The open session's events after `after_seq`, in order.
    pub fn events_after(&self, session: &SessionRef, after_seq: u64) -> Vec<SessionEvent> {
        let shared = self.inner.lock();
        let Some(events) = shared.open.get(session) else {
            return vec![];
        };
        let start = events.partition_point(|event| event.seq <= after_seq);
        events[start..].to_vec()
    }
}

/// Keeps one daemon connected for as long as the engine lives.
async fn maintain_connection(inner: Arc<Inner>, url: String) {
    let mut backoff = MIN_BACKOFF;
    loop {
        let (outcome, was_online) = run_connection(&inner, &url).await;
        if was_online {
            backoff = MIN_BACKOFF;
        }
        {
            let mut shared = inner.lock();
            let Some(daemon) = shared.daemons.get_mut(&url) else {
                return;
            };
            daemon.outgoing = None;
            // Dropping these fails each waiting request with `Disconnected`.
            daemon.waiting.clear();
            if was_online {
                daemon.device.last_seen_ms = Some(now_ms());
            }
            daemon.device.connection = outcome;
        }
        inner.device_changed(&url);
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

type Socket = WebSocketStream<Box<dyn Io>>;

/// Opens the websocket the URL describes. The iroh connection comes back too, because
/// dropping it would close the stream.
async fn open(
    inner: &Inner,
    url: &str,
) -> Result<(Socket, Option<iroh::endpoint::Connection>), String> {
    if let Some(key) = protocol::remote_daemon_key(url) {
        let daemon: EndpointId = key
            .parse()
            .map_err(|_| format!("{url} has a damaged key"))?;
        let remote = inner
            .remote
            .as_ref()
            .ok_or("this client can't reach other machines")?;
        let endpoint = remote.endpoint().await?;
        let connection = endpoint
            .connect(daemon, REMOTE_ALPN)
            .await
            .map_err(|error| format!("couldn't reach the device: {error}"))?;
        let (send, recv) = connection.open_bi().await.map_err(describe_closed)?;
        let stream: Box<dyn Io> = Box::new(tokio::io::join(recv, send));
        // The stream is already the daemon's, so the address only fills in the handshake.
        let (socket, _) = tokio_tungstenite::client_async("ws://tectr", stream)
            .await
            .map_err(|error| {
                connection
                    .close_reason()
                    .map_or_else(|| error.to_string(), describe_closed)
            })?;
        return Ok((socket, Some(connection)));
    }
    let request = url
        .into_client_request()
        .map_err(|error| error.to_string())?;
    let host = request
        .uri()
        .host()
        .ok_or_else(|| format!("{url} has no host"))?
        .to_owned();
    let port = request.uri().port_u16().unwrap_or(80);
    let tcp = TcpStream::connect((host.as_str(), port))
        .await
        .map_err(|error| error.to_string())?;
    // Streaming text goes out a token at a time, which shouldn't wait to fill a packet.
    let _ = tcp.set_nodelay(true);
    let stream: Box<dyn Io> = Box::new(tcp);
    let (socket, _) = tokio_tungstenite::client_async(request, stream)
        .await
        .map_err(|error| error.to_string())?;
    Ok((socket, None))
}

/// A daemon that refuses a client closes the connection with a reason, which says more
/// than the error around it.
fn describe_closed(error: iroh::endpoint::ConnectionError) -> String {
    match error {
        iroh::endpoint::ConnectionError::ApplicationClosed(close) => {
            let reason = String::from_utf8_lossy(&close.reason).into_owned();
            if reason.is_empty() {
                "the device closed the connection".to_owned()
            } else {
                reason
            }
        }
        other => other.to_string(),
    }
}

fn describe_read(error: iroh::endpoint::ReadToEndError) -> String {
    use iroh::endpoint::{ReadError, ReadToEndError};
    match error {
        ReadToEndError::Read(ReadError::ConnectionLost(error)) => describe_closed(error),
        other => other.to_string(),
    }
}

fn describe_write(error: iroh::endpoint::WriteError) -> String {
    match error {
        iroh::endpoint::WriteError::ConnectionLost(error) => describe_closed(error),
        other => other.to_string(),
    }
}

/// Runs one connection until it ends. Returns the resulting connection state
/// and whether this attempt ever got online.
async fn run_connection(inner: &Inner, url: &str) -> (Connection, bool) {
    let mut online = false;
    let result = connect_and_run(inner, url, &mut online).await;
    let state = match result {
        Ok(state) => state,
        Err(reason) => Connection::Offline { reason },
    };
    (state, online)
}

async fn connect_and_run(
    inner: &Inner,
    url: &str,
    online: &mut bool,
) -> Result<Connection, String> {
    let (mut socket, _connection) = open(inner, url).await?;
    send(
        &mut socket,
        &ClientMessage::Hello {
            min_version: MIN_PROTOCOL_VERSION,
            max_version: PROTOCOL_VERSION,
            client: inner.client_name.clone(),
        },
    )
    .await?;

    match next_message(&mut socket).await? {
        ServerMessage::Welcome { version, .. }
            if (MIN_PROTOCOL_VERSION..=PROTOCOL_VERSION).contains(&version) => {}
        ServerMessage::Welcome { version, .. } => {
            return Ok(Connection::Incompatible {
                daemon_min: version,
                daemon_max: version,
            });
        }
        ServerMessage::VersionRejected {
            min_version,
            max_version,
        } => {
            return Ok(Connection::Incompatible {
                daemon_min: min_version,
                daemon_max: max_version,
            });
        }
        other => return Err(format!("expected a welcome, got {other:?}")),
    }

    let (outgoing, mut to_send) = mpsc::unbounded_channel();
    loop {
        tokio::select! {
            message = next_message(&mut socket) => {
                let message = message?;
                handle_message(inner, url, message, &outgoing, online);
            }
            Some(message) = to_send.recv() => send(&mut socket, &message).await?,
        }
    }
}

fn handle_message(
    inner: &Inner,
    url: &str,
    message: ServerMessage,
    outgoing: &mpsc::UnboundedSender<ClientMessage>,
    online: &mut bool,
) {
    match message {
        ServerMessage::Snapshot { state } => {
            {
                let mut shared = inner.lock();
                let Shared { daemons, open } = &mut *shared;
                let Some(daemon) = daemons.get_mut(url) else {
                    return;
                };
                // The daemon also sends a snapshot when this client falls
                // behind. Only the first one on a connection means "online".
                if !*online {
                    resubscribe(open, &state.machine, outgoing);
                    daemon.outgoing = Some(outgoing.clone());
                    daemon.device.connection = Connection::Online;
                    *online = true;
                }
                daemon.device.state = Some(state);
            }
            inner.device_changed(url);
        }
        ServerMessage::StateChanged { change } => {
            {
                let mut shared = inner.lock();
                let Some(daemon) = shared.daemons.get_mut(url) else {
                    return;
                };
                if let Some(state) = &mut daemon.device.state {
                    state.apply(&change);
                }
            }
            inner.device_changed(url);
        }
        ServerMessage::SessionEvent { event } => {
            let mut shared = inner.lock();
            let Some(events) = shared.open.get_mut(&event.session) else {
                return; // Closed while this event was on its way.
            };
            // Only the next event in order counts. Anything else is a stale
            // copy, such as one still on its way when a session was closed and
            // quickly reopened. Keeping it would make the replay look old.
            let next_seq = events.last().map_or(1, |last| last.seq + 1);
            if event.seq != next_seq {
                tracing::debug!(seq = event.seq, next_seq, "ignoring an out-of-order event");
                return;
            }
            let update = Update::SessionEvent {
                session: event.session.clone(),
                seq: event.seq,
            };
            events.push(event);
            drop(shared);
            let _ = inner.updates.send(update);
        }
        ServerMessage::Response { id, result } => {
            let mut shared = inner.lock();
            let Some(daemon) = shared.daemons.get_mut(url) else {
                return;
            };
            if let Some(waiter) = daemon.waiting.remove(&id) {
                let _ = waiter.send(result);
            }
        }
        ServerMessage::Welcome { .. }
        | ServerMessage::VersionRejected { .. }
        | ServerMessage::Unknown => {}
    }
}

/// After connecting, picks up every open session on this machine where it left off.
fn resubscribe(
    open: &HashMap<SessionRef, Vec<SessionEvent>>,
    machine: &Machine,
    outgoing: &mpsc::UnboundedSender<ClientMessage>,
) {
    for (session, events) in open {
        if session.machine_id == machine.id {
            let _ = outgoing.send(ClientMessage::Subscribe {
                session_id: session.session_id.clone(),
                after_seq: events.last().map_or(0, |event| event.seq),
            });
        }
    }
}

async fn next_message(socket: &mut Socket) -> Result<ServerMessage, String> {
    while let Some(frame) = socket.next().await {
        match frame.map_err(|error| error.to_string())? {
            Message::Text(text) => {
                return serde_json::from_str(&text)
                    .map_err(|error| format!("invalid daemon message: {error}"));
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    Err("the daemon closed the connection".to_owned())
}

async fn send(socket: &mut Socket, message: &ClientMessage) -> Result<(), String> {
    let text = serde_json::to_string(message).map_err(|error| error.to_string())?;
    socket
        .send(Message::text(text))
        .await
        .map_err(|error| error.to_string())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}
