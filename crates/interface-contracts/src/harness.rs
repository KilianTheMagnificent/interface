//! Additive harness surface v1. Identity references grant no filesystem or provider authority.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AttentionTarget {
    Session {
        key: String,
    },
    Terminal {
        terminal_id: String,
        generation: u64,
    },
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttentionItem {
    pub id: String,
    pub kind: String,
    pub target: AttentionTarget,
    pub title: String,
    pub place: String,
    pub at: u64,
    pub provenance: String,
    pub seen: bool,
    pub resolved: bool,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskEvidence {
    pub id: String,
    pub objective: String,
    pub target: AttentionTarget,
    pub root: String,
    pub baseline: String,
    pub candidate: String,
    pub changed_paths: Vec<String>,
    pub checks: Vec<crate::ipc::ReviewCheckRunResult>,
    pub check_candidate: Option<String>,
    pub findings: Vec<String>,
    pub state: String,
    pub at: u64,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SharedDraft {
    pub target: AttentionTarget,
    pub revision: u64,
    pub text: String,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationBranch {
    pub parent: String,
    pub child: String,
    pub root: String,
    pub boundary: String,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HarnessState {
    pub version: u8,
    pub revision: u64,
    pub attention: Vec<AttentionItem>,
    pub tasks: Vec<TaskEvidence>,
    pub drafts: Vec<SharedDraft>,
    pub branches: Vec<ConversationBranch>,
}
impl Default for HarnessState {
    fn default() -> Self {
        Self {
            version: 1,
            revision: 0,
            attention: vec![],
            tasks: vec![],
            drafts: vec![],
            branches: vec![],
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum HarnessCommand {
    Read {},
    RecordBranch {
        branch: ConversationBranch,
    },
    ReadDraft {
        target: AttentionTarget,
    },
    SaveDraft {
        target: AttentionTarget,
        expected_revision: u64,
        text: String,
    },
    Catalog {},
    RecordAttention {
        item: AttentionItem,
    },
    MarkSeen {
        ids: Vec<String>,
    },
    Resolve {
        id: String,
        target: AttentionTarget,
    },
    CaptureResult {
        id: String,
        objective: String,
        target: AttentionTarget,
        root: String,
    },
    RunChecks {
        id: String,
        selected: Vec<String>,
    },
    InspectResult {
        id: String,
    },
    Context {
        root: String,
        query: String,
    },
    Presets {
        root: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_v20_contract_round_trips_and_rejects_extra_effect_fields() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/harness-v20.json"
        ))
        .unwrap();
        for value in fixture["commands"].as_array().unwrap() {
            let command: HarnessCommand = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(command).unwrap(), *value);
            let mut foreign = value.clone();
            foreign["executeShell"] = serde_json::json!(true);
            assert!(serde_json::from_value::<HarnessCommand>(foreign).is_err());
        }
        let state: HarnessState = serde_json::from_value(fixture["state"].clone()).unwrap();
        assert_eq!(serde_json::to_value(state).unwrap(), fixture["state"]);
        let request: crate::terminal::TerminalGenerationRequest =
            serde_json::from_value(fixture["serverRequest"].clone()).unwrap();
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            fixture["serverRequest"]
        );
        let links: Vec<crate::workspace_tools::TerminalServerLink> =
            serde_json::from_value(fixture["serverLinks"].clone()).unwrap();
        assert_eq!(serde_json::to_value(links).unwrap(), fixture["serverLinks"]);
    }
}
