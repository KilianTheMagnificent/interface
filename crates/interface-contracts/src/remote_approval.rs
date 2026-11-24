//! Phone approval v4, independent of desktop IPC and persisted schemas.
use crate::terminal_controls::TerminalControlScreenV1;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalObservationV4 {
    pub version: u8,
    pub cursor: u64,
    pub screen: TerminalControlScreenV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalDecisionV4 {
    AllowOnce,
    Deny,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalResponseV4 {
    pub version: u8,
    pub cursor: u64,
    pub screen: TerminalControlScreenV1,
    pub approval_id: String,
    pub decision: ApprovalDecisionV4,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalV4 {
    pub id: String,
    pub title: String,
    pub detail: String,
    pub can_allow_once: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalSnapshotV4 {
    pub version: u8,
    pub approval: Option<ApprovalV4>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn phone_approval_matches_typescript_and_rejects_unversioned_actions() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/remote-approval-v4.json"
        ))
        .unwrap();
        let observation: ApprovalObservationV4 =
            serde_json::from_value(fixture["observation"].clone()).unwrap();
        assert_eq!(
            serde_json::to_value(observation).unwrap(),
            fixture["observation"]
        );
        let response: ApprovalResponseV4 =
            serde_json::from_value(fixture["response"].clone()).unwrap();
        assert_eq!(serde_json::to_value(response).unwrap(), fixture["response"]);
        let snapshot: ApprovalSnapshotV4 =
            serde_json::from_value(fixture["snapshot"].clone()).unwrap();
        assert_eq!(serde_json::to_value(snapshot).unwrap(), fixture["snapshot"]);
        let mut invalid = fixture["response"].clone();
        invalid["decision"] = "allowAlways".into();
        assert!(serde_json::from_value::<ApprovalResponseV4>(invalid).is_err());
        let mut invalid = fixture["observation"].clone();
        invalid["profile"] = "codex".into();
        assert!(serde_json::from_value::<ApprovalObservationV4>(invalid).is_err());
    }
}
