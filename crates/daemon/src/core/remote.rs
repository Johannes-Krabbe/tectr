//! Pairing and the allowed list. Connections themselves are in `crate::remote`.

use std::sync::Arc;
use std::time::Duration;

use iroh::EndpointId;
use protocol::{
    CommandError, CommandOutput, Machine, PairRequest, PairResponse, PairedClient, PairingCode,
    PairingRequest, StateChange,
};
use tokio::sync::oneshot;

use super::{Core, internal, invalid, not_found, now_ms};

/// Long enough to carry a code to another machine, short enough that one left on a screen
/// stops working soon.
const PAIRING_LIFETIME: Duration = Duration::from_secs(5 * 60);

/// How long a code keeps working past the expiry it reports, even once a new code has
/// replaced it. A client that scanned it as the clock ran out still gets in.
const PAIRING_GRACE: Duration = Duration::from_secs(10);

/// Wrong secrets a code survives. The secret is far too long to guess, so this only
/// stops someone from trying forever.
const PAIRING_ATTEMPTS: u32 = 5;

/// How long a client that brought the right secret waits for its digits to be typed.
pub(crate) const PAIRING_WAIT: Duration = PAIRING_LIFETIME;

/// Clients that may wait at once. More than one means someone else has the code, which the
/// device shows, and a few keep a stranger from filling the list for long.
const MAX_WAITING: usize = 3;

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

/// A client that brought the right secret, waiting for someone here to type its digits.
struct Waiting {
    id: u64,
    key: String,
    name: String,
    digits: String,
    answer: oneshot::Sender<PairResponse>,
}

impl Waiting {
    fn refuse(self, reason: &str) {
        let _ = self.answer.send(refused(reason));
    }
}

/// The codes out and the clients waiting. Its lock is taken before the core's, so the
/// waiting list a change carries is always the latest.
#[derive(Default)]
pub(crate) struct Pairings {
    pub(super) codes: Vec<Pairing>,
    waiting: Vec<Waiting>,
    next_id: u64,
}

/// What a client that brought the right secret shows, and how it hears the outcome.
pub struct PairingWait {
    pub id: u64,
    pub digits: String,
    pub machine: Machine,
    pub answer: oneshot::Receiver<PairResponse>,
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
        self.pairing_lock().codes.iter().any(|code| code.open(now))
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
        let mut pairings = self.pairing_lock();
        // A new code replaces the ones before it, which keep only their grace from now.
        pairings.codes.retain(|code| code.open(now));
        for code in pairings.codes.iter_mut() {
            code.expires_at_ms = code.expires_at_ms.min(now);
        }
        pairings.codes.push(Pairing {
            secret: secret.clone(),
            expires_at_ms,
            wrong_attempts: 0,
        });
        drop(pairings);
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
        let mut pairings = self.pairing_lock();
        pairings.codes.clear();
        let waiting = std::mem::take(&mut pairings.waiting);
        if !waiting.is_empty() {
            self.emit_waiting(&pairings);
        }
        for client in waiting {
            client.refuse("the device stopped pairing");
        }
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

    /// Lets `key` wait for its digits to be typed if it brought the secret of an open code.
    /// Too many wrong secrets use a code up. The same client asking again takes the place of
    /// its last try.
    pub fn request_pairing(
        &self,
        key: &EndpointId,
        request: &PairRequest,
    ) -> Result<PairingWait, PairResponse> {
        let mut pairings = self.pairing_lock();
        if pairings.codes.is_empty() {
            return Err(refused("this device isn't pairing right now"));
        }
        let now = now_ms();
        pairings.codes.retain(|code| code.open(now));
        if pairings.codes.is_empty() {
            return Err(refused("this code has expired"));
        }
        // Comparing with every code, so timing doesn't tell which one was close.
        let matched = pairings.codes.iter().fold(false, |found, code| {
            found | same_secret(&code.secret, &request.secret)
        });
        if !matched {
            for code in pairings.codes.iter_mut() {
                code.wrong_attempts += 1;
            }
            pairings
                .codes
                .retain(|code| code.wrong_attempts < PAIRING_ATTEMPTS);
            return Err(refused("this code is wrong"));
        }
        let key = key.to_string();
        if let Some(at) = pairings.waiting.iter().position(|w| w.key == key) {
            pairings
                .waiting
                .remove(at)
                .refuse("this device asked again");
        }
        if pairings.waiting.len() >= MAX_WAITING {
            return Err(refused(
                "too many devices are waiting to pair with this device",
            ));
        }
        let name: String = request.name.trim().chars().take(MAX_CLIENT_NAME).collect();
        let digits = new_digits(&pairings.waiting);
        let (answer, wait) = oneshot::channel();
        pairings.next_id += 1;
        let id = pairings.next_id;
        pairings.waiting.push(Waiting {
            id,
            key,
            name: if name.is_empty() {
                "Unnamed".to_owned()
            } else {
                name
            },
            digits: digits.clone(),
            answer,
        });
        self.emit_waiting(&pairings);
        Ok(PairingWait {
            id,
            digits,
            machine: self.lock().state.machine.clone(),
            answer: wait,
        })
    }

    /// Takes a client off the waiting list once it stops waiting, if it's still there.
    pub fn stop_waiting(&self, id: u64) {
        let mut pairings = self.pairing_lock();
        let before = pairings.waiting.len();
        pairings.waiting.retain(|w| w.id != id);
        if pairings.waiting.len() != before {
            self.emit_waiting(&pairings);
        }
    }

    /// Puts the waiting client that shows `digits` on the allowed list. Pairing one uses up
    /// every code and refuses the other clients waiting.
    pub(super) fn confirm_pairing(&self, digits: &str) -> Result<CommandOutput, CommandError> {
        let digits: String = digits.chars().filter(char::is_ascii_digit).collect();
        let mut pairings = self.pairing_lock();
        let Some(at) = pairings.waiting.iter().position(|w| w.digits == digits) else {
            return Err(invalid("no device waiting to pair shows those digits"));
        };
        let chosen = pairings.waiting.remove(at);
        // It went away a moment ago, and `stop_waiting` hasn't caught up yet.
        if chosen.answer.is_closed() {
            self.emit_waiting(&pairings);
            return Err(invalid("that device stopped waiting"));
        }
        pairings.codes.clear();
        let others = std::mem::take(&mut pairings.waiting);
        self.emit_waiting(&pairings);
        drop(pairings);
        for other in others {
            other.refuse("the device paired another one instead");
        }
        let client = PairedClient {
            key: chosen.key.clone(),
            name: chosen.name.clone(),
            paired_at_ms: now_ms(),
        };
        let mut inner = self.lock();
        if let Err(error) = inner.store.insert_paired_client(&client) {
            chosen.refuse("the device couldn't save the pairing");
            return Err(internal(error));
        }
        inner.emit(StateChange::ClientPaired {
            client: client.clone(),
        });
        let machine = inner.state.machine.clone();
        drop(inner);
        tracing::info!(name = %client.name, "paired a new client");
        let _ = chosen.answer.send(PairResponse::Paired { machine });
        Ok(CommandOutput::Paired { client })
    }

    fn emit_waiting(&self, pairings: &Pairings) {
        let requests = pairings
            .waiting
            .iter()
            .map(|w| PairingRequest {
                key: w.key.clone(),
                name: w.name.clone(),
            })
            .collect();
        self.lock()
            .emit(StateChange::PairingRequestsChanged { requests });
    }

    pub(super) fn pairing_lock(&self) -> std::sync::MutexGuard<'_, Pairings> {
        self.pairing
            .lock()
            .expect("a panic poisoned the pairing state")
    }
}

fn refused(reason: &str) -> PairResponse {
    PairResponse::Refused {
        reason: reason.to_owned(),
    }
}

/// Six random digits that no other waiting client shows, so typing them picks one client.
fn new_digits(waiting: &[Waiting]) -> String {
    loop {
        // The last six bytes of a v4 UUID are all random, and come from the OS's generator.
        let bytes = uuid::Uuid::new_v4().into_bytes();
        let random = bytes[10..]
            .iter()
            .fold(0u64, |n, byte| n << 8 | u64::from(*byte));
        let digits = format!("{:06}", random % 1_000_000);
        if !waiting.iter().any(|w| w.digits == digits) {
            return digits;
        }
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
