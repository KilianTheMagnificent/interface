//! Read-only desktop projection of pane-owned delegation. Protocol v12.
use crate::terminal::{TerminalGeneration, TerminalId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadTerminalDelegation {
    pub id: TerminalId,
    pub generation: TerminalGeneration,
    #[serde(default)]
    pub task_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DelegationActivityView {
    pub entries: Vec<DelegationActivityEntry>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DelegationActivityEntry {
    pub sequence: u64,
    pub kind: String,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DelegationTaskView {
    pub id: String,
    pub title: String,
    pub model: String,
    pub state: String,
    pub activity: Option<DelegationActivityView>,
    pub terminal: Option<crate::terminal::TerminalSummary>,
    pub notes: Option<String>,
    pub files: Vec<String>,
    pub elapsed_ms: Option<u64>,
    pub usage: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DelegationWorkflowView {
    pub id: String,
    pub title: String,
    pub decision: String,
    pub reason: Option<String>,
    pub state: String,
    pub hook_observed: bool,
    pub parallel_tools: u64,
    pub checks: Vec<String>,
    pub tasks: Vec<DelegationTaskView>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalDelegationView {
    pub version: u8,
    pub available: bool,
    pub workflows: Vec<DelegationWorkflowView>,
    pub detail: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_v12_read_contract_matches_the_renderer_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/terminal-delegation-v12.json"
        ))
        .unwrap();
        let request: ReadTerminalDelegation =
            serde_json::from_value(fixture["request"].clone()).unwrap();
        let view: TerminalDelegationView = serde_json::from_value(fixture["view"].clone()).unwrap();
        assert_eq!(serde_json::to_value(request).unwrap(), fixture["request"]);
        assert_eq!(view.version, 3);
        assert!(view.workflows[0].tasks[0].terminal.is_some());
        assert_eq!(serde_json::to_value(view).unwrap(), fixture["view"]);
        assert_eq!(
            fixture["channel"],
            crate::ipc::CHANNELS.read_terminal_delegation
        );
        let mut wrong = fixture["request"].clone();
        wrong["generation"] = serde_json::json!(-1);
        assert!(serde_json::from_value::<ReadTerminalDelegation>(wrong).is_err());
        let mut extra = fixture["request"].clone();
        extra["owner"] = serde_json::json!("other-pane");
        assert!(serde_json::from_value::<ReadTerminalDelegation>(extra).is_err());
    }
}
