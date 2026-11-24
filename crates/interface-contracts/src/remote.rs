//! Remote access v1. Desktop administration stays on authenticated local IPC.
use serde::{Deserialize, Serialize};

/// HTTP companion v2. Desktop administration and the v1 PTY transport are unchanged.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteWorkspaceV2 {
    pub id: String,
    pub name: String,
    pub root: String,
    pub terminals: Vec<crate::terminal::TerminalSummary>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteWorkspaceSnapshotV2 {
    pub version: u8,
    pub workspaces: Vec<RemoteWorkspaceV2>,
}

/// Phone roster v3: the v2 workspaces plus the project the Mac sidebar files each
/// terminal under, keyed by terminal id. `None` is the sidebar's "Other" group
/// (a chat that ran outside any repository).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteWorkspaceSnapshotV3 {
    pub version: u8,
    pub workspaces: Vec<RemoteWorkspaceV2>,
    pub projects: std::collections::BTreeMap<String, Option<String>>,
}

/// Phone roster v4 (review D6): the v3 shape, whose terminals carry the
/// host's `attention` (desktop protocol v44), so the phone can say which
/// sessions need you. v1, v2 and v3 keep their bytes and never send it.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteWorkspaceSnapshotV4 {
    pub version: u8,
    pub workspaces: Vec<RemoteWorkspaceV2>,
    pub projects: std::collections::BTreeMap<String, Option<String>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteHistoryV2 {
    pub version: u8,
    pub terminal_id: crate::terminal::TerminalId,
    pub generation: crate::terminal::TerminalGeneration,
    pub history: crate::ipc::HistoryPage,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "camelCase", deny_unknown_fields)]
pub enum RemoteAccessRequest {
    Status {},
    Enable { origin: String },
    Disable {},
    Pair {},
    Revoke { id: String },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteDevice {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteAccessStatus {
    pub version: u8,
    pub enabled: bool,
    pub local_url: Option<String>,
    pub public_url: Option<String>,
    pub devices: Vec<RemoteDevice>,
    /// Returned once only to the desktop that requested pairing; never logged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pairing_code: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_administration_matches_the_typescript_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/remote-access-v1.json"
        ))
        .unwrap();
        for request in fixture["requests"].as_array().unwrap() {
            let parsed: RemoteAccessRequest = serde_json::from_value(request.clone()).unwrap();
            assert_eq!(serde_json::to_value(parsed).unwrap(), *request);
            let mut unknown = request.clone();
            unknown["unexpected"] = true.into();
            assert!(serde_json::from_value::<RemoteAccessRequest>(unknown).is_err());
        }
        let status: RemoteAccessStatus = serde_json::from_value(fixture["status"].clone()).unwrap();
        assert_eq!(serde_json::to_value(status).unwrap(), fixture["status"]);
    }
    #[test]
    fn roster_v4_matches_the_typescript_and_swift_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/remote-phone-v4.json"
        ))
        .unwrap();
        let roster: RemoteWorkspaceSnapshotV4 =
            serde_json::from_value(fixture["roster"].clone()).unwrap();
        assert_eq!(roster.version, 4);
        let terminals = &roster.workspaces[0].terminals;
        assert_eq!(
            terminals[0].attention.as_ref().map(|a| a.since.as_str()),
            Some("2026-10-14T09:12:00.000Z")
        );
        assert!(terminals[1].attention.is_none());
        assert_eq!(serde_json::to_value(&roster).unwrap(), fixture["roster"]);
    }

    #[test]
    fn companion_v2_matches_the_typescript_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/remote-companion-v2.json"
        ))
        .unwrap();
        let snapshot: RemoteWorkspaceSnapshotV2 =
            serde_json::from_value(fixture["snapshot"].clone()).unwrap();
        assert_eq!(serde_json::to_value(snapshot).unwrap(), fixture["snapshot"]);
        let history: RemoteHistoryV2 = serde_json::from_value(fixture["history"].clone()).unwrap();
        assert_eq!(serde_json::to_value(history).unwrap(), fixture["history"]);
        let mut unknown = fixture["history"].clone();
        unknown["providerKey"] = "arbitrary".into();
        assert!(serde_json::from_value::<RemoteHistoryV2>(unknown).is_err());
    }
}
