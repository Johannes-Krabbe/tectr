//! The engine shared by every tectr app. The UI sends it commands and reads
//! state from it, and never touches a socket.
//!
//! The engine is built around a list of daemons. It keeps one connection to each
//! with automatic reconnects, merges what they report, and resumes each open
//! session's events from the last sequence number it saw.
//!
//! Not here yet: the disk cache of last known state, per-frame batching of
//! updates (the desktop app does its own for now), and the `suspend()`/`resume()`
//! calls for iOS.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use protocol::frame::{self, Kind};
use protocol::{
    Approval, ClientMessage, Command, CommandError, CommandOutput, DaemonState,
    MIN_PROTOCOL_VERSION, Machine, MachineId, PROTOCOL_VERSION, ServerMessage, SessionEvent,
    SessionRef, TerminalColors, TerminalId,
};
use serde::Serialize;
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const MIN_BACKOFF: Duration = Duration::from_millis(100);
const MAX_BACKOFF: Duration = Duration::from_secs(10);
const UPDATE_BUFFER: usize = 4096;

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
}

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
}

struct Inner {
    shared: Mutex<Shared>,
    updates: broadcast::Sender<Update>,
    next_request: AtomicU64,
    client_name: String,
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
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl Drop for Engine {
    fn drop(&mut self) {
        for task in self
            .tasks
            .lock()
            .expect("a panic poisoned the task list")
            .drain(..)
        {
            task.abort();
        }
    }
}

impl Engine {
    pub fn new(client_name: impl Into<String>) -> Self {
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
                client_name: client_name.into(),
            }),
            tasks: Mutex::new(vec![]),
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
            .push(task);
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
            .ok_or_else(|| EngineError::Offline(machine_id.clone()))?;
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
            let daemon = shared
                .daemons
                .get_mut(&url)
                .expect("daemons are never removed");
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

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

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
    let (mut socket, _) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|error| error.to_string())?;
    // Typing in a terminal sends a few bytes at a time, which shouldn't wait to be combined.
    if let MaybeTlsStream::Plain(stream) = socket.get_ref() {
        let _ = stream.set_nodelay(true);
    }
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
            Incoming::Frame(_) => {}
        }
    };
    match welcome {
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
            message = next_message(&mut socket) => match message? {
                Incoming::Message(message) => handle_message(inner, url, *message, &outgoing, online),
                Incoming::Frame(data) => handle_frame(inner, url, data),
            },
            Some(message) = to_send.recv() => send(&mut socket, message).await?,
        }
    }
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
                let daemon = daemons.get_mut(url).expect("daemons are never removed");
                // The daemon also sends a snapshot when this client falls
                // behind. Only the first one on a connection means "online".
                if !*online {
                    resubscribe(open, terminals, &state.machine, outgoing);
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
                let daemon = shared
                    .daemons
                    .get_mut(url)
                    .expect("daemons are never removed");
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
            let daemon = shared
                .daemons
                .get_mut(url)
                .expect("daemons are never removed");
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
}

async fn next_message(socket: &mut Socket) -> Result<Incoming, String> {
    while let Some(frame) = socket.next().await {
        match frame.map_err(|error| error.to_string())? {
            Message::Text(text) => {
                return serde_json::from_str(&text)
                    .map(|message| Incoming::Message(Box::new(message)))
                    .map_err(|error| format!("invalid daemon message: {error}"));
            }
            Message::Binary(data) => return Ok(Incoming::Frame(data.to_vec())),
            Message::Close(_) => break,
            _ => {}
        }
    }
    Err("the daemon closed the connection".to_owned())
}

async fn send(socket: &mut Socket, message: Outgoing) -> Result<(), String> {
    let message = match message {
        Outgoing::Message(message) => {
            Message::text(serde_json::to_string(&message).map_err(|error| error.to_string())?)
        }
        Outgoing::Frame(frame) => Message::binary(frame),
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
