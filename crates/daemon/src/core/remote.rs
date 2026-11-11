//! Pairing and the allowed list. Connections themselves are in `crate::remote`.

use std::sync::Arc;
use std::time::Duration;

use iroh::EndpointId;
use protocol::{
    CommandError, CommandOutput, PairRequest, PairResponse, PairedClient, PairingCode, StateChange,
};

use super::{Core, internal, not_found, now_ms};

/// Long enough to carry a code to another machine, short enough that one left on a screen
/// stops working soon.
const PAIRING_LIFETIME: Duration = Duration::from_secs(5 * 60);

/// How long a code keeps working past the expiry it reports, even once a new code has
/// replaced it. A client that scanned it as the clock ran out still gets in.
const PAIRING_GRACE: Duration = Duration::from_secs(10);

/// Wrong secrets a code survives. The secret is far too long to guess, so this only
/// stops someone from trying forever.
const PAIRING_ATTEMPTS: u32 = 5;

/// Longer names are cut, because the name comes from the machine that wants in.
const MAX_CLIENT_NAME: usize = 100;

/// A code shown on this device, waiting for a client to use it.
pub(super) struct Pairing {
    secret: String,
    pub(super) expires_at_ms: i64,
    wrong_attempts: u32,
}

impl Pairing {
    fn open(&self, now: i64) -> bool {
        now <= self.expires_at_ms + PAIRING_GRACE.as_millis() as i64
    }
}

impl Core {
    /// What clients dial to reach this daemon over iroh.
    pub fn remote_id(&self) -> EndpointId {
        self.remote_key.public()
    }

    pub fn has_paired_clients(&self) -> bool {
        !self.lock().state.paired_clients.is_empty()
    }

    pub fn remote_addr(&self) -> Option<iroh::EndpointAddr> {
        let endpoint = self.endpoint.get()?;
        Some(iroh::EndpointAddr::from_parts(
            endpoint.id(),
            endpoint
                .bound_sockets()
                .into_iter()
                .map(iroh::TransportAddr::Ip),
        ))
    }

    pub fn is_paired(&self, key: &EndpointId) -> bool {
        let key = key.to_string();
        self.lock()
            .state
            .paired_clients
            .iter()
            .any(|c| c.key == key)
    }

    /// Whether a code is out, so a pairing attempt is worth reading at all.
    pub fn pairing_open(&self) -> bool {
        let now = now_ms();
        self.pairing_lock().iter().any(|code| code.open(now))
    }

    /// Starts listening over iroh if nothing has yet. A daemon that's only used locally
    /// never does, so it never reaches the network.
    pub async fn ensure_remote(self: &Arc<Self>) -> anyhow::Result<()> {
        self.endpoint
            .get_or_try_init(|| async {
                let endpoint = crate::remote::bind(&self.network, self.remote_key.clone()).await?;
                tokio::spawn(crate::remote::accept_loop(self.clone(), endpoint.clone()));
                anyhow::Ok(endpoint)
            })
            .await?;
        Ok(())
    }

    pub(super) async fn start_pairing(self: &Arc<Self>) -> Result<CommandOutput, CommandError> {
        self.ensure_remote().await.map_err(internal)?;
        let secret = crate::new_id();
        let now = now_ms();
        let expires_at_ms = now + PAIRING_LIFETIME.as_millis() as i64;
        let mut codes = self.pairing_lock();
        // A new code replaces the ones before it, which keep only their grace from now.
        codes.retain(|code| code.open(now));
        for code in codes.iter_mut() {
            code.expires_at_ms = code.expires_at_ms.min(now);
        }
        codes.push(Pairing {
            secret: secret.clone(),
            expires_at_ms,
            wrong_attempts: 0,
        });
        drop(codes);
        let code = PairingCode {
            daemon_key: self.remote_id().to_string(),
            secret,
        };
        Ok(CommandOutput::Pairing {
            code: code.to_string(),
            expires_at_ms,
        })
    }

    pub(super) fn cancel_pairing(&self) -> CommandOutput {
        self.pairing_lock().clear();
        CommandOutput::Done
    }

    pub(super) fn remove_paired_client(&self, key: &str) -> Result<CommandOutput, CommandError> {
        let mut inner = self.lock();
        if !inner.store.delete_paired_client(key).map_err(internal)? {
            return Err(not_found("paired client", &key));
        }
        // Each of its connections sees this and closes.
        inner.emit(StateChange::PairedClientRemoved {
            key: key.to_owned(),
        });
        Ok(CommandOutput::Done)
    }

    /// Puts `key` on the allowed list if it brought the secret of an open code. One pairing
    /// uses up every code, and too many wrong secrets use a code up.
    pub fn pair(&self, key: &EndpointId, request: &PairRequest) -> PairResponse {
        let refused = |reason: &str| PairResponse::Refused {
            reason: reason.to_owned(),
        };
        {
            let mut codes = self.pairing_lock();
            if codes.is_empty() {
                return refused("this device isn't pairing right now");
            }
            let now = now_ms();
            codes.retain(|code| code.open(now));
            if codes.is_empty() {
                return refused("this code has expired");
            }
            // Comparing with every code, so timing doesn't tell which one was close.
            let matched = codes.iter().fold(false, |found, code| {
                found | same_secret(&code.secret, &request.secret)
            });
            if !matched {
                for code in codes.iter_mut() {
                    code.wrong_attempts += 1;
                }
                codes.retain(|code| code.wrong_attempts < PAIRING_ATTEMPTS);
                return refused("this code is wrong");
            }
            codes.clear();
        }
        let name: String = request.name.trim().chars().take(MAX_CLIENT_NAME).collect();
        let client = PairedClient {
            key: key.to_string(),
            name: if name.is_empty() {
                "Unnamed".to_owned()
            } else {
                name
            },
            paired_at_ms: now_ms(),
        };
        let mut inner = self.lock();
        if let Err(error) = inner.store.insert_paired_client(&client) {
            tracing::warn!("couldn't save a paired client: {error:#}");
            return refused("the device couldn't save the pairing");
        }
        inner.emit(StateChange::ClientPaired { client });
        PairResponse::Paired {
            machine: inner.state.machine.clone(),
        }
    }

    pub(super) fn pairing_lock(&self) -> std::sync::MutexGuard<'_, Vec<Pairing>> {
        self.pairing
            .lock()
            .expect("a panic poisoned the pairing state")
    }
}

/// Takes as long whichever character differs, so timing doesn't give the secret away.
fn same_secret(expected: &str, given: &str) -> bool {
    expected.len() == given.len()
        && expected
            .bytes()
            .zip(given.bytes())
            .fold(0, |diff, (a, b)| diff | (a ^ b))
            == 0
}
