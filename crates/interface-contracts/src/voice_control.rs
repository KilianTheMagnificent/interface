//! Versioned, audio-independent control desk. The Mac owns every receipt.
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const VERSION: u8 = 1;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub version: u8,
    pub command: Command,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Command {
    Inspect {},
    Begin {
        run_id: String,
        cwd: String,
        duration_seconds: u64,
        max_actions: u16,
    },
    Message {
        run_id: String,
        request_id: String,
        text: String,
    },
    Stop {
        run_id: String,
    },
    Quiet {
        run_id: String,
        quiet: bool,
    },
    EndConversation {
        run_id: String,
    },
    Approve {
        run_id: String,
        request_id: String,
        approval_id: String,
        allow: bool,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Target {
    Pane { id: String, generation: u64 },
    Session { key: String },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Action {
    pub request_id: String,
    pub tool: String,
    pub input: Value,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RunState {
    Starting,
    Active,
    Stopped,
    Expired,
    RecoveryRequired,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ReceiptState {
    Started,
    Accepted,
    Unsent,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Receipt {
    pub action: Action,
    pub state: ReceiptState,
    pub detail: String,
    pub result: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Note {
    pub role: String,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Approval {
    pub id: String,
    pub session_key: String,
    pub provider_approval_id: String,
    pub description: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentObservation {
    pub session_key: String,
    pub status: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StopReceipt {
    pub target: Target,
    pub state: ReceiptState,
    pub detail: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Run {
    pub id: String,
    pub cwd: String,
    pub started_at_ms: u64,
    pub deadline_at_ms: u64,
    pub max_actions: u16,
    pub state: RunState,
    pub session_key: Option<String>,
    pub quiet: bool,
    pub conversation_open: bool,
    pub targets: Vec<Target>,
    pub controlled_targets: Vec<Target>,
    pub stops: Vec<StopReceipt>,
    pub coordinator_busy: bool,
    pub observations: Vec<AgentObservation>,
    pub pending_updates: Vec<Action>,
    pub receipts: Vec<Receipt>,
    pub notes: Vec<Note>,
    pub approvals: Vec<Approval>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Snapshot {
    pub version: u8,
    pub revision: u64,
    pub runs: Vec<Run>,
    pub storage_uncertain: bool,
    /// Deliberately disconnected until the final provider integration.
    pub audio_connected: bool,
}

/// An actually parsed renderer screen, fenced against host output and human input.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PaneScreen {
    pub screen: crate::terminal_controls::TerminalControlScreenV1,
    /// Decimal u64; JavaScript retains the exact byte cursor.
    pub cursor: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_desktop_phone_fixture_round_trips_and_refuses_extra_authority() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/voice-control-v21.json"
        ))
        .unwrap();
        for request in fixture["requests"].as_array().unwrap() {
            let decoded: Request = serde_json::from_value(request.clone()).unwrap();
            assert_eq!(serde_json::to_value(decoded).unwrap(), *request);
            let mut invalid = request.clone();
            invalid["command"]["bypassPermissions"] = Value::Bool(true);
            assert!(serde_json::from_value::<Request>(invalid).is_err());
        }
        let screen: PaneScreen = serde_json::from_value(fixture["paneScreen"].clone()).unwrap();
        assert_eq!(serde_json::to_value(screen).unwrap(), fixture["paneScreen"]);
        let snapshot: Snapshot = serde_json::from_value(fixture["snapshot"].clone()).unwrap();
        assert_eq!(serde_json::to_value(snapshot).unwrap(), fixture["snapshot"]);
    }
}
