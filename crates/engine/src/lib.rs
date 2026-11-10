//! The engine shared by every tectr app. The UI sends it commands and reads
//! state from it, and never touches a socket.
//!
//! The engine is built around a list of daemons. It keeps one connection to each
//! with automatic reconnects, merges what they report, and resumes each open
//! session's events from the last sequence number it saw.
//!
//! A connection is only trusted while its daemon keeps answering. The engine pings on a
//! timer, shows a device that has gone quiet as not responding, and then drops the
//! connection, so a command never sits on a dead one. See [`protocol::liveness`].
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
use protocol::frame::{self, Kind};
pub use protocol::liveness::Liveness;
use protocol::liveness::SleepWatch;
use protocol::{
    Approval, ClientMessage, Command, CommandError, CommandOutput, DaemonState,
    MIN_PROTOCOL_VERSION, Machine, MachineId, PAIRING_ALPN, PROTOCOL_VERSION, PairRequest,
    PairResponse, PairingCode, REMOTE_ALPN, ServerMessage, SessionEvent, SessionRef,
    TerminalColors, TerminalId,
};
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::sync::{Notify, broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

const MIN_BACKOFF: Duration = Duration::from_millis(100);
const MAX_BACKOFF: Duration = Duration::from_secs(10);
const UPDATE_BUFFER: usize = 4096;
/// How long one attempt to connect may take. An attempt at a device that's away resends
/// at growing gaps, so a fresh one finds a device that just came back sooner.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
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
    Online {
        /// It hasn't answered for a while. Commands still go out, and either it answers
        /// or the connection drops and they fail.
        stalled: bool,
    },
    Offline {
        reason: String,
    },
    /// The daemon and this client have no protocol version in common.
    Incompatible {
        daemon_min: u32,
        daemon_max: u32,
    },
}

impl Connection {
    /// Whether commands can be sent, which a stalled connection still takes.
    pub fn is_online(&self) -> bool {
        matches!(self, Connection::Online { .. })
    }
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
    /// With the device's name, or its ID if it has never connected.
    #[error("{0} is offline")]
    Offline(String),
    /// The connection dropped before the answer came. The command may or may
    /// not have run, so the engine doesn't retry it.
    #[error("the connection dropped before the device answered")]
    Disconnected,
    #[error("{}", .0.message)]
    Command(CommandError),
    #[error("{0}")]
    Pairing(String),
}

/// Gets a terminal's frames: [`Kind::Output`] or [`Kind::Reset`] with its payload, which is
/// also what to acknowledge once drawn. It's called on the connection's task, so it should
/// hand the frame on rather than draw it.
pub type TerminalSink = Arc<dyn Fn(Kind, Vec<u8>) + Send + Sync>;

/// A terminal the UI shows, attached again after every reconnect.
struct AttachedTerminal {
    cols: u16,
    rows: u16,
    colors: Option<TerminalColors>,
    sink: TerminalSink,
}

enum Outgoing {
    Message(ClientMessage),
    Frame(Vec<u8>),
    /// Asks the daemon's websocket for a pong, which shows it's still there.
    Ping,
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
    outgoing: Option<mpsc::UnboundedSender<Outgoing>>,
    waiting: HashMap<u64, oneshot::Sender<Result<CommandOutput, CommandError>>>,
}

struct Shared {
    daemons: BTreeMap<String, DaemonEntry>,
    /// Sessions the UI is following, with all their events so far.
    open: HashMap<SessionRef, Vec<SessionEvent>>,
    terminals: HashMap<(MachineId, TerminalId), AttachedTerminal>,
}

impl Shared {
    fn outgoing(&self, machine_id: &MachineId) -> Option<&mpsc::UnboundedSender<Outgoing>> {
        self.daemons
            .values()
            .find(|daemon| {
                daemon.device.state.as_ref().map(|state| &state.machine.id) == Some(machine_id)
            })?
            .outgoing
            .as_ref()
    }

    /// What to call a machine in an error: its device's name, or its ID if it has never
    /// connected.
    fn name_of(&self, machine_id: &MachineId) -> String {
        self.daemons
            .values()
            .filter_map(|daemon| daemon.device.state.as_ref())
            .find(|state| &state.machine.id == machine_id)
            .map_or_else(
                || machine_id.to_string(),
                |state| state.machine.name.clone(),
            )
    }
}

struct Inner {
    shared: Mutex<Shared>,
    updates: broadcast::Sender<Update>,
    next_request: AtomicU64,
    client_name: String,
    remote: Option<RemoteSetup>,
    liveness: Liveness,
    /// Wakes every connection that's waiting to try again.
    retry: Notify,
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
                    terminals: HashMap::new(),
                }),
                updates,
                next_request: AtomicU64::new(1),
                client_name,
                remote,
                liveness: Liveness::default(),
                retry: Notify::new(),
            }),
            tasks: Mutex::new(HashMap::new()),
        }
    }

    /// Changes how soon a quiet connection is given up on, which tests shorten. It only
    /// works before the first daemon is added.
    pub fn with_liveness(mut self, liveness: Liveness) -> Self {
        Arc::get_mut(&mut self.inner)
            .expect("liveness is set before any daemon is added")
            .liveness = liveness;
        self
    }

    /// Tries every device that isn't connected again now, instead of after its wait. An
    /// attempt already under way starts over, because one at a device that was away
    /// resends at growing gaps and a fresh one gets through sooner. So this is for when
    /// someone asks or comes back, not for calling in a loop.
    pub fn reconnect(&self) {
        self.inner.retry.notify_waiters();
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

    /// Runs a command on a machine. It fails right away if the machine is offline, and
    /// with [`EngineError::Disconnected`] if the connection drops before the answer.
    pub async fn request(
        &self,
        machine_id: &MachineId,
        command: Command,
    ) -> Result<CommandOutput, EngineError> {
        let id = self.inner.next_request.fetch_add(1, Ordering::Relaxed);
        let answer = {
            let mut shared = self.inner.lock();
            let reachable = shared.daemons.values_mut().find(|daemon| {
                daemon.outgoing.is_some()
                    && daemon.device.state.as_ref().map(|state| &state.machine.id)
                        == Some(machine_id)
            });
            let Some(daemon) = reachable else {
                return Err(EngineError::Offline(shared.name_of(machine_id)));
            };
            let (answer_tx, answer_rx) = oneshot::channel();
            daemon.waiting.insert(id, answer_tx);
            if let Some(outgoing) = &daemon.outgoing {
                let _ = outgoing.send(Outgoing::Message(ClientMessage::Request { id, command }));
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
                let _ = outgoing.send(Outgoing::Message(ClientMessage::Subscribe {
                    session_id: session.session_id.clone(),
                    after_seq: 0,
                }));
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
                let _ = outgoing.send(Outgoing::Message(ClientMessage::Unsubscribe {
                    session_id: session.session_id.clone(),
                }));
            }
        }
    }

    /// Starts showing a terminal at this size. Its frames go to `sink` until it's detached,
    /// across reconnects, each of which starts with a reset.
    pub fn attach_terminal(
        &self,
        machine_id: &MachineId,
        terminal_id: &TerminalId,
        (cols, rows): (u16, u16),
        colors: Option<TerminalColors>,
        sink: TerminalSink,
    ) {
        let mut shared = self.inner.lock();
        if let Some(outgoing) = shared.outgoing(machine_id) {
            let _ = outgoing.send(Outgoing::Message(ClientMessage::AttachTerminal {
                terminal_id: terminal_id.clone(),
                cols,
                rows,
                colors: colors.clone(),
            }));
        }
        shared.terminals.insert(
            (machine_id.clone(), terminal_id.clone()),
            AttachedTerminal {
                cols,
                rows,
                colors,
                sink,
            },
        );
    }

    pub fn detach_terminal(&self, machine_id: &MachineId, terminal_id: &TerminalId) {
        let mut shared = self.inner.lock();
        let key = (machine_id.clone(), terminal_id.clone());
        if shared.terminals.remove(&key).is_some()
            && let Some(outgoing) = shared.outgoing(machine_id)
        {
            let _ = outgoing.send(Outgoing::Message(ClientMessage::DetachTerminal {
                terminal_id: terminal_id.clone(),
            }));
        }
    }

    /// The terminal is shown at a new size. The daemon resets every client showing it.
    pub fn resize_terminal(
        &self,
        machine_id: &MachineId,
        terminal_id: &TerminalId,
        (cols, rows): (u16, u16),
    ) {
        let mut shared = self.inner.lock();
        let key = (machine_id.clone(), terminal_id.clone());
        let Some(attached) = shared.terminals.get_mut(&key) else {
            return;
        };
        attached.cols = cols;
        attached.rows = rows;
        if let Some(outgoing) = shared.outgoing(machine_id) {
            let _ = outgoing.send(Outgoing::Message(ClientMessage::ResizeTerminal {
                terminal_id: terminal_id.clone(),
                cols,
                rows,
            }));
        }
    }

    /// What was typed or pasted. It fails right away if the machine is offline, because
    /// keys typed into a terminal that isn't there shouldn't arrive later.
    pub fn terminal_input(
        &self,
        machine_id: &MachineId,
        terminal_id: &TerminalId,
        data: &[u8],
    ) -> Result<(), EngineError> {
        let shared = self.inner.lock();
        let outgoing = shared
            .outgoing(machine_id)
            .ok_or_else(|| EngineError::Offline(shared.name_of(machine_id)))?;
        let frame = frame::encode(Kind::Input, terminal_id, data);
        outgoing
            .send(Outgoing::Frame(frame))
            .map_err(|_| EngineError::Disconnected)
    }

    /// How many payload bytes of a terminal's frames have been drawn.
    pub fn terminal_ack(&self, machine_id: &MachineId, terminal_id: &TerminalId, bytes: u32) {
        let shared = self.inner.lock();
        if let Some(outgoing) = shared.outgoing(machine_id) {
            let frame = frame::encode(Kind::Ack, terminal_id, &frame::ack_payload(bytes));
            let _ = outgoing.send(Outgoing::Frame(frame));
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

/// Why a connection ended, or never started.
enum Lost {
    /// What to show until the next attempt.
    Because(String),
    /// Asked to try again in the middle of an attempt. What's shown stays as it is, and
    /// the next attempt starts straight away.
    Retry,
}

impl From<String> for Lost {
    fn from(reason: String) -> Self {
        Lost::Because(reason)
    }
}

/// Keeps one daemon connected for as long as the engine lives.
async fn maintain_connection(inner: Arc<Inner>, url: String) {
    let mut backoff = MIN_BACKOFF;
    loop {
        let mut was_online = false;
        let outcome = connect_and_run(&inner, &url, &mut was_online).await;
        if was_online {
            backoff = MIN_BACKOFF;
        }
        let retry_now = matches!(outcome, Err(Lost::Retry));
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
            match outcome {
                Ok(state) => daemon.device.connection = state,
                Err(Lost::Because(reason)) => {
                    daemon.device.connection = Connection::Offline { reason };
                }
                Err(Lost::Retry) => {}
            }
        }
        inner.device_changed(&url);
        if retry_now {
            backoff = MIN_BACKOFF;
            continue;
        }
        tokio::select! {
            () = tokio::time::sleep(backoff) => backoff = (backoff * 2).min(MAX_BACKOFF),
            () = inner.retry.notified() => backoff = MIN_BACKOFF,
        }
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

enum Handshake {
    /// The iroh connection comes along, because dropping it would close the stream.
    Ready(Box<Socket>, Option<iroh::endpoint::Connection>),
    /// The daemon answered, and this client can't talk to it.
    Refused(Connection),
}

/// Connects and agrees on a protocol version.
async fn handshake(inner: &Inner, url: &str) -> Result<Handshake, String> {
    let (mut socket, connection) = open(inner, url).await?;
    send(
        &mut socket,
        Outgoing::Message(ClientMessage::Hello {
            min_version: MIN_PROTOCOL_VERSION,
            max_version: PROTOCOL_VERSION,
            client: inner.client_name.clone(),
        }),
    )
    .await?;

    let welcome = loop {
        match next_message(&mut socket).await? {
            Incoming::Message(message) => break *message,
            Incoming::Frame(_) | Incoming::Alive => {}
        }
    };
    let refused = |daemon_min, daemon_max| {
        Handshake::Refused(Connection::Incompatible {
            daemon_min,
            daemon_max,
        })
    };
    match welcome {
        ServerMessage::Welcome { version, .. }
            if (MIN_PROTOCOL_VERSION..=PROTOCOL_VERSION).contains(&version) =>
        {
            Ok(Handshake::Ready(Box::new(socket), connection))
        }
        ServerMessage::Welcome { version, .. } => Ok(refused(version, version)),
        ServerMessage::VersionRejected {
            min_version,
            max_version,
        } => Ok(refused(min_version, max_version)),
        other => Err(format!("expected a welcome, got {other:?}")),
    }
}

/// Runs one connection until it ends. Returns what to show from then on, and sets
/// `online` once the daemon's state has arrived.
async fn connect_and_run(inner: &Inner, url: &str, online: &mut bool) -> Result<Connection, Lost> {
    let attempt = tokio::time::timeout(CONNECT_TIMEOUT, handshake(inner, url));
    let (mut socket, connection) = tokio::select! {
        result = attempt => {
            match result.map_err(|_| "the device didn't answer in time".to_owned())?? {
                Handshake::Ready(socket, connection) => (*socket, connection),
                Handshake::Refused(state) => return Ok(state),
            }
        }
        () = inner.retry.notified() => return Err(Lost::Retry),
    };

    let liveness = inner.liveness;
    let quiet = || "the device stopped answering".to_owned();
    let mut ping =
        tokio::time::interval_at(Instant::now() + liveness.ping_every, liveness.ping_every);
    ping.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut heard = Instant::now();
    let mut stalled = false;
    let mut sleep = SleepWatch::start();
    let (outgoing, mut to_send) = mpsc::unbounded_channel();
    loop {
        let deadline = heard + liveness.silence_limit;
        tokio::select! {
            message = next_message(&mut socket) => {
                let message = message?;
                heard = Instant::now();
                if stalled {
                    stalled = false;
                    set_stalled(inner, url, false);
                }
                match message {
                    Incoming::Message(message) => handle_message(inner, url, *message, &outgoing, online),
                    Incoming::Frame(data) => handle_frame(inner, url, data),
                    Incoming::Alive => {}
                }
            }
            // A daemon that stopped reading would hold a send up for good, and the limit
            // with it, so the limit covers sends too.
            Some(message) = to_send.recv() => {
                tokio::time::timeout_at(deadline, send(&mut socket, message))
                    .await
                    .map_err(|_| quiet())??;
            }
            _ = ping.tick() => {
                // Timers stood still while this machine slept, but a daemon on another
                // machine counted the silence and has let go of this connection. One on
                // this machine slept too, and its connection is as it was.
                if connection.is_some() && sleep.slept() >= liveness.daemon_limit() {
                    return Err("this machine was asleep".to_owned().into());
                }
                if !stalled && heard.elapsed() >= liveness.stalled_after {
                    stalled = true;
                    set_stalled(inner, url, true);
                }
                tokio::time::timeout_at(deadline, send(&mut socket, Outgoing::Ping))
                    .await
                    .map_err(|_| quiet())??;
            }
            () = tokio::time::sleep_until(deadline) => return Err(quiet().into()),
        }
    }
}

/// Shows an online device as not responding, or as responding again.
fn set_stalled(inner: &Inner, url: &str, stalled: bool) {
    {
        let mut shared = inner.lock();
        let Some(daemon) = shared.daemons.get_mut(url) else {
            return;
        };
        let Connection::Online { stalled: shown } = &mut daemon.device.connection else {
            return;
        };
        if *shown == stalled {
            return;
        }
        *shown = stalled;
    }
    inner.device_changed(url);
}

/// Hands a terminal's frame to whoever shows it, outside the lock.
fn handle_frame(inner: &Inner, url: &str, data: Vec<u8>) {
    let Some(frame) = frame::decode(&data) else {
        return;
    };
    if !matches!(frame.kind, Kind::Output | Kind::Reset) {
        return;
    }
    let sink = {
        let shared = inner.lock();
        let Some(machine_id) = shared
            .daemons
            .get(url)
            .and_then(|daemon| daemon.device.state.as_ref())
            .map(|state| state.machine.id.clone())
        else {
            return;
        };
        let key = (machine_id, TerminalId::from(frame.terminal_id));
        let Some(attached) = shared.terminals.get(&key) else {
            return; // Detached while this was on its way.
        };
        attached.sink.clone()
    };
    sink(frame.kind, frame.payload.to_vec());
}

fn handle_message(
    inner: &Inner,
    url: &str,
    message: ServerMessage,
    outgoing: &mpsc::UnboundedSender<Outgoing>,
    online: &mut bool,
) {
    match message {
        ServerMessage::Snapshot { state } => {
            {
                let mut shared = inner.lock();
                let Shared {
                    daemons,
                    open,
                    terminals,
                } = &mut *shared;
                let Some(daemon) = daemons.get_mut(url) else {
                    return;
                };
                // The daemon also sends a snapshot when this client falls
                // behind. Only the first one on a connection means "online".
                if !*online {
                    resubscribe(open, terminals, &state.machine, outgoing);
                    daemon.outgoing = Some(outgoing.clone());
                    daemon.device.connection = Connection::Online { stalled: false };
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

/// After connecting, picks up every open session on this machine where it left off, and shows
/// its terminals again.
fn resubscribe(
    open: &HashMap<SessionRef, Vec<SessionEvent>>,
    terminals: &HashMap<(MachineId, TerminalId), AttachedTerminal>,
    machine: &Machine,
    outgoing: &mpsc::UnboundedSender<Outgoing>,
) {
    for (session, events) in open {
        if session.machine_id == machine.id {
            let _ = outgoing.send(Outgoing::Message(ClientMessage::Subscribe {
                session_id: session.session_id.clone(),
                after_seq: events.last().map_or(0, |event| event.seq),
            }));
        }
    }
    for ((machine_id, terminal_id), attached) in terminals {
        if machine_id == &machine.id {
            let _ = outgoing.send(Outgoing::Message(ClientMessage::AttachTerminal {
                terminal_id: terminal_id.clone(),
                cols: attached.cols,
                rows: attached.rows,
                colors: attached.colors.clone(),
            }));
        }
    }
}

enum Incoming {
    Message(Box<ServerMessage>),
    Frame(Vec<u8>),
    /// A pong, or a ping: nothing but a sign that the daemon is there.
    Alive,
}

async fn next_message(socket: &mut Socket) -> Result<Incoming, String> {
    let closed = || "the device closed the connection".to_owned();
    let frame = socket.next().await.ok_or_else(closed)?;
    match frame.map_err(|error| error.to_string())? {
        Message::Text(text) => serde_json::from_str(&text)
            .map(|message| Incoming::Message(Box::new(message)))
            .map_err(|error| format!("invalid daemon message: {error}")),
        Message::Binary(data) => Ok(Incoming::Frame(data.to_vec())),
        Message::Close(_) => Err(closed()),
        _ => Ok(Incoming::Alive),
    }
}

async fn send(socket: &mut Socket, message: Outgoing) -> Result<(), String> {
    let message = match message {
        Outgoing::Message(message) => {
            Message::text(serde_json::to_string(&message).map_err(|error| error.to_string())?)
        }
        Outgoing::Frame(frame) => Message::binary(frame),
        Outgoing::Ping => Message::Ping(Default::default()),
    };
    socket
        .send(message)
        .await
        .map_err(|error| error.to_string())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}
