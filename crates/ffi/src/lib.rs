//! The engine for the iOS app, through Swift bindings that UniFFI generates.
//!
//! Protocol values cross as JSON, the same shapes the desktop app's webview reads, so this
//! facade stays a handful of calls and a protocol change needs no new bindings. Swift
//! decodes them with types written to match `protocol`.
//!
//! The engine runs on a runtime of its own, because Swift's async calls don't run inside
//! Tokio.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use engine::{ClientStore, Engine, Network, Remote, Update};
use protocol::{Command, MachineId, SessionId, SessionRef, TerminalColors, TerminalId};
use tokio::runtime::Runtime;
use tokio::sync::broadcast::error::{RecvError, TryRecvError};
use tokio::task::JoinHandle;

uniffi::setup_scaffolding!();

/// Streaming output arrives token by token, so the app is woken at most once per frame.
const FRAME: Duration = Duration::from_millis(16);

#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum CoreError {
    #[error("{0}")]
    Failed(String),
}

fn failed(error: impl ToString) -> CoreError {
    CoreError::Failed(error.to_string())
}

/// Told when something changed, so the app rereads what it shows. `devices` says whether the
/// device tree changed, and otherwise only open sessions have new events.
#[uniffi::export(with_foreign)]
pub trait Listener: Send + Sync {
    fn changed(&self, devices: bool);
}

/// Gets a terminal's frames: kind 1 is output to write after what came before, and kind 2 a
/// reset, whose payload starts with the columns and rows as two little-endian `u16`s. Each
/// payload's length is what to acknowledge once drawn. Called off the main thread.
#[uniffi::export(with_foreign)]
pub trait TerminalSink: Send + Sync {
    fn frame(&self, kind: u8, payload: Vec<u8>);
}

#[derive(uniffi::Object)]
pub struct Core {
    engine: Arc<Engine>,
    store: ClientStore,
    listener: Mutex<Option<JoinHandle<()>>>,
    /// Last, so it shuts down after the engine has let go of its connections.
    runtime: Runtime,
}

#[uniffi::export]
impl Core {
    /// Keeps this client's key and the devices it paired with in `data_dir`, and starts
    /// connecting to each of those devices.
    #[uniffi::constructor]
    pub fn new(data_dir: String, client_name: String) -> Result<Arc<Self>, CoreError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("tectr-engine")
            .enable_all()
            .build()
            .map_err(failed)?;
        let store = ClientStore::new(data_dir);
        let key = store.key().map_err(failed)?;
        let engine = {
            let _inside = runtime.enter();
            let engine = Engine::with_remote(
                client_name,
                Remote {
                    key,
                    network: Network::Public,
                },
            );
            for url in store.daemons() {
                engine.add_daemon(&url);
            }
            engine
        };
        Ok(Arc::new(Self {
            engine: Arc::new(engine),
            store,
            listener: Mutex::new(None),
            runtime,
        }))
    }

    /// Calls `listener` after changes, at most once a frame. A new listener replaces the last.
    pub fn listen(&self, listener: Arc<dyn Listener>) {
        let mut updates = self.engine.updates();
        let task = self.runtime.spawn(async move {
            loop {
                let mut devices = match updates.recv().await {
                    Ok(Update::SessionEvent { .. } | Update::SessionReplayed { .. }) => false,
                    Ok(Update::DeviceChanged { .. }) | Err(RecvError::Lagged(_)) => true,
                    Err(RecvError::Closed) => return,
                };
                tokio::time::sleep(FRAME).await;
                loop {
                    match updates.try_recv() {
                        Ok(Update::SessionEvent { .. } | Update::SessionReplayed { .. }) => {}
                        Ok(Update::DeviceChanged { .. }) | Err(TryRecvError::Lagged(_)) => {
                            devices = true;
                        }
                        Err(TryRecvError::Empty | TryRecvError::Closed) => break,
                    }
                }
                listener.changed(devices);
            }
        });
        if let Some(previous) = self
            .listener
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .replace(task)
        {
            previous.abort();
        }
    }

    /// Every device as JSON: `engine::Device`s, each with its part of the tree.
    pub fn devices(&self) -> String {
        json(&self.engine.devices())
    }

    /// Runs a `protocol::Command`, given as JSON, and answers with its `CommandOutput`.
    pub async fn request(&self, machine_id: String, command: String) -> Result<String, CoreError> {
        let command: Command = serde_json::from_str(&command).map_err(failed)?;
        let engine = self.engine.clone();
        let output = self
            .runtime
            .spawn(async move { engine.request(&MachineId(machine_id), command).await })
            .await
            .map_err(failed)?
            .map_err(failed)?;
        Ok(json(&output))
    }

    /// Tries every device that isn't connected again now, for when someone comes back to the
    /// app or asks.
    pub fn reconnect(&self) {
        self.engine.reconnect();
    }

    pub fn open_session(&self, machine_id: String, session_id: String) {
        self.engine.open_session(&session(machine_id, session_id));
    }

    pub fn close_session(&self, machine_id: String, session_id: String) {
        self.engine.close_session(&session(machine_id, session_id));
    }

    /// An open session's `SessionEvent`s after `after_seq`, as JSON.
    pub fn events_after(&self, machine_id: String, session_id: String, after_seq: u64) -> String {
        json(
            &self
                .engine
                .events_after(&session(machine_id, session_id), after_seq),
        )
    }

    /// Whether an open session's history has all arrived.
    pub fn is_replayed(&self, machine_id: String, session_id: String) -> bool {
        self.engine.is_replayed(&session(machine_id, session_id))
    }

    /// Starts showing a terminal. `colors` is a `TerminalColors` as JSON, for programs that
    /// ask the terminal for them.
    pub fn attach_terminal(
        &self,
        machine_id: String,
        terminal_id: String,
        cols: u16,
        rows: u16,
        colors: Option<String>,
        sink: Arc<dyn TerminalSink>,
    ) {
        let colors = colors.and_then(|colors| serde_json::from_str::<TerminalColors>(&colors).ok());
        let sink = Arc::new(move |kind: protocol::frame::Kind, payload: Vec<u8>| {
            sink.frame(kind as u8, payload);
        });
        self.engine.attach_terminal(
            &MachineId(machine_id),
            &TerminalId(terminal_id),
            (cols, rows),
            colors,
            sink,
        );
    }

    pub fn detach_terminal(&self, machine_id: String, terminal_id: String) {
        self.engine
            .detach_terminal(&MachineId(machine_id), &TerminalId(terminal_id));
    }

    pub fn resize_terminal(&self, machine_id: String, terminal_id: String, cols: u16, rows: u16) {
        self.engine.resize_terminal(
            &MachineId(machine_id),
            &TerminalId(terminal_id),
            (cols, rows),
        );
    }

    pub fn terminal_input(
        &self,
        machine_id: String,
        terminal_id: String,
        data: Vec<u8>,
    ) -> Result<(), CoreError> {
        self.engine
            .terminal_input(&MachineId(machine_id), &TerminalId(terminal_id), &data)
            .map_err(failed)
    }

    pub fn terminal_ack(&self, machine_id: String, terminal_id: String, bytes: u32) {
        self.engine
            .terminal_ack(&MachineId(machine_id), &TerminalId(terminal_id), bytes);
    }

    /// Asks the device that showed `code` to pair. `name` is what that device lists this
    /// phone as. The answer has the digits to type on the device.
    pub async fn pair(&self, code: String, name: String) -> Result<Arc<Pairing>, CoreError> {
        let engine = self.engine.clone();
        let pending = self
            .runtime
            .spawn(async move { engine.pair(&code, &name).await })
            .await
            .map_err(failed)?
            .map_err(failed)?;
        Ok(Arc::new(Pairing {
            digits: pending.digits.clone(),
            machine_name: pending.machine.name.clone(),
            pending: Mutex::new(Some(pending)),
            stop: Mutex::new(None),
        }))
    }

    /// Waits for the pairing's digits to be typed on the device, then keeps the device.
    pub async fn finish_pairing(&self, pairing: Arc<Pairing>) -> Result<(), CoreError> {
        let pending = pairing
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .ok_or_else(|| failed("this pairing already finished"))?;
        let (stop, stopped) = tokio::sync::oneshot::channel();
        *pairing.stop.lock().unwrap_or_else(PoisonError::into_inner) = Some(stop);
        let engine = self.engine.clone();
        let url = self
            .runtime
            .spawn(async move {
                tokio::select! {
                    url = engine.finish_pairing(pending) => url.map_err(failed),
                    _ = stopped => Err(failed("stopped pairing")),
                }
            })
            .await
            .map_err(failed)??;
        self.store.add_daemon(&url).map_err(failed)
    }

    /// Stops reaching a device, and takes this phone off its allowed list first while it can.
    pub async fn forget(&self, url: String) -> Result<(), CoreError> {
        let machine = self
            .engine
            .devices()
            .into_iter()
            .find(|device| device.url == url && device.connection.is_online())
            .and_then(|device| device.state)
            .map(|state| state.machine.id);
        if let Some(machine) = machine {
            let key = self.client_key()?;
            let engine = self.engine.clone();
            // It may already have removed this phone itself.
            let _ = self
                .runtime
                .spawn(async move {
                    engine
                        .request(&machine, Command::RemovePairedClient { key })
                        .await
                })
                .await;
        }
        self.store.remove_daemon(&url).map_err(failed)?;
        self.engine.remove_daemon(&url);
        Ok(())
    }

    /// This phone's public key, which paired devices list it by.
    pub fn client_key(&self) -> Result<String, CoreError> {
        let key = self.store.key().map_err(failed)?;
        Ok(key.public().to_string())
    }
}

/// A pairing whose digits someone types on the device. Stopping it, or letting it go, takes
/// this phone off the device's waiting list.
#[derive(uniffi::Object)]
pub struct Pairing {
    digits: String,
    machine_name: String,
    pending: Mutex<Option<engine::PendingPairing>>,
    /// Dropping it stops `finish_pairing`.
    stop: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

#[uniffi::export]
impl Pairing {
    pub fn digits(&self) -> String {
        self.digits.clone()
    }

    /// The device's name, where the digits are typed.
    pub fn machine_name(&self) -> String {
        self.machine_name.clone()
    }

    pub fn stop(&self) {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        self.stop
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
    }
}

fn session(machine_id: String, session_id: String) -> SessionRef {
    SessionRef {
        machine_id: MachineId(machine_id),
        session_id: SessionId(session_id),
    }
}

fn json(value: &impl serde::Serialize) -> String {
    serde_json::to_string(value).expect("protocol values always serialize")
}

/// Why a prompt can't run, when it's an agent command for something tectr sets itself.
#[uniffi::export]
pub fn refused_prompt(prompt: String) -> Option<String> {
    protocol::refused_prompt(&prompt)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core(dir: &tempfile::TempDir) -> Arc<Core> {
        Core::new(dir.path().to_string_lossy().into_owned(), "test".into())
            .expect("the core starts")
    }

    #[test]
    fn a_new_phone_has_no_devices_and_keeps_its_key() {
        let dir = tempfile::tempdir().unwrap();
        let first = core(&dir);
        assert_eq!(first.devices(), "[]");
        let key = first.client_key().unwrap();
        drop(first);
        assert_eq!(core(&dir).client_key().unwrap(), key);
    }

    #[test]
    fn a_command_that_isnt_one_fails_before_reaching_a_device() {
        let dir = tempfile::tempdir().unwrap();
        let core = core(&dir);
        let answer = block_on(core.request("m".into(), r#"{"command":"#.into()));
        assert!(answer.is_err());
        let offline =
            block_on(core.request("m".into(), r#"{"command":"list_conversations"}"#.into()));
        assert!(matches!(offline, Err(CoreError::Failed(message)) if message.contains("offline")));
    }

    #[test]
    fn prompts_for_what_tectr_sets_are_refused() {
        assert!(refused_prompt("/model opus".into()).is_some());
        assert!(refused_prompt("/usage".into()).is_none());
    }

    /// Runs a future the way Swift does, outside any Tokio runtime.
    fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future)
    }
}
