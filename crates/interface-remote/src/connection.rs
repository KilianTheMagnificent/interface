//! Admission shared by nearby and relay transports. No network effects live here.
use std::collections::BTreeMap;
use url::Url;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionError {
    InvalidInvitation,
    InvalidOrigin,
    Replay,
    Expired,
    Capacity,
}

/// A relay is a fixed HTTPS origin, never a supplied request path or proxy target.
/// # Errors
/// Refuses credentials, insecure schemes, paths, queries, fragments and non-DNS hosts.
pub fn relay_origin(value: &str) -> Result<Url, ConnectionError> {
    let url = Url::parse(value).map_err(|_| ConnectionError::InvalidOrigin)?;
    let host = url.host_str().ok_or(ConnectionError::InvalidOrigin)?;
    if value.len() > 253
        || !value.is_ascii()
        || url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
        || url.port_or_known_default() != Some(443)
        || !host.contains('.')
        || host.ends_with('.')
        || host.parse::<std::net::IpAddr>().is_ok()
        || host.split('.').any(|label| {
            label.is_empty()
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        || value != format!("https://{host}")
    {
        return Err(ConnectionError::InvalidOrigin);
    }
    Ok(url)
}

/// IDs are retained until their complete acceptance interval has passed. Capacity
/// failure cannot evict an unexpired ID and make a write replayable.
pub struct RequestWindow {
    admitted: BTreeMap<Uuid, u64>,
    capacity: usize,
    latest_time: u64,
}
impl RequestWindow {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            admitted: BTreeMap::new(),
            capacity,
            latest_time: 0,
        }
    }
    /// Validates lifetime, monotonic clock, and non-replay without consuming a
    /// slot in the bounded window (used for idempotent read-only `/api/v5/ping`
    /// health checks so frequent pings cannot exhaust capacity for writes).
    /// # Errors
    /// Rejects duplicate IDs already in the window, invalid lifetimes, and clock rollback.
    pub fn check_freshness(
        &mut self,
        id: Uuid,
        issued_at: u64,
        expires_at: u64,
        now: u64,
    ) -> Result<(), ConnectionError> {
        if now < self.latest_time
            || issued_at > now
            || expires_at <= now
            || expires_at <= issued_at
            || expires_at.saturating_sub(issued_at) > 90
        {
            return Err(ConnectionError::Expired);
        }
        self.latest_time = now;
        self.admitted.retain(|_, expiry| *expiry > now);
        if self.admitted.contains_key(&id) {
            return Err(ConnectionError::Replay);
        }
        Ok(())
    }

    /// Call only after successful authenticated decryption, before dispatching effects.
    /// # Errors
    /// Rejects duplicate IDs, invalid lifetimes, clock rollback and a full window.
    pub fn admit(
        &mut self,
        id: Uuid,
        issued_at: u64,
        expires_at: u64,
        now: u64,
    ) -> Result<(), ConnectionError> {
        if now < self.latest_time
            || issued_at > now
            || expires_at <= now
            || expires_at <= issued_at
            || expires_at.saturating_sub(issued_at) > 90
        {
            return Err(ConnectionError::Expired);
        }
        self.latest_time = now;
        self.admitted.retain(|_, expiry| *expiry > now);
        if self.admitted.contains_key(&id) {
            return Err(ConnectionError::Replay);
        }
        if self.admitted.len() >= self.capacity {
            return Err(ConnectionError::Capacity);
        }
        self.admitted.insert(id, expires_at);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use interface_contracts::remote_connection::{
        MAX_INVITATION_BYTES, PAIRING_PREFIX, PairingInvitationV5,
    };
    use sha2::{Digest, Sha256};
    use std::fmt::Write;

    // A Rust mirror of the phone scanner (PairingInvitation.swift). Only this
    // cross-language oracle uses it: it pins that the shared v5 fixture is
    // canonical JSON and that macId == sha256(publicKey).
    fn public_identity(public_key: &str) -> Result<String, ConnectionError> {
        let bytes = public_key
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let text =
                    std::str::from_utf8(pair).map_err(|_| ConnectionError::InvalidInvitation)?;
                u8::from_str_radix(text, 16).map_err(|_| ConnectionError::InvalidInvitation)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Sha256::digest(&bytes)
            .iter()
            .fold(String::with_capacity(64), |mut out, byte| {
                let _ = write!(out, "{byte:02x}");
                out
            }))
    }

    /// Only accepts the bounded v5 payload emitted by the Mac, with matching identity.
    fn parse_invitation(text: &str, now: u64) -> Result<PairingInvitationV5, ConnectionError> {
        if text.len() > MAX_INVITATION_BYTES {
            return Err(ConnectionError::InvalidInvitation);
        }
        let json = text
            .strip_prefix(PAIRING_PREFIX)
            .ok_or(ConnectionError::InvalidInvitation)?;
        let value: PairingInvitationV5 =
            serde_json::from_str(json).map_err(|_| ConnectionError::InvalidInvitation)?;
        let canonical = serde_json::to_value(&value)
            .and_then(|v| serde_json::to_string(&v))
            .map_err(|_| ConnectionError::InvalidInvitation)?;
        if json != canonical {
            return Err(ConnectionError::InvalidInvitation);
        }
        value
            .validate_at(now)
            .map_err(|_| ConnectionError::InvalidInvitation)?;
        if public_identity(&value.public_key)? != value.mac_id {
            return Err(ConnectionError::InvalidInvitation);
        }
        if let Some(origin) = &value.relay_origin {
            relay_origin(origin)?;
        }
        Ok(value)
    }

    #[test]
    fn scanner_and_rust_share_identity_and_origin_admission() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/remote-connection-v5.json"
        ))
        .unwrap();
        let now = fixture["invitation"]["issuedAt"].as_u64().unwrap();
        let qr = fixture["qr"].as_str().unwrap();
        let invitation = parse_invitation(qr, now).unwrap();
        assert_eq!(
            serde_json::to_value(invitation).unwrap(),
            fixture["invitation"]
        );
        assert!(parse_invitation(qr, now + 300).is_err());
        assert!(parse_invitation(&qr.replacen("Studio Mac", "Other Mac\n", 1), now).is_err());
        let mut altered = fixture["invitation"].clone();
        altered["macId"] = "ab".repeat(32).into();
        assert!(parse_invitation(&format!("{PAIRING_PREFIX}{altered}"), now).is_err());
        let duplicate = qr.replacen('{', "{\"version\":5,", 1);
        assert!(parse_invitation(&duplicate, now).is_err());
        for origin in fixture["invalidRelayOrigins"].as_array().unwrap() {
            assert!(relay_origin(origin.as_str().unwrap()).is_err(), "{origin}");
        }
    }
    #[test]
    fn replay_capacity_expiry_and_clock_rollback_fail_closed() {
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let mut window = RequestWindow::new(1);
        assert_eq!(window.admit(first, 100, 190, 100), Ok(()));
        assert_eq!(
            window.admit(first, 100, 190, 101),
            Err(ConnectionError::Replay)
        );
        assert_eq!(
            window.admit(second, 100, 190, 101),
            Err(ConnectionError::Capacity)
        );
        assert_eq!(
            window.admit(first, 100, 190, 102),
            Err(ConnectionError::Replay)
        );
        assert_eq!(window.admit(second, 190, 280, 190), Ok(()));
        assert_eq!(
            window.admit(first, 100, 190, 100),
            Err(ConnectionError::Expired)
        );
        assert_eq!(
            window.admit(first, 190, 281, 190),
            Err(ConnectionError::Expired)
        );
    }

    #[test]
    fn ping_freshness_validates_window_without_exhausting_capacity() {
        let mut window = RequestWindow::new(2);
        for _ in 0..100 {
            assert_eq!(
                window.check_freshness(Uuid::new_v4(), 100, 160, 100),
                Ok(())
            );
        }
        assert_eq!(window.admit(Uuid::new_v4(), 100, 160, 100), Ok(()));
        assert_eq!(window.admit(Uuid::new_v4(), 100, 160, 100), Ok(()));
        // Even with a full window, ping freshness checks continue to succeed.
        assert_eq!(
            window.check_freshness(Uuid::new_v4(), 100, 160, 100),
            Ok(())
        );
        // Expired or rolled-back pings are still rejected.
        assert_eq!(
            window.check_freshness(Uuid::new_v4(), 100, 160, 99),
            Err(ConnectionError::Expired)
        );
    }
}
