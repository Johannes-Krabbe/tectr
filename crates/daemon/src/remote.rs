//! Connections from clients on other machines, over iroh. A paired client gets the same
//! websocket protocol as loopback, on one QUIC stream. Any other key may only ask to pair,
//! and only while a code is out.

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use iroh::endpoint::{Connection, Incoming, presets};
use iroh::{Endpoint, EndpointId, SecretKey};
use protocol::{CLOSE_ASLEEP, PAIRING_ALPN, PairRequest, PairResponse, REMOTE_ALPN};
use tokio::task::JoinSet;

use crate::core::{Core, PAIRING_WAIT};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// A pairing request is a secret and a name, so anything longer isn't one.
const MAX_PAIR_REQUEST: usize = 4096;

/// QUIC close codes, so a refused client can tell why.
const CLOSE_DONE: u32 = 0;
const CLOSE_NOT_PAIRED: u32 = 1;

/// How the daemon reaches the network for remote clients.
#[derive(Debug, Clone, Default)]
pub enum Network {
    /// n0's public relays and address lookup, so clients find this daemon by its key alone.
    #[default]
    Public,
    /// Loopback only, with no relay or lookup, for tests. Clients need the address from
    /// [`crate::Daemon::remote_addr`].
    Local,
}

pub(crate) async fn bind(network: &Network, key: SecretKey) -> Result<Endpoint> {
    let builder = match network {
        Network::Public => Endpoint::builder(presets::N0),
        Network::Local => Endpoint::builder(presets::Minimal)
            .clear_ip_transports()
            .bind_addr("127.0.0.1:0")
            .context("invalid loopback address")?,
    };
    builder
        .secret_key(key)
        .alpns(vec![REMOTE_ALPN.to_vec(), PAIRING_ALPN.to_vec()])
        .bind()
        .await
        .context("couldn't start listening for remote clients")
}

/// Runs until the endpoint closes, which also ends every connection. It's boxed because a
/// connection can start a pairing, which is what starts this loop, and the compiler can't
/// work out that cycle's type by itself.
pub(crate) fn accept_loop(
    core: Arc<Core>,
    endpoint: Endpoint,
) -> Pin<Box<dyn Future<Output = ()> + Send>> {
    Box::pin(run_accept_loop(core, endpoint))
}

async fn run_accept_loop(core: Arc<Core>, endpoint: Endpoint) {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else { return };
                let core = core.clone();
                connections.spawn(async move {
                    if let Err(error) = handle(core, incoming).await {
                        tracing::debug!("remote connection ended: {error:#}");
                    }
                });
            }
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
}

async fn handle(core: Arc<Core>, incoming: Incoming) -> Result<()> {
    let connection = tokio::time::timeout(HANDSHAKE_TIMEOUT, incoming.accept()?)
        .await
        .context("no handshake in time")??;
    let peer = connection.remote_id();
    if connection.alpn() == REMOTE_ALPN {
        if !core.is_paired(&peer) {
            connection.close(CLOSE_NOT_PAIRED.into(), b"not paired with this device");
            return Ok(());
        }
        if in_dark_wake().await {
            tracing::debug!(peer = %peer.fmt_short(), "refused a remote client while asleep");
            connection.close(CLOSE_ASLEEP.into(), b"the device is asleep");
            return Ok(());
        }
        tracing::debug!(peer = %peer.fmt_short(), "remote client connected");
        let result = serve(core.clone(), &connection, peer).await;
        if core.is_paired(&peer) {
            connection.close(CLOSE_DONE.into(), b"");
        } else {
            connection.close(CLOSE_NOT_PAIRED.into(), b"not paired with this device");
        }
        result
    } else if connection.alpn() == PAIRING_ALPN {
        pair(&core, &connection, peer).await
    } else {
        bail!("unexpected protocol");
    }
}

async fn serve(core: Arc<Core>, connection: &Connection, peer: EndpointId) -> Result<()> {
    let (send, recv) = tokio::time::timeout(HANDSHAKE_TIMEOUT, connection.accept_bi())
        .await
        .context("no stream in time")??;
    crate::server::serve_remote(core, tokio::io::join(recv, send), peer.to_string()).await
}

async fn pair(core: &Core, connection: &Connection, peer: EndpointId) -> Result<()> {
    // Nothing to check a secret against, so there's no reason to read one.
    if !core.pairing_open() {
        connection.close(
            CLOSE_NOT_PAIRED.into(),
            b"this device isn't pairing right now",
        );
        return Ok(());
    }
    let (mut send, mut recv) = tokio::time::timeout(HANDSHAKE_TIMEOUT, connection.accept_bi())
        .await
        .context("no stream in time")??;
    let request = tokio::time::timeout(HANDSHAKE_TIMEOUT, recv.read_to_end(MAX_PAIR_REQUEST))
        .await
        .context("no pairing request in time")??;
    let wait = match serde_json::from_slice::<PairRequest>(&request) {
        Ok(request) => core.request_pairing(&peer, &request),
        Err(_) => Err(PairResponse::Refused {
            reason: "that wasn't a pairing request".to_owned(),
        }),
    };
    let response = match wait {
        Err(refused) => refused,
        Ok(wait) => {
            // However this ends, the client is no longer waiting.
            let _waiting = StopWaiting(core, wait.id);
            let confirm = PairResponse::Confirm {
                digits: wait.digits,
                machine: wait.machine,
            };
            write_line(&mut send, &confirm).await?;
            tokio::select! {
                answer = wait.answer => answer.unwrap_or_else(|_| PairResponse::Refused {
                    reason: "the device stopped pairing".to_owned(),
                }),
                _ = connection.closed() => return Ok(()),
                () = tokio::time::sleep(PAIRING_WAIT) => PairResponse::Refused {
                    reason: "nobody typed the digits on the device in time".to_owned(),
                },
            }
        }
    };
    write_line(&mut send, &response).await?;
    send.finish()?;
    // Closing now could drop the answer, so the client closes once it has read it.
    let _ = tokio::time::timeout(HANDSHAKE_TIMEOUT, connection.closed()).await;
    Ok(())
}

struct StopWaiting<'a>(&'a Core, u64);

impl Drop for StopWaiting<'_> {
    fn drop(&mut self) {
        self.0.stop_waiting(self.1);
    }
}

async fn write_line(send: &mut iroh::endpoint::SendStream, response: &PairResponse) -> Result<()> {
    let mut line = serde_json::to_vec(response)?;
    line.push(b'\n');
    send.write_all(&line).await?;
    Ok(())
}

/// Whether this machine is only briefly awake. A sleeping Mac wakes every few minutes for
/// seconds at a time, with no display, to fetch mail and keep its network up. A client let in
/// then would show it online until it falls asleep again moments later. Anything unclear
/// counts as awake, because refusing an awake machine is worse than a flicker.
async fn in_dark_wake() -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    let output = tokio::process::Command::new("/usr/bin/pmset")
        .args(["-g", "systemstate"])
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(Duration::from_secs(2), output).await {
        Ok(Ok(output)) if output.status.success() => {
            is_dark_wake(&String::from_utf8_lossy(&output.stdout))
        }
        _ => false,
    }
}

/// A fully awake Mac lists Graphics among its capabilities, display off or not. A dark wake
/// leaves it out.
fn is_dark_wake(systemstate: &str) -> bool {
    systemstate
        .lines()
        .find_map(|line| line.trim().strip_prefix("Current System Capabilities are:"))
        .is_some_and(|capabilities| {
            let mut capabilities = capabilities.split_whitespace();
            capabilities.clone().any(|capability| capability == "CPU")
                && !capabilities.any(|capability| capability == "Graphics")
        })
}

#[cfg(test)]
mod tests {
    use super::is_dark_wake;

    #[test]
    fn a_dark_wake_is_a_running_machine_without_graphics() {
        let state = |capabilities: &str| {
            format!("Current System Capabilities are: {capabilities}\nCurrent Power State: 4\n")
        };
        assert!(!is_dark_wake(&state("CPU Graphics Audio Network ")));
        assert!(is_dark_wake(&state("CPU Network ")));
        assert!(is_dark_wake(&state("CPU Audio Network")));
    }

    #[test]
    fn output_it_does_not_understand_counts_as_awake() {
        assert!(!is_dark_wake(""));
        assert!(!is_dark_wake("pmset: unknown option"));
        assert!(!is_dark_wake("Current System Capabilities are: \n"));
    }
}
