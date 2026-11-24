//! Protocol 52: cooperative update checkpoints and their restart receipts.
//! The feature was removed on 2026-10-21; the wire shapes stay because the
//! host still answers `iface:updateSessions` (an idle `read`, nothing else).
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "command", rename_all = "camelCase", deny_unknown_fields)]
pub enum UpdateSessionsCommand {
    Read,
    Prepare,
    Cancel,
    Resume,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateSessionsPhase {
    #[default]
    Idle,
    Checkpointing,
    Ready,
    Installing,
    Resuming,
    Complete,
    Cancelled,
    Blocked,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateSessionPhase {
    Pending,
    RequestStarted,
    Waiting,
    Ready,
    ResumeStarted,
    Resumed,
    Blocked,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateSessionView {
    pub id: String,
    pub title: String,
    pub phase: UpdateSessionPhase,
    pub detail: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateSessionsView {
    pub version: u8,
    pub phase: UpdateSessionsPhase,
    pub sessions: Vec<UpdateSessionView>,
    pub detail: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn update_sessions_protocol_52_matches_shared_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/update-sessions-v52.json"
        ))
        .unwrap();
        let command: UpdateSessionsCommand =
            serde_json::from_value(fixture["command"].clone()).unwrap();
        assert_eq!(command, UpdateSessionsCommand::Prepare);
        let view: UpdateSessionsView = serde_json::from_value(fixture["view"].clone()).unwrap();
        assert_eq!(serde_json::to_value(view).unwrap(), fixture["view"]);
    }
}
