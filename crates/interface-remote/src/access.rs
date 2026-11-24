#[path = "trust.rs"]
mod trust;
use crate::{
    connection_crypto::constant_time_eq,
    replay::{Replay, StreamKey},
};
use interface_contracts::remote::{RemoteAccessStatus, RemoteDevice};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::Mutex,
    time::{Duration, Instant},
};
pub use trust::TrustPersistence;
use uuid::Uuid;

const DEVICE_TTL: Duration = Duration::from_secs(12 * 60 * 60);
const PAIR_TTL: Duration = Duration::from_secs(5 * 60);
const CONTROL_TTL: Duration = Duration::from_secs(30);
const MAX_COMMANDS: usize = 4096;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteError {
    pub code: &'static str,
    pub message: &'static str,
    pub replay_safe: bool,
}

impl RemoteError {
    #[must_use]
    pub const fn new(code: &'static str, message: &'static str, replay_safe: bool) -> Self {
        Self {
            code,
            message,
            replay_safe,
        }
    }
}

type RemoteResult<T> = Result<T, RemoteError>;
fn denied() -> RemoteError {
    RemoteError::new(
        "unauthorized",
        "Pair this phone from Interface on your Mac.",
        true,
    )
}
fn unavailable() -> RemoteError {
    RemoteError::new("unavailable", "Remote access is unavailable.", true)
}
fn digest(value: &[u8]) -> [u8; 32] {
    Sha256::digest(value).into()
}

struct Pairing {
    digest: [u8; 32],
    expires: Instant,
    remaining: u8,
}
struct Device {
    public: RemoteDevice,
    digest: [u8; 32],
    expires: Instant,
    commands: BTreeMap<Uuid, ([u8; 32], RemoteResult<()>)>,
}
struct Lease {
    id: Uuid,
    device: Uuid,
    expires: Instant,
}

#[derive(Default)]
struct State {
    trust: Option<trust::TrustState>,
    enabled: bool,
    local_origin: String,
    public_origin: Option<String>,
    pairing: Option<Pairing>,
    devices: BTreeMap<Uuid, Device>,
    leases: BTreeMap<StreamKey, Lease>,
    /// Phone Threads v1: a phone's control of one thread (`engine#sessionId`),
    /// held for the same 30 seconds as a terminal's.
    thread_leases: BTreeMap<String, Lease>,
}

/// A thread effect admitted by [`RemoteHub::begin_thread_effect`].
#[derive(Debug)]
pub enum ThreadEffect {
    /// This command id already ran: its recorded outcome, never a second run.
    Recorded(RemoteResult<()>),
    /// Reserved as outcome-unknown; the caller runs the effect once and
    /// settles the receipt with [`RemoteHub::settle_thread_effect`].
    Reserved(ThreadReceipt),
}

/// The receipt a reserved thread effect settles.
#[derive(Clone, Copy, Debug)]
pub struct ThreadReceipt {
    device: Uuid,
    command: Uuid,
    fingerprint: [u8; 32],
}

pub struct RemoteHub {
    state: Mutex<State>,
    pub replay: Replay,
}

impl Default for RemoteHub {
    /// Phone access starts off, so output is counted but not kept.
    fn default() -> Self {
        Self {
            state: Mutex::default(),
            replay: Replay::without_retention(),
        }
    }
}

impl RemoteHub {
    /// # Errors
    /// Refuses poisoned access state.
    pub fn activate(
        &self,
        local_origin: String,
        public_origin: Option<String>,
    ) -> RemoteResult<()> {
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        let trust = state.trust.take();
        *state = State {
            trust,
            enabled: true,
            local_origin,
            public_origin,
            ..State::default()
        };
        self.replay.set_retaining(true);
        Ok(())
    }

    pub fn disable(&self) {
        if let Ok(mut state) = self.state.lock() {
            let mut trust = state.trust.take();
            if let Some(trust) = &mut trust {
                trust.clear_pending();
            }
            *state = State {
                trust,
                ..State::default()
            };
        }
        self.replay.set_retaining(false);
        // Output stays bounded. Volatile grants end; explicitly remembered trust is retained.
    }

    /// Whether phone access is on, without building the whole status.
    ///
    /// # Errors
    /// Refuses unavailable access state.
    pub fn is_enabled(&self) -> RemoteResult<bool> {
        Ok(self.state.lock().map_err(|_| unavailable())?.enabled)
    }

    /// # Errors
    /// Refuses unavailable access state.
    pub fn status(&self) -> RemoteResult<RemoteAccessStatus> {
        let state = self.state.lock().map_err(|_| unavailable())?;
        Ok(RemoteAccessStatus {
            version: 1,
            enabled: state.enabled,
            local_url: state.enabled.then(|| state.local_origin.clone()),
            public_url: state.public_origin.clone(),
            devices: state
                .devices
                .values()
                .filter(|d| d.expires > Instant::now())
                .map(|d| d.public.clone())
                .collect(),
            pairing_code: None,
        })
    }

    /// # Errors
    /// Refuses a disabled listener or unavailable access state.
    pub fn pairing_code(&self) -> RemoteResult<String> {
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        if !state.enabled {
            return Err(unavailable());
        }
        let raw = Uuid::new_v4().simple().to_string()[..16].to_uppercase();
        let code = raw
            .as_bytes()
            .chunks(4)
            .map(|c| String::from_utf8_lossy(c))
            .collect::<Vec<_>>()
            .join("-");
        state.pairing = Some(Pairing {
            digest: digest(raw.as_bytes()),
            expires: Instant::now() + PAIR_TTL,
            remaining: 5,
        });
        Ok(code)
    }

    /// # Errors
    /// Refuses invalid, expired, exhausted or already-used codes and full device rosters.
    pub fn enroll(&self, code: &str, name: &str) -> RemoteResult<String> {
        if code.len() > 32
            || name.len() > 80
            || name.trim().is_empty()
            || name.chars().any(char::is_control)
        {
            return Err(denied());
        }
        let normalized = code.replace('-', "").to_uppercase();
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        let remembered = state
            .trust
            .as_ref()
            .map_or_else(Vec::new, trust::TrustState::ids);
        state
            .devices
            .retain(|id, d| d.expires > Instant::now() || remembered.contains(id));
        if !state.enabled || state.devices.len() >= 8 {
            return Err(denied());
        }
        let pair = state.pairing.as_mut().ok_or_else(denied)?;
        if pair.expires <= Instant::now() || pair.remaining == 0 {
            return Err(denied());
        }
        pair.remaining -= 1;
        if pair.digest != digest(normalized.as_bytes()) {
            return Err(denied());
        }
        state.pairing = None;
        let token = Uuid::new_v4().simple().to_string();
        let id = Uuid::new_v4();
        state.devices.insert(
            id,
            Device {
                public: RemoteDevice {
                    id: id.to_string(),
                    name: name.trim().to_owned(),
                },
                digest: digest(token.as_bytes()),
                expires: Instant::now() + DEVICE_TTL,
                commands: BTreeMap::new(),
            },
        );
        Ok(token)
    }

    /// # Errors
    /// Refuses invalid, expired or revoked device grants.
    pub fn authenticate(&self, token: &str) -> RemoteResult<Uuid> {
        if token.len() != 32 {
            return Err(denied());
        }
        let candidate = digest(token.as_bytes());
        let state = self.state.lock().map_err(|_| unavailable())?;
        if !state.enabled {
            return Err(denied());
        }
        state
            .devices
            .iter()
            .find_map(|(id, d)| {
                (constant_time_eq(&candidate, &d.digest) && d.expires > Instant::now())
                    .then_some(*id)
            })
            .ok_or_else(denied)
    }

    /// # Errors
    /// Refuses unavailable access state.
    pub fn revoke(&self, id: Uuid) -> RemoteResult<()> {
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        state.devices.remove(&id);
        state.leases.retain(|_, lease| lease.device != id);
        state.thread_leases.retain(|_, lease| lease.device != id);
        trust::forget_if_trusted(&mut state, id)?;
        Ok(())
    }

    /// Only the configured exact authorities can reach any HTTP route. Mutations
    /// and websocket upgrades also require a matching browser Origin.
    pub fn accepts(&self, host: &str, origin: Option<&str>, require_origin: bool) -> bool {
        let Ok(state) = self.state.lock() else {
            return false;
        };
        if !state.enabled {
            return false;
        }
        [&state.local_origin]
            .into_iter()
            .chain(state.public_origin.iter())
            .any(|allowed| {
                let authority = allowed
                    .strip_prefix("https://")
                    .or_else(|| allowed.strip_prefix("http://"));
                authority == Some(host)
                    && match origin {
                        Some(origin) => origin == allowed,
                        None => !require_origin,
                    }
            })
    }

    /// # Errors
    /// Refuses expired devices or a lease held by another phone.
    pub fn claim(&self, device: Uuid, key: StreamKey) -> RemoteResult<Uuid> {
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        validate_device(&state, device)?;
        state
            .leases
            .retain(|_, lease| lease.expires > Instant::now());
        if let Some(lease) = state.leases.get_mut(&key) {
            if lease.expires > Instant::now() {
                if lease.device != device {
                    return Err(RemoteError::new(
                        "control-busy",
                        "Another phone controls this terminal.",
                        true,
                    ));
                }
                lease.expires = Instant::now() + CONTROL_TTL;
                return Ok(lease.id);
            }
        }
        let id = Uuid::new_v4();
        state.leases.insert(
            key,
            Lease {
                id,
                device,
                expires: Instant::now() + CONTROL_TTL,
            },
        );
        Ok(id)
    }

    pub fn release(&self, device: Uuid, key: StreamKey) {
        if let Ok(mut state) = self.state.lock() {
            if state.leases.get(&key).is_some_and(|l| l.device == device) {
                state.leases.remove(&key);
            }
        }
    }

    /// One synchronous enqueue is serialized with desktop takeover. A receipt is
    /// reserved before dispatch and never evicted within a device grant. Retrying
    /// a command can only return its original receipt, never enqueue it again.
    /// # Errors
    /// Refuses stale authority, conflicting commands, full receipt ledgers or input admission failures. Unknown receipts never dispatch again.
    pub fn input(
        &self,
        device: Uuid,
        key: StreamKey,
        lease: Uuid,
        command: Uuid,
        bytes: &[u8],
        enqueue: impl FnOnce() -> RemoteResult<()>,
    ) -> RemoteResult<()> {
        if bytes.is_empty() || bytes.len() > 16 * 1024 {
            return Err(RemoteError::new(
                "input-size",
                "Send between 1 byte and 16 KiB.",
                true,
            ));
        }
        let mut hash = Sha256::new();
        hash.update(key.0.as_bytes());
        hash.update(key.1.to_le_bytes());
        hash.update(bytes);
        let fingerprint: [u8; 32] = hash.finalize().into();
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        validate_device(&state, device)?;
        let device_state = state.devices.get(&device).ok_or_else(denied)?;
        if let Some((previous, result)) = device_state.commands.get(&command) {
            return if previous == &fingerprint {
                result.clone()
            } else {
                Err(RemoteError::new(
                    "command-conflict",
                    "This command identifier was already used.",
                    false,
                ))
            };
        }
        if device_state.commands.len() >= MAX_COMMANDS {
            return Err(RemoteError::new(
                "grant-full",
                "Pair again to send more input.",
                true,
            ));
        }
        if !state
            .leases
            .get(&key)
            .is_some_and(|l| l.device == device && l.id == lease && l.expires > Instant::now())
        {
            return Err(RemoteError::new(
                "control-lost",
                "Control returned to the Mac. Take control again to type.",
                true,
            ));
        }
        let device_state = state.devices.get_mut(&device).ok_or_else(denied)?;
        device_state.commands.insert(
            command,
            (
                fingerprint,
                Err(RemoteError::new(
                    "outcome-unknown",
                    "Input may have been queued. It was not sent again.",
                    false,
                )),
            ),
        );
        let result = enqueue();
        device_state
            .commands
            .insert(command, (fingerprint, result.clone()));
        result
    }

    /// Phone Threads v1: the control lease a thread effect needs, as
    /// [`Self::claim`] is for terminal input.
    /// # Errors
    /// Refuses expired devices or a thread another phone controls.
    pub fn claim_thread(&self, device: Uuid, thread: &str) -> RemoteResult<Uuid> {
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        validate_device(&state, device)?;
        let now = Instant::now();
        state.thread_leases.retain(|_, lease| lease.expires > now);
        if let Some(lease) = state.thread_leases.get_mut(thread) {
            if lease.device != device {
                return Err(RemoteError::new(
                    "control-busy",
                    "Another phone is answering this thread.",
                    true,
                ));
            }
            lease.expires = now + CONTROL_TTL;
            return Ok(lease.id);
        }
        let id = Uuid::new_v4();
        state.thread_leases.insert(
            thread.to_owned(),
            Lease {
                id,
                device,
                expires: now + CONTROL_TTL,
            },
        );
        Ok(id)
    }

    pub fn release_thread(&self, device: Uuid, thread: &str) {
        if let Ok(mut state) = self.state.lock() {
            if state
                .thread_leases
                .get(thread)
                .is_some_and(|l| l.device == device)
            {
                state.thread_leases.remove(thread);
            }
        }
    }

    /// Admits one thread effect under the rules terminal input follows: the
    /// command id's receipt first (a repeat answers what it recorded, a
    /// different request under it conflicts), then the device's ledger
    /// bound, then this phone's current lease. A fresh command is recorded as
    /// outcome-unknown before the effect starts, so a reply lost anywhere
    /// after this never runs it again.
    /// # Errors
    /// Refuses expired devices, reused command ids, a full ledger and a lost lease.
    pub fn begin_thread_effect(
        &self,
        device: Uuid,
        thread: &str,
        lease: Uuid,
        command: Uuid,
        request: &[u8],
    ) -> RemoteResult<ThreadEffect> {
        let mut hash = Sha256::new();
        hash.update(b"thread\0");
        hash.update(thread.as_bytes());
        hash.update(b"\0");
        hash.update(request);
        let fingerprint: [u8; 32] = hash.finalize().into();
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        validate_device(&state, device)?;
        let device_state = state.devices.get(&device).ok_or_else(denied)?;
        if let Some((previous, result)) = device_state.commands.get(&command) {
            return if previous == &fingerprint {
                Ok(ThreadEffect::Recorded(result.clone()))
            } else {
                Err(RemoteError::new(
                    "command-conflict",
                    "This command identifier was already used.",
                    false,
                ))
            };
        }
        if device_state.commands.len() >= MAX_COMMANDS {
            return Err(RemoteError::new(
                "grant-full",
                "Pair again to send more input.",
                true,
            ));
        }
        if !state
            .thread_leases
            .get(thread)
            .is_some_and(|l| l.device == device && l.id == lease && l.expires > Instant::now())
        {
            return Err(RemoteError::new(
                "control-lost",
                "This thread's control lapsed. Try again.",
                true,
            ));
        }
        let device_state = state.devices.get_mut(&device).ok_or_else(denied)?;
        device_state.commands.insert(
            command,
            (
                fingerprint,
                Err(RemoteError::new(
                    "outcome-unknown",
                    "Your answer may have reached the agent. Check on your Mac before answering again.",
                    false,
                )),
            ),
        );
        Ok(ThreadEffect::Reserved(ThreadReceipt {
            device,
            command,
            fingerprint,
        }))
    }

    /// Records what a reserved thread effect did. A grant that ended since
    /// keeps nothing: its ledger went with it.
    pub fn settle_thread_effect(&self, receipt: ThreadReceipt, result: RemoteResult<()>) {
        if let Ok(mut state) = self.state.lock() {
            if let Some(device) = state.devices.get_mut(&receipt.device) {
                device
                    .commands
                    .insert(receipt.command, (receipt.fingerprint, result));
            }
        }
    }

    pub fn desktop_input<T>(&self, key: StreamKey, enqueue: impl FnOnce() -> T) -> T {
        let mut state = self.state.lock().ok();
        if let Some(state) = state.as_mut() {
            state.leases.remove(&key);
        }
        self.replay.invalidate_approval(key);
        enqueue()
    }
}

fn validate_device(state: &State, id: Uuid) -> RemoteResult<()> {
    if state.enabled
        && state
            .devices
            .get(&id)
            .is_some_and(|d| d.expires > Instant::now())
    {
        Ok(())
    } else {
        Err(denied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn output_is_kept_only_while_phone_access_is_on_and_always_counted() {
        let hub = RemoteHub::default();
        let key = (Uuid::new_v4(), 1);
        hub.replay.append(key, b"off");
        assert!(hub.replay.read(key, 0).unwrap().bytes.is_empty());
        hub.activate("http://127.0.0.1:4321".into(), None).unwrap();
        hub.replay.append(key, b"on");
        assert_eq!(hub.replay.read(key, 3).unwrap().bytes, b"on");
        hub.disable();
        assert!(hub.replay.read(key, 3).unwrap().bytes.is_empty());
        // Approvals and the voice assistant still compare against the count.
        assert!(hub.replay.approval_current(key, 5));
    }

    #[test]
    fn expired_pairings_grants_and_leases_never_authorize_input() {
        let hub = RemoteHub::default();
        hub.activate("http://127.0.0.1:4321".into(), None).unwrap();
        let code = hub.pairing_code().unwrap();
        hub.state.lock().unwrap().pairing.as_mut().unwrap().expires = Instant::now();
        assert!(hub.enroll(&code, "iPhone").is_err());
        let code = hub.pairing_code().unwrap();
        for _ in 0..5 {
            assert!(hub.enroll("wrong", "iPhone").is_err());
        }
        assert!(hub.enroll(&code, "iPhone").is_err());
        let token = hub.enroll(&hub.pairing_code().unwrap(), "iPhone").unwrap();
        let id = hub.authenticate(&token).unwrap();
        let key = (Uuid::new_v4(), 1);
        let lease = hub.claim(id, key).unwrap();
        hub.state
            .lock()
            .unwrap()
            .leases
            .get_mut(&key)
            .unwrap()
            .expires = Instant::now();
        assert!(
            hub.input(id, key, lease, Uuid::new_v4(), b"x", || panic!(
                "expired lease"
            ))
            .is_err()
        );
        hub.state
            .lock()
            .unwrap()
            .devices
            .get_mut(&id)
            .unwrap()
            .expires = Instant::now();
        assert!(hub.authenticate(&token).is_err());
        assert!(hub.claim(id, key).is_err());
    }

    #[test]
    fn unknown_receipts_and_a_full_ledger_never_replay_or_evict_commands() {
        let hub = RemoteHub::default();
        hub.activate("http://127.0.0.1:4321".into(), None).unwrap();
        let token = hub.enroll(&hub.pairing_code().unwrap(), "iPhone").unwrap();
        let id = hub.authenticate(&token).unwrap();
        let key = (Uuid::new_v4(), 1);
        let lease = hub.claim(id, key).unwrap();
        let command = Uuid::new_v4();
        let result = hub.input(id, key, lease, command, b"x", || {
            Err(RemoteError::new("unknown", "Unknown", false))
        });
        assert!(!result.unwrap_err().replay_safe);
        assert!(
            !hub.input(id, key, lease, command, b"x", || panic!("replayed unknown"))
                .unwrap_err()
                .replay_safe
        );
        for _ in 1..MAX_COMMANDS {
            hub.input(id, key, lease, Uuid::new_v4(), b"x", || Ok(()))
                .unwrap();
        }
        assert_eq!(
            hub.input(id, key, lease, Uuid::new_v4(), b"x", || panic!(
                "full ledger"
            ))
            .unwrap_err()
            .code,
            "grant-full"
        );
        hub.disable();
        assert!(hub.authenticate(&token).is_err());
    }

    #[test]
    fn a_thread_answer_runs_once_under_its_lease_and_its_receipt_answers_repeats() {
        let hub = RemoteHub::default();
        hub.activate("http://127.0.0.1:4321".into(), None).unwrap();
        let token = hub.enroll(&hub.pairing_code().unwrap(), "iPhone").unwrap();
        let id = hub.authenticate(&token).unwrap();
        let thread = "claude#thread";
        let begin = |lease, command, request: &[u8]| {
            hub.begin_thread_effect(id, thread, lease, command, request)
        };
        // Without this phone's lease nothing is reserved.
        assert_eq!(
            begin(Uuid::new_v4(), Uuid::new_v4(), b"allow")
                .unwrap_err()
                .code,
            "control-lost"
        );
        let lease = hub.claim_thread(id, thread).unwrap();
        assert_eq!(hub.claim_thread(id, thread).unwrap(), lease);
        let other_token = hub.enroll(&hub.pairing_code().unwrap(), "iPad").unwrap();
        let other = hub.authenticate(&other_token).unwrap();
        assert_eq!(
            hub.claim_thread(other, thread).unwrap_err().code,
            "control-busy"
        );
        hub.release_thread(other, thread);
        let command = Uuid::new_v4();
        let ThreadEffect::Reserved(receipt) = begin(lease, command, b"allow").unwrap() else {
            panic!("a fresh command is reserved");
        };
        // While the effect runs, a repeat learns only that it may have happened.
        match begin(lease, command, b"allow").unwrap() {
            ThreadEffect::Recorded(Err(error)) => {
                assert_eq!(error.code, "outcome-unknown");
                assert!(!error.replay_safe);
            }
            other => panic!("an unsettled command ran again: {other:?}"),
        }
        hub.settle_thread_effect(receipt, Ok(()));
        assert!(matches!(
            begin(lease, command, b"allow").unwrap(),
            ThreadEffect::Recorded(Ok(()))
        ));
        assert_eq!(
            begin(lease, command, b"deny").unwrap_err().code,
            "command-conflict"
        );
        // A lease answers for its own thread only, and ends with the grant.
        assert_eq!(
            hub.begin_thread_effect(id, "claude#other", lease, Uuid::new_v4(), b"x")
                .unwrap_err()
                .code,
            "control-lost"
        );
        hub.lock_session(&token).unwrap();
        assert_eq!(
            begin(lease, Uuid::new_v4(), b"allow").unwrap_err().code,
            "unauthorized"
        );
        assert!(hub.claim_thread(other, thread).is_ok());
    }

    #[test]
    fn pairing_revocation_origin_and_command_delivery_are_fenced() {
        let hub = RemoteHub::default();
        hub.activate(
            "http://127.0.0.1:4321".into(),
            Some("https://mac.example.ts.net".into()),
        )
        .unwrap();
        assert!(!hub.accepts("evil.test", Some("https://evil.test"), true));
        assert!(!hub.accepts("mac.example.ts.net", Some("https://evil.test"), true));
        assert!(hub.accepts(
            "mac.example.ts.net",
            Some("https://mac.example.ts.net"),
            true
        ));
        let code = hub.pairing_code().unwrap();
        let token = hub.enroll(&code, "iPhone").unwrap();
        assert!(hub.enroll(&code, "again").is_err());
        let device = hub.authenticate(&token).unwrap();
        let key = (Uuid::new_v4(), 1);
        let lease = hub.claim(device, key).unwrap();
        let command = Uuid::new_v4();
        hub.input(device, key, lease, command, b"echo once\r", || Ok(()))
            .unwrap();
        hub.input(device, key, lease, command, b"echo once\r", || {
            panic!("duplicate input")
        })
        .unwrap();
        assert!(
            hub.input(device, key, lease, command, b"different", || panic!())
                .is_err()
        );
        hub.desktop_input(key, || ());
        assert!(
            hub.input(device, key, lease, Uuid::new_v4(), b"late", || panic!())
                .is_err()
        );
        hub.revoke(device).unwrap();
        assert!(hub.authenticate(&token).is_err());
    }
}
