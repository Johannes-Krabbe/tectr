//! The tectr daemon. It runs on each machine, owns that machine's workspaces,
//! topics, and sessions, and drives agents over ACP. Clients come and go over a
//! loopback websocket and always see the same event schema.

mod acp;
mod agents;
mod core;
mod git;
mod server;
mod store;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

pub use agents::{AgentLauncher, default_agents};
use anyhow::{Context, Result};
use protocol::MachineId;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::core::Core;

/// How many live updates a slow client can fall behind by before it's
/// resynchronized from the database.
const DEFAULT_EVENT_BUFFER: usize = 4096;

pub struct Config {
    pub data_dir: PathBuf,
    /// The daemon only ever listens on loopback. A network listener has to
    /// wait for pairing, because whoever can connect can run agents here.
    pub port: u16,
    pub agents: BTreeMap<String, AgentLauncher>,
    pub event_buffer: usize,
}

impl Config {
    pub fn new(data_dir: PathBuf, port: u16) -> Self {
        Self {
            data_dir,
            port,
            agents: default_agents(),
            event_buffer: DEFAULT_EVENT_BUFFER,
        }
    }
}

pub struct Daemon {
    addr: SocketAddr,
    machine_id: MachineId,
    core: Arc<Core>,
    shutdown: watch::Sender<bool>,
    accept_task: JoinHandle<()>,
}

impl Daemon {
    pub async fn start(config: Config) -> Result<Self> {
        create_private_dir(&config.data_dir)?;

        // Bind first. If another daemon already owns this port, this fails
        // before startup recovery could mark that daemon's sessions as stopped.
        let listener = TcpListener::bind(("127.0.0.1", config.port))
            .await
            .with_context(|| format!("failed to listen on 127.0.0.1:{}", config.port))?;
        let addr = listener.local_addr()?;

        if config
            .agents
            .values()
            .any(|agent| matches!(agent, AgentLauncher::ClaudeAdapter))
            && let Err(error) = agents::check_node().await
        {
            // Other agents can still work, so this isn't fatal. Starting a
            // Claude session reports the same error to the client.
            tracing::error!("{error:#}");
        }

        let core = Core::open(
            &config.data_dir,
            machine_name(),
            config.agents,
            config.event_buffer,
        )?;
        let machine_id = core.machine().id;
        let (shutdown, shutdown_rx) = watch::channel(false);
        let accept_task = tokio::spawn(server::accept_loop(core.clone(), listener, shutdown_rx));

        Ok(Self {
            addr,
            machine_id,
            core,
            shutdown,
            accept_task,
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn machine_id(&self) -> &MachineId {
        &self.machine_id
    }

    /// Closes all connections and stops all agents.
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        let _ = self.accept_task.await;
        self.core.shutdown().await;
    }
}

fn create_private_dir(path: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("failed to create {}", path.display()))?;
    // This directory holds transcripts, which can contain anything an agent
    // has read.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn machine_name() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}

pub(crate) fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}
