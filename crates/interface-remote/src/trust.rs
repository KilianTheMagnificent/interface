use super::{Device, RemoteError, RemoteHub, RemoteResult, State, denied, digest, unavailable};
use crate::connection_crypto::constant_time_eq;
use interface_contracts::{
    remote::RemoteDevice,
    remote_trust::{
        EnrollmentRequest, EnrollmentStatus, PendingDevice, RememberedListener, RemoteTrustData,
        TrustCommand, TrustStatus, TrustedDeviceRecord, UnlockRequest,
    },
};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};
use uuid::Uuid;

pub trait TrustPersistence: Send + Sync {
    /// # Errors
    /// Refuses invalid or unavailable persisted trust.
    fn load(&self) -> RemoteResult<RemoteTrustData>;
    /// # Errors
    /// Refuses a failed or uncertain atomic write.
    fn save(&self, value: &RemoteTrustData) -> RemoteResult<()>;
}
struct Pending {
    verification: String,
    record: TrustedDeviceRecord,
    expires: Instant,
}
pub(super) struct TrustState {
    store: Arc<dyn TrustPersistence>,
    data: RemoteTrustData,
    pending: BTreeMap<Uuid, Pending>,
    failed: bool,
}
fn verification(id: Uuid) -> String {
    id.simple().to_string()[..8].to_uppercase()
}
fn validate_secret(version: u8, id: &str, secret: &str) -> RemoteResult<Uuid> {
    if version != 3 || secret.len() != 64 || !secret.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(denied());
    }
    let parsed = Uuid::parse_str(id).map_err(|_| denied())?;
    if parsed.to_string() != id {
        return Err(denied());
    }
    Ok(parsed)
}
fn trusted(state: &mut State) -> RemoteResult<&mut TrustState> {
    let trust = state
        .trust
        .as_mut()
        .filter(|t| !t.failed)
        .ok_or_else(unavailable)?;
    trust.pending.retain(|_, p| p.expires > Instant::now());
    Ok(trust)
}
fn save(state: &mut State, data: RemoteTrustData) -> RemoteResult<()> {
    let trust = trusted(state)?;
    if trust.store.save(&data).is_err() {
        // An uncertain write may already be on disk. Never keep serving stale grants.
        trust.failed = true;
        state.enabled = false;
        state.devices.clear();
        state.leases.clear();
        state.thread_leases.clear();
        return Err(RemoteError::new(
            "trust-write-failed",
            "Device settings could not be saved. Remote access stopped; restart Interface before trying again.",
            false,
        ));
    }
    trust.data = data;
    Ok(())
}
impl RemoteHub {
    /// # Errors
    /// Refuses unsafe or unsupported trust storage before serving any requests.
    pub fn configure_trust(&self, store: Arc<dyn TrustPersistence>) -> RemoteResult<()> {
        let data = store.load()?;
        if !data.valid() {
            return Err(unavailable());
        }
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        if state.enabled || state.trust.is_some() {
            return Err(unavailable());
        }
        state.trust = Some(TrustState {
            store,
            data,
            pending: BTreeMap::new(),
            failed: false,
        });
        Ok(())
    }
    /// # Errors
    /// Refuses unavailable trust storage.
    pub fn remembered_listener(&self) -> RemoteResult<Option<RememberedListener>> {
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        if state.trust.is_none() {
            return Ok(None);
        }
        Ok(trusted(&mut state)?.data.listener.clone())
    }
    /// # Errors
    /// Refuses unproven durability. Legacy embedders without persistence remain ephemeral.
    pub fn remember_listener(&self, listener: RememberedListener) -> RemoteResult<()> {
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        if state.trust.is_none() {
            return Ok(());
        }
        let data = &trusted(&mut state)?.data;
        if data.listener.as_ref().is_some_and(|saved| {
            saved.enabled == listener.enabled
                && saved.port == listener.port
                && saved.origin == listener.origin
        }) {
            return Ok(());
        }
        let mut data = data.clone();
        data.listener = Some(listener);
        save(&mut state, data)
    }
    /// # Errors
    /// Refuses unsupported administration and failed persistence.
    pub fn manage_trust(&self, version: u8, command: TrustCommand) -> RemoteResult<TrustStatus> {
        if version != 3 {
            return Err(unavailable());
        }
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        match command {
            TrustCommand::Status {} => {}
            TrustCommand::Approve { id } => {
                if !state.enabled {
                    return Err(unavailable());
                }
                let id = Uuid::parse_str(&id).map_err(|_| denied())?;
                let trust = trusted(&mut state)?;
                let pending = trust.pending.get(&id).ok_or_else(denied)?;
                if trust.data.devices.len() >= 8 {
                    return Err(unavailable());
                }
                let mut data = trust.data.clone();
                data.devices.push(pending.record.clone());
                save(&mut state, data)?;
                trusted(&mut state)?.pending.remove(&id);
            }
            TrustCommand::Reject { id } => {
                let id = Uuid::parse_str(&id).map_err(|_| denied())?;
                trusted(&mut state)?.pending.remove(&id);
            }
            TrustCommand::Forget { id } => {
                let device = Uuid::parse_str(&id).map_err(|_| denied())?;
                let mut data = trusted(&mut state)?.data.clone();
                data.devices.retain(|d| d.id != id);
                // Revoke volatile authority even if the following save fails.
                state.devices.remove(&device);
                state.leases.retain(|_, l| l.device != device);
                state.thread_leases.retain(|_, l| l.device != device);
                save(&mut state, data)?;
            }
        }
        let trust = trusted(&mut state)?;
        Ok(TrustStatus {
            version: 3,
            pending: trust
                .pending
                .iter()
                .map(|(id, p)| PendingDevice {
                    id: id.to_string(),
                    name: p.record.name.clone(),
                    verification: p.verification.clone(),
                })
                .collect(),
            devices: trust
                .data
                .devices
                .iter()
                .map(|d| RemoteDevice {
                    id: d.id.clone(),
                    name: d.name.clone(),
                })
                .collect(),
        })
    }
    /// # Errors
    /// Bounds unauthenticated requests and requires Mac approval before granting anything.
    pub fn request_enrollment(
        &self,
        request: &EnrollmentRequest,
    ) -> RemoteResult<EnrollmentStatus> {
        let id = validate_secret(request.version, &request.id, &request.secret)?;
        if request.name.trim().is_empty()
            || request.name.len() > 80
            || request.name.chars().any(char::is_control)
        {
            return Err(denied());
        }
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        if !state.enabled {
            return Err(denied());
        }
        let trust = trusted(&mut state)?;
        let candidate = digest(request.secret.as_bytes());
        if let Some(d) = trust.data.devices.iter().find(|d| d.id == request.id) {
            if !constant_time_eq(&d.digest, &candidate) {
                return Err(denied());
            }
            return Ok(EnrollmentStatus {
                version: 3,
                approved: true,
                verification: String::new(),
            });
        }
        if let Some(p) = trust.pending.get(&id) {
            if !constant_time_eq(&p.record.digest, &candidate) {
                return Err(denied());
            }
        } else {
            if trust.pending.len() >= 8 || trust.data.devices.len() >= 8 {
                return Err(RemoteError::new(
                    "pairing-full",
                    "The pairing list is full. Review devices on your Mac, then try again.",
                    true,
                ));
            }
            trust.pending.insert(
                id,
                Pending {
                    verification: verification(Uuid::new_v4()),
                    record: TrustedDeviceRecord {
                        id: id.to_string(),
                        name: request.name.trim().to_owned(),
                        digest: candidate,
                    },
                    expires: Instant::now() + Duration::from_secs(300),
                },
            );
        }
        Ok(EnrollmentStatus {
            version: 3,
            approved: false,
            verification: trust
                .pending
                .get(&id)
                .ok_or_else(denied)?
                .verification
                .clone(),
        })
    }
    /// # Errors
    /// Polling never creates or extends a pending approval.
    pub fn enrollment_status(&self, request: &UnlockRequest) -> RemoteResult<EnrollmentStatus> {
        let id = validate_secret(request.version, &request.id, &request.secret)?;
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        if !state.enabled {
            return Err(denied());
        }
        let trust = trusted(&mut state)?;
        let candidate = digest(request.secret.as_bytes());
        let approved = trust
            .data
            .devices
            .iter()
            .any(|d| d.id == request.id && constant_time_eq(&d.digest, &candidate));
        if !approved
            && !trust
                .pending
                .get(&id)
                .is_some_and(|p| constant_time_eq(&p.record.digest, &candidate))
        {
            return Err(denied());
        }
        Ok(EnrollmentStatus {
            version: 3,
            approved,
            verification: trust
                .pending
                .get(&id)
                .map_or_else(String::new, |p| p.verification.clone()),
        })
    }
    /// Exchanges Keychain-held trust for one short-lived, revocable session.
    /// # Errors
    /// Refuses disabled, revoked, malformed or unapproved credentials.
    pub fn unlock(&self, request: &UnlockRequest) -> RemoteResult<String> {
        let id = validate_secret(request.version, &request.id, &request.secret)?;
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        if !state.enabled {
            return Err(denied());
        }
        let candidate = digest(request.secret.as_bytes());
        let record = trusted(&mut state)?
            .data
            .devices
            .iter()
            .find(|d| d.id == request.id && constant_time_eq(&d.digest, &candidate))
            .cloned()
            .ok_or_else(denied)?;
        let token = Uuid::new_v4().simple().to_string();
        let expires = Instant::now() + Duration::from_secs(900);
        if let Some(device) = state.devices.get_mut(&id) {
            // Preserve receipts on renewal; rotating a cookie cannot replay an input.
            device.digest = digest(token.as_bytes());
            device.expires = expires;
        } else {
            state.devices.insert(
                id,
                Device {
                    public: RemoteDevice {
                        id: record.id,
                        name: record.name,
                    },
                    digest: digest(token.as_bytes()),
                    expires,
                    commands: BTreeMap::new(),
                },
            );
        }
        state.leases.retain(|_, l| l.device != id);
        state.thread_leases.retain(|_, l| l.device != id);
        Ok(token)
    }
    /// # Errors
    /// An older cookie cannot revoke a newer unlock, even across concurrent requests.
    pub fn lock_session(&self, token: &str) -> RemoteResult<()> {
        if token.len() != 32 {
            return Err(denied());
        }
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        if !state.enabled {
            return Err(denied());
        }
        let candidate = digest(token.as_bytes());
        let id = state
            .devices
            .iter()
            .find_map(|(id, d)| {
                (d.expires > Instant::now() && constant_time_eq(&candidate, &d.digest))
                    .then_some(*id)
            })
            .ok_or_else(denied)?;
        if let Some(device) = state.devices.get_mut(&id) {
            device.expires = Instant::now();
        }
        state.leases.retain(|_, l| l.device != id);
        state.thread_leases.retain(|_, l| l.device != id);
        Ok(())
    }
}

impl TrustState {
    pub(super) fn clear_pending(&mut self) {
        self.pending.clear();
    }
}

impl TrustState {
    pub(super) fn ids(&self) -> Vec<Uuid> {
        self.data
            .devices
            .iter()
            .filter_map(|d| Uuid::parse_str(&d.id).ok())
            .collect()
    }
}
pub(super) fn forget_if_trusted(state: &mut State, id: Uuid) -> RemoteResult<()> {
    if let Some(trust) = &state.trust {
        if trust.data.devices.iter().any(|d| d.id == id.to_string()) {
            let mut data = trust.data.clone();
            data.devices.retain(|d| d.id != id.to_string());
            save(state, data)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    };
    #[derive(Default)]
    struct Store {
        data: Mutex<RemoteTrustData>,
        fail: AtomicBool,
    }
    impl TrustPersistence for Store {
        fn load(&self) -> RemoteResult<RemoteTrustData> {
            Ok(self.data.lock().unwrap().clone())
        }
        fn save(&self, data: &RemoteTrustData) -> RemoteResult<()> {
            if self.fail.load(Ordering::SeqCst) {
                return Err(unavailable());
            }
            *self.data.lock().unwrap() = data.clone();
            Ok(())
        }
    }
    fn hub(store: Arc<Store>) -> RemoteHub {
        let hub = RemoteHub::default();
        hub.configure_trust(store).unwrap();
        hub.activate(
            "http://127.0.0.1:4321".into(),
            Some("https://mac.example.ts.net".into()),
        )
        .unwrap();
        hub
    }
    fn request() -> EnrollmentRequest {
        EnrollmentRequest {
            version: 3,
            id: Uuid::new_v4().to_string(),
            name: "Phone".into(),
            secret: "ab".repeat(32),
        }
    }
    fn unlock(r: &EnrollmentRequest) -> UnlockRequest {
        UnlockRequest {
            version: 3,
            id: r.id.clone(),
            secret: r.secret.clone(),
        }
    }
    #[test]
    fn only_mac_approval_grants_trust_and_restart_preserves_digests_not_sessions() {
        let store = Arc::new(Store::default());
        let hub = hub(store.clone());
        let request = request();
        assert!(!hub.request_enrollment(&request).unwrap().approved);
        assert!(hub.unlock(&unlock(&request)).is_err());
        let status = hub.manage_trust(3, TrustCommand::Status {}).unwrap();
        assert_eq!(status.pending.len(), 1);
        assert!(status.devices.is_empty());
        let serialized = serde_json::to_string(&status).unwrap();
        assert!(!serialized.contains(&request.secret));
        assert!(!serialized.contains("digest"));
        hub.manage_trust(
            3,
            TrustCommand::Approve {
                id: request.id.clone(),
            },
        )
        .unwrap();
        assert!(hub.enrollment_status(&unlock(&request)).unwrap().approved);
        let cookie = hub.unlock(&unlock(&request)).unwrap();
        assert!(hub.authenticate(&cookie).is_ok());
        assert!(
            !serde_json::to_string(&*store.data.lock().unwrap())
                .unwrap()
                .contains(&request.secret)
        );
        hub.disable();
        assert!(hub.authenticate(&cookie).is_err());
        let restarted = self::hub(store.clone());
        assert!(restarted.authenticate(&cookie).is_err());
        let renewed = restarted.unlock(&unlock(&request)).unwrap();
        assert!(restarted.authenticate(&renewed).is_ok());
        restarted
            .revoke(Uuid::parse_str(&request.id).unwrap())
            .unwrap();
        assert!(restarted.authenticate(&renewed).is_err());
        assert!(restarted.unlock(&unlock(&request)).is_err());
        assert!(self::hub(store).unlock(&unlock(&request)).is_err());
    }
    #[test]
    fn locking_and_cookie_rotation_preserve_unknown_input_receipts() {
        let hub = hub(Arc::new(Store::default()));
        let request = request();
        hub.request_enrollment(&request).unwrap();
        hub.manage_trust(
            3,
            TrustCommand::Approve {
                id: request.id.clone(),
            },
        )
        .unwrap();
        let cookie = hub.unlock(&unlock(&request)).unwrap();
        let device = hub.authenticate(&cookie).unwrap();
        let key = (Uuid::new_v4(), 1);
        let lease = hub.claim(device, key).unwrap();
        let command = Uuid::new_v4();
        assert!(
            hub.input(device, key, lease, command, b"x", || Err(RemoteError::new(
                "unknown", "Unknown", false
            )))
            .is_err()
        );
        hub.lock_session(&cookie).unwrap();
        assert!(hub.authenticate(&cookie).is_err());
        assert!(hub.claim(device, key).is_err());
        let renewed = hub.unlock(&unlock(&request)).unwrap();
        assert!(hub.authenticate(&cookie).is_err());
        assert!(hub.lock_session(&cookie).is_err());
        assert_eq!(hub.authenticate(&renewed).unwrap(), device);
        let lease = hub.claim(device, key).unwrap();
        assert!(
            !hub.input(device, key, lease, command, b"x", || panic!(
                "replayed after Face ID"
            ))
            .unwrap_err()
            .replay_safe
        );
    }
    #[test]
    fn pending_requests_are_bounded_expire_and_do_not_allow_id_substitution() {
        let hub = hub(Arc::new(Store::default()));
        let mut request = request();
        hub.request_enrollment(&request).unwrap();
        let original = request.secret.clone();
        request.secret = "cd".repeat(32);
        assert!(hub.request_enrollment(&request).is_err());
        assert!(hub.enrollment_status(&unlock(&request)).is_err());
        request.secret = original;
        for _ in 1..8 {
            hub.request_enrollment(&self::request()).unwrap();
        }
        assert!(hub.request_enrollment(&self::request()).is_err());
        for pending in hub
            .state
            .lock()
            .unwrap()
            .trust
            .as_mut()
            .unwrap()
            .pending
            .values_mut()
        {
            pending.expires = Instant::now();
        }
        assert!(hub.enrollment_status(&unlock(&request)).is_err());
        assert!(
            hub.manage_trust(
                3,
                TrustCommand::Approve {
                    id: request.id.clone()
                }
            )
            .is_err()
        );
        hub.request_enrollment(&request).unwrap();
        hub.manage_trust(
            3,
            TrustCommand::Reject {
                id: request.id.clone(),
            },
        )
        .unwrap();
        assert!(hub.enrollment_status(&unlock(&request)).is_err());
        request.version = 4;
        assert!(hub.request_enrollment(&request).is_err());
    }
    #[test]
    fn failed_save_stops_existing_sessions_and_never_acknowledges_approval() {
        let store = Arc::new(Store::default());
        let hub = hub(store.clone());
        let request = request();
        let browser = hub.enroll(&hub.pairing_code().unwrap(), "Safari").unwrap();
        hub.request_enrollment(&request).unwrap();
        store.fail.store(true, Ordering::SeqCst);
        assert!(
            !hub.manage_trust(
                3,
                TrustCommand::Approve {
                    id: request.id.clone()
                }
            )
            .unwrap_err()
            .replay_safe
        );
        assert!(hub.authenticate(&browser).is_err());
        assert!(hub.unlock(&unlock(&request)).is_err());
        assert!(hub.manage_trust(3, TrustCommand::Status {}).is_err());
    }
}
