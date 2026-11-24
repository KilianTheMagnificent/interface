//! V5 connection metadata. These DTOs confer no device or session authority.
use crate::validation::ValidationError;
pub use crate::validation::lowercase_hex;
use serde::{Deserialize, Serialize};

pub const CONNECTION_VERSION: u8 = 5;
pub const PAIRING_PREFIX: &str = "interface-pair:v5:";
pub const MAX_INVITATION_BYTES: usize = 2_048;
pub const INVITATION_LIFETIME_SECONDS: u64 = 300;
pub const MAX_PACKET_BYTES: usize = 4 * 1024 * 1024;
pub const PAIRING_PREFIX_V6: &str = "interface-pair:v6:";
pub const MAX_NEARBY_ORIGINS: usize = 8;

/// A separate v6 invitation adds routing hints without changing v5 trust or packets.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingInvitationV6 {
    pub version: u8,
    pub invitation: PairingInvitationV5,
    pub nearby_origins: Vec<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingRequestV6 {
    pub action: PairingActionV6,
    pub version: u8,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PairingActionV6 {
    PairDirect,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingStatusV6 {
    pub version: u8,
    pub invitation: PairingInvitationV6,
    pub qr_rows: Vec<String>,
}

/// Non-secret sidecar; the accepted private identity configuration stays v1.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionListenerV1 {
    pub version: u8,
    pub port: u16,
}
impl ConnectionListenerV1 {
    #[must_use]
    pub fn valid(&self) -> bool {
        self.version == 1 && self.port >= 1024
    }
}

/// Canonical, bounded RFC1918/link-local IPv4 hints, never URLs with credentials.
#[must_use]
pub fn nearby_origin(value: &str) -> bool {
    let Some(authority) = value.strip_prefix("http://") else {
        return false;
    };
    let Some((host, port)) = authority.split_once(':') else {
        return false;
    };
    let Ok(ip) = host.parse::<std::net::Ipv4Addr>() else {
        return false;
    };
    let Ok(port) = port.parse::<u16>() else {
        return false;
    };
    (ip.is_private() || ip.is_link_local())
        && port >= 1024
        && value == format!("http://{ip}:{port}")
}
/// The Mac's current nearby routes, refreshed after each unlock so an address
/// change never needs a new QR code. Version 1 also carried a tailnet
/// (100.64.0.0/10) address until the Tailscale route was retired on
/// 2026-10-14; such an origin is no longer valid.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MacRoutesV1 {
    pub version: u8,
    pub origins: Vec<String>,
}
impl MacRoutesV1 {
    #[must_use]
    pub fn valid(&self) -> bool {
        self.version == 1
            && self.origins.len() <= MAX_NEARBY_ORIGINS
            && self
                .origins
                .iter()
                .enumerate()
                .all(|(i, origin)| nearby_origin(origin) && !self.origins[..i].contains(origin))
    }
}

impl PairingInvitationV6 {
    /// # Errors
    /// Rejects malformed, repeated or unbounded routing hints and expired invitations.
    pub fn validate_at(&self, now: u64) -> Result<(), ValidationError> {
        self.invitation.validate_at(now)?;
        if self.version != 6
            || self.nearby_origins.len() > MAX_NEARBY_ORIGINS
            || self.nearby_origins.iter().enumerate().any(|(i, origin)| {
                !nearby_origin(origin) || self.nearby_origins[..i].contains(origin)
            })
        {
            return Err(ValidationError::new(
                "invitation",
                "invalid v6 routing hints",
            ));
        }
        Ok(())
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EncryptedPacketV5 {
    pub version: u8,
    pub mac_id: String,
    pub request_id: String,
    pub sender_key: String,
    pub encapsulated_key: String,
    pub ciphertext: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TunnelRequestV5 {
    pub version: u8,
    pub issued_at: u64,
    pub expires_at: u64,
    pub method: String,
    pub path: String,
    pub body: String,
    pub cookie: Option<String>,
    pub control: Option<String>,
    pub command: Option<String>,
    pub invitation_id: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TunnelResponseV5 {
    pub version: u8,
    pub status: u16,
    pub body: String,
    pub content_type: String,
    pub set_cookie: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionConfigurationV1 {
    pub version: u8,
    pub enabled: bool,
    pub private_key: String,
    pub relay_origin: Option<String>,
}
impl ConnectionConfigurationV1 {
    #[must_use]
    pub fn valid(&self) -> bool {
        self.version == 1
            && lowercase_hex(&self.private_key, 32)
            && self
                .relay_origin
                .as_ref()
                .is_none_or(|v| !v.is_empty() && v.len() <= 253)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionAdminRequest {
    pub action: ConnectionAction,
    pub version: u8,
    #[serde(flatten)]
    pub command: ConnectionCommand,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionAction {
    Connection,
}
#[derive(Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "camelCase", deny_unknown_fields)]
pub enum ConnectionCommand {
    Status {},
    Enable {
        #[serde(rename = "relayOrigin")]
        relay_origin: Option<String>,
    },
    Disable {},
    Pair {},
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionStatusV5 {
    pub version: u8,
    pub enabled: bool,
    pub mac_id: Option<String>,
    pub relay_origin: Option<String>,
    pub relay_online: bool,
    pub nearby_available: bool,
    pub invitation: Option<PairingInvitationV5>,
    pub qr_rows: Option<Vec<String>>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MacProfile {
    Production,
    Development,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InvitationKind {
    InterfacePairing,
}

/// Only presented locally as a short-lived QR invitation. Do not log its contents.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PairingInvitationV5 {
    pub version: u8,
    pub kind: InvitationKind,
    pub mac_id: String,
    pub mac_name: String,
    pub profile: MacProfile,
    pub public_key: String,
    pub invitation_id: String,
    pub issued_at: u64,
    pub expires_at: u64,
    pub relay_origin: Option<String>,
}

impl PairingInvitationV5 {
    /// Validates the wire shape. Crypto and URL admission remain the Rust service's job.
    /// # Errors
    /// Rejects unsupported versions, malformed fields and invalid lifetimes.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let name = &self.mac_name;
        if self.version != CONNECTION_VERSION
            || !lowercase_hex(&self.mac_id, 32)
            || !lowercase_hex(&self.public_key, 65)
            || !self.public_key.starts_with("04")
            || !lowercase_hex(&self.invitation_id, 32)
            || name.trim() != name
            || name.is_empty()
            || name.len() > 80
            || name.chars().any(char::is_control)
            || self.issued_at == 0
            || self.issued_at > 9_007_199_254_740_991 - INVITATION_LIFETIME_SECONDS
            || self.expires_at <= self.issued_at
            || self.expires_at - self.issued_at > INVITATION_LIFETIME_SECONDS
            || self
                .relay_origin
                .as_ref()
                .is_some_and(|v| v.is_empty() || v.len() > 253)
        {
            return Err(ValidationError::new("invitation", "invalid v5 invitation"));
        }
        Ok(())
    }

    /// # Errors
    /// A future or expired invitation never grants a fresh enrollment window.
    pub fn validate_at(&self, now: u64) -> Result<(), ValidationError> {
        self.validate()?;
        if now < self.issued_at || now >= self.expires_at {
            return Err(ValidationError::new(
                "invitation",
                "invitation is not current",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn v6_wrapper_routes_and_admin_match_native_and_typescript() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/remote-connection-v6.json"
        ))
        .unwrap();
        let invitation: PairingInvitationV6 =
            serde_json::from_value(fixture["invitation"].clone()).unwrap();
        let now = invitation.invitation.issued_at;
        invitation.validate_at(now).unwrap();
        let canonical = serde_json::to_value(&invitation).unwrap();
        assert_eq!(
            format!("{PAIRING_PREFIX_V6}{canonical}"),
            fixture["qr"].as_str().unwrap()
        );
        assert!(fixture["qr"].as_str().unwrap().len() <= MAX_INVITATION_BYTES);
        let admin: PairingRequestV6 = serde_json::from_value(fixture["admin"].clone()).unwrap();
        assert_eq!(serde_json::to_value(admin).unwrap(), fixture["admin"]);
        for value in fixture["validNearbyOrigins"].as_array().unwrap() {
            assert!(nearby_origin(value.as_str().unwrap()));
        }
        for value in fixture["invalidNearbyOrigins"].as_array().unwrap() {
            assert!(!nearby_origin(value.as_str().unwrap()), "{value}");
        }
        for origins in [
            vec![invitation.nearby_origins[0].clone(); 2],
            (1..=9).map(|i| format!("http://10.0.0.{i}:5000")).collect(),
        ] {
            let mut invalid = invitation.clone();
            invalid.nearby_origins = origins;
            assert!(invalid.validate_at(now).is_err());
        }
        assert!(
            invitation
                .validate_at(invitation.invitation.expires_at)
                .is_err()
        );
        let mut invalid = canonical;
        invalid["secret"] = "unexpected".into();
        assert!(serde_json::from_value::<PairingInvitationV6>(invalid).is_err());
    }
    #[test]
    fn mac_routes_are_nearby_origins_only_now_the_tailnet_route_is_retired() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/remote-routes-v1.json"
        ))
        .unwrap();
        let routes: MacRoutesV1 = serde_json::from_value(fixture["routes"].clone()).unwrap();
        assert!(routes.valid());
        assert_eq!(serde_json::to_value(&routes).unwrap(), fixture["routes"]);
        for value in fixture["validOrigins"].as_array().unwrap() {
            assert!(nearby_origin(value.as_str().unwrap()), "{value}");
        }
        for value in fixture["invalidOrigins"].as_array().unwrap() {
            assert!(!nearby_origin(value.as_str().unwrap()), "{value}");
        }
        // A route list a Tailscale-era Mac sent is refused as a whole.
        let mut tailnet = routes.clone();
        tailnet.origins.push("http://100.101.102.103:58747".into());
        assert!(!tailnet.valid());
        let mut repeated = routes.clone();
        repeated.origins.push(repeated.origins[0].clone());
        assert!(!repeated.valid());
    }
    fn fixture() -> serde_json::Value {
        serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/remote-connection-v5.json"
        ))
        .unwrap()
    }
    #[test]
    fn v5_admin_and_tunnel_envelopes_roundtrip_shared_fixtures() {
        let fixture = fixture();
        for value in fixture["admin"].as_array().unwrap() {
            let request: ConnectionAdminRequest = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(request).unwrap(), *value);
            let mut extra = value.clone();
            extra["token"] = "unexpected".into();
            assert!(serde_json::from_value::<ConnectionAdminRequest>(extra).is_err());
        }
        let status: ConnectionStatusV5 = serde_json::from_value(fixture["status"].clone()).unwrap();
        assert_eq!(serde_json::to_value(status).unwrap(), fixture["status"]);
        let request: TunnelRequestV5 = serde_json::from_value(fixture["request"].clone()).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), fixture["request"]);
        let response: TunnelResponseV5 =
            serde_json::from_value(fixture["response"].clone()).unwrap();
        assert_eq!(serde_json::to_value(response).unwrap(), fixture["response"]);
    }
    #[test]
    fn invitation_has_shared_shape_and_no_implicit_version_upgrade() {
        let value = fixture()["invitation"].clone();
        let invitation: PairingInvitationV5 = serde_json::from_value(value.clone()).unwrap();
        invitation.validate_at(invitation.issued_at).unwrap();
        assert_eq!(serde_json::to_value(&invitation).unwrap(), value);
        for (key, bad) in [
            ("version", serde_json::json!(6)),
            ("profile", serde_json::json!("unknown")),
            ("macId", serde_json::json!("aa")),
            (
                "publicKey",
                serde_json::json!("02".to_owned() + &"ab".repeat(64)),
            ),
            ("macName", serde_json::json!("Mac\n")),
            ("expiresAt", serde_json::json!(invitation.issued_at + 301)),
            ("issuedAt", serde_json::json!(u64::MAX)),
            ("secret", serde_json::json!("unexpected")),
        ] {
            let mut malformed = value.clone();
            malformed[key] = bad;
            assert!(
                serde_json::from_value::<PairingInvitationV5>(malformed)
                    .map_or(true, |parsed| parsed.validate().is_err()),
                "{key}"
            );
        }
        assert!(invitation.validate_at(invitation.issued_at - 1).is_err());
        assert!(invitation.validate_at(invitation.expires_at).is_err());
    }
}
