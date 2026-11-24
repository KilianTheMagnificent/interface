//! Explicit v3 native companion enrollment. Secrets are never Debug or desktop DTOs.
use crate::remote::RemoteDevice;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "camelCase", deny_unknown_fields)]
pub enum TrustCommand {
    Status {},
    Approve { id: String },
    Reject { id: String },
    Forget { id: String },
}
#[derive(Deserialize, Serialize)]
// The flattened command applies deny_unknown_fields after action/version are consumed.
// Serde forbids deny_unknown_fields on this outer flatten container.
#[serde(rename_all = "camelCase")]
pub struct TrustAdminRequest {
    pub action: TrustAction,
    pub version: u8,
    #[serde(flatten)]
    pub command: TrustCommand,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TrustAction {
    Trusted,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingDevice {
    pub id: String,
    pub name: String,
    pub verification: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrustStatus {
    pub version: u8,
    pub pending: Vec<PendingDevice>,
    pub devices: Vec<RemoteDevice>,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnrollmentRequest {
    pub version: u8,
    pub id: String,
    pub name: String,
    pub secret: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnlockRequest {
    pub version: u8,
    pub id: String,
    pub secret: String,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnrollmentStatus {
    pub version: u8,
    pub approved: bool,
    pub verification: String,
}

/// New owner-private v1 file; only credential digests, never bearer credentials.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrustedDeviceRecord {
    pub id: String,
    pub name: String,
    pub digest: [u8; 32],
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RememberedListener {
    pub enabled: bool,
    pub port: u16,
    pub origin: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteTrustData {
    pub version: u8,
    pub listener: Option<RememberedListener>,
    pub devices: Vec<TrustedDeviceRecord>,
}
impl Default for RemoteTrustData {
    fn default() -> Self {
        Self {
            version: 1,
            listener: None,
            devices: Vec::new(),
        }
    }
}
impl RemoteTrustData {
    #[must_use]
    pub fn valid(&self) -> bool {
        self.version == 1
            && self.devices.len() <= 8
            && self
                .listener
                .as_ref()
                .is_none_or(|l| l.port != 0 && l.origin.len() <= 253)
            && self.devices.iter().enumerate().all(|(i, d)| {
                uuid::Uuid::parse_str(&d.id).is_ok()
                    && !d.name.trim().is_empty()
                    && d.name.len() <= 80
                    && !d.name.chars().any(char::is_control)
                    && self.devices[..i].iter().all(|p| p.id != d.id)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_trust_envelopes_match_shared_fixture_and_reject_unknown_fields() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/remote-trust-v3.json"
        ))
        .unwrap();
        for request in fixture["admin"].as_array().unwrap() {
            let parsed: TrustAdminRequest = serde_json::from_value(request.clone()).unwrap();
            assert_eq!(serde_json::to_value(parsed).unwrap(), *request);
            let mut extra = request.clone();
            extra["origin"] = "untrusted".into();
            assert!(serde_json::from_value::<TrustAdminRequest>(extra).is_err());
        }
        let status: TrustStatus = serde_json::from_value(fixture["status"].clone()).unwrap();
        assert_eq!(serde_json::to_value(status).unwrap(), fixture["status"]);
        let enrollment: EnrollmentRequest =
            serde_json::from_value(fixture["enrollment"].clone()).unwrap();
        assert_eq!(
            serde_json::to_value(enrollment).unwrap(),
            fixture["enrollment"]
        );
        let unlock: UnlockRequest = serde_json::from_value(fixture["unlock"].clone()).unwrap();
        assert_eq!(serde_json::to_value(unlock).unwrap(), fixture["unlock"]);
        let reply: EnrollmentStatus = serde_json::from_value(fixture["reply"].clone()).unwrap();
        assert_eq!(serde_json::to_value(reply).unwrap(), fixture["reply"]);
    }
}
