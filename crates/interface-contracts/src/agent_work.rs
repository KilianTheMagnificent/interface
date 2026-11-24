//! Agent task contracts and evidence, separate from the accepted v2 roster store.
pub const AGENT_TOOL_NAMES: [&str; 8] = [
    "task_status",
    "artifact_register",
    "memory_propose",
    "notification_request",
    "routine_draft",
    "consult_start",
    "consult_review",
    "consult_apply",
];
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentBudget {
    pub wall_seconds: u32,
    pub no_progress_seconds: u32,
    pub max_actions: u32,
    pub max_reported_tokens: Option<u64>,
}
impl Default for AgentBudget {
    fn default() -> Self {
        Self {
            wall_seconds: 1800,
            no_progress_seconds: 600,
            max_actions: 200,
            max_reported_tokens: None,
        }
    }
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ArtifactFormat {
    Nonempty,
    Json,
    Text,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentArtifactCheck {
    pub path: String,
    pub format: ArtifactFormat,
    pub contains: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentInputRequirement {
    pub path: String,
    pub max_age_seconds: u32,
}
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentWorkSpec {
    pub expected_output: String,
    pub artifacts: Vec<AgentArtifactCheck>,
    pub inputs: Vec<AgentInputRequirement>,
    pub budget: AgentBudget,
    /// Read-only allows shared resource leases. Writes still need provider containment.
    pub read_only: bool,
    /// Owner opt-in to the legacy risky-list ceiling for opaque commands.
    #[serde(default)]
    pub allow_opaque_shell_commands: bool,
    /// Protocol 71: owner opt-in to web searches without asking. A search can
    /// carry what the agent has read to the search provider, so it asks by
    /// default. Only `true` is written, so a spec with it off is unchanged.
    #[serde(default, skip_serializing_if = "is_false")]
    pub allow_web_search: bool,
    /// Owner-only selected-file data-sharing grant. Never supplied by a child tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consultation: Option<AgentConsultationGrant>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentConsultationGrant {
    /// An explicit claude/<model> or codex/<model>, with no fallback.
    pub model: String,
    pub files: Vec<String>,
    /// Existing files the parent may compare-and-save after reviewing a child proposal.
    pub writable_files: Vec<String>,
    pub max_children: u8,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentChildProposal {
    pub path: String,
    pub expected_sha256: String,
    pub text: String,
    pub applied_sha256: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentConsultation {
    pub id: String,
    pub task_id: String,
    pub model: String,
    pub brief: String,
    pub inputs: Vec<AgentEvidence>,
    pub result_sha256: Option<String>,
    pub report: String,
    pub proposals: Vec<AgentChildProposal>,
    /// pending, completed, failed, unknown. A completed child never completes its parent.
    pub status: String,
    pub review: Option<String>,
    pub elapsed_ms: Option<u64>,
    pub reported_tokens: Option<u64>,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentExecutionState {
    Starting,
    Running,
    Completed,
    Failed,
    Stopped,
    Unknown,
}
impl AgentExecutionState {
    #[must_use]
    pub const fn holds_resources(self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Unknown)
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentVerificationState {
    Unverified,
    Verified,
    Failed,
    Stale,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentEvidence {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
    pub checked_at_ms: u64,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentTaskRecord {
    pub id: String,
    pub agent_id: String,
    pub session_key: Option<String>,
    pub goal: String,
    pub spec: AgentWorkSpec,
    pub roots: Vec<String>,
    pub cwd: String,
    pub started_at_ms: u64,
    pub deadline_at_ms: u64,
    pub progress_at_ms: u64,
    pub progress_sha256: String,
    pub began: bool,
    pub actions: u32,
    pub reported_tokens: Option<u64>,
    pub execution: AgentExecutionState,
    pub verification: AgentVerificationState,
    pub input_evidence: Vec<AgentEvidence>,
    pub artifacts: Vec<AgentEvidence>,
    pub detail: String,
    pub next_action: String,
    pub stop_requested: bool,
    pub notes: Vec<String>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentMemoryRevision {
    pub sha256: String,
    pub text: String,
    pub observed_at_ms: u64,
    pub source: String,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentFactState {
    Proposed,
    OwnerConfirmed,
    Superseded,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentMemoryFact {
    pub id: String,
    pub text: String,
    pub source: String,
    pub observed_at_ms: u64,
    pub expires_at_ms: Option<u64>,
    pub supersedes: Option<String>,
    pub state: AgentFactState,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentWorkView {
    pub version: u8,
    pub revision: u64,
    pub spec: AgentWorkSpec,
    pub tasks: Vec<AgentTaskRecord>,
    pub memory_history: Vec<AgentMemoryRevision>,
    pub facts: Vec<AgentMemoryFact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub consultations: Vec<AgentConsultation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expired_approvals: Vec<crate::agents::AgentPendingApproval>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AgentWorkCommand {
    Read {},
    Configure {
        expected_revision: u64,
        spec: AgentWorkSpec,
    },
    Verify {
        task_id: String,
    },
    Stop {
        task_id: String,
    },
    Acknowledge {
        task_id: String,
    },
    RestoreMemory {
        sha256: String,
        expected_sha256: String,
    },
    ProposeFact {
        fact: AgentMemoryFact,
    },
    ConfirmFact {
        id: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn v29_agent_work_fixture_round_trips_and_rejects_foreign_authority() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/agent-work-v29.json"
        ))
        .unwrap();
        for value in fixture["commands"].as_array().unwrap() {
            let command: crate::agents::AgentCommand =
                serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(command).unwrap(), *value);
            let mut invalid = value.clone();
            invalid["command"]["bypassPermissions"] = serde_json::json!(true);
            assert!(serde_json::from_value::<crate::agents::AgentCommand>(invalid).is_err());
        }
        let view: AgentWorkView = serde_json::from_value(fixture["view"].clone()).unwrap();
        assert_eq!(serde_json::to_value(view).unwrap(), fixture["view"]);
    }

    /// Protocol 71: an agent's "Search the web without asking" switch
    /// round-trips; a spec without it reads as off and writes nothing for it,
    /// so a spec saved with it off is byte-identical to before.
    #[test]
    fn shared_v71_web_search_switch_round_trips_and_its_absence_means_ask() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/ask-first-v71.json"
        ))
        .unwrap();
        assert_eq!(fixture["protocolVersion"], 71);
        for value in fixture["agentCommands"].as_array().unwrap() {
            let command: crate::agents::AgentCommand =
                serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(command).unwrap(), *value);
        }
        let spec: AgentWorkSpec =
            serde_json::from_value(fixture["agentCommands"][0]["command"]["spec"].clone()).unwrap();
        assert!(spec.allow_web_search);
        let older: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/agent-work-v29.json"
        ))
        .unwrap();
        let older: AgentWorkSpec = serde_json::from_value(older["view"]["spec"].clone()).unwrap();
        assert!(!older.allow_web_search, "absent means ask");
        let written = serde_json::to_value(AgentWorkSpec::default()).unwrap();
        assert!(
            written.get("allowWebSearch").is_none(),
            "off is not written"
        );
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentMissedRunPolicy {
    Skip,
    RunOnce,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCalendarSchedule {
    pub timezone: String,
    /// ISO weekdays, Monday=1 through Sunday=7. Empty means every day.
    pub weekdays: Vec<u8>,
    pub hour: u8,
    pub minute: u8,
    pub missed: AgentMissedRunPolicy,
    /// Pause after a failed or uncertain run. Unknown work is always non-replayable.
    pub pause_on_failure: bool,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCalendarView {
    pub routine_id: String,
    pub schedule: Option<AgentCalendarSchedule>,
    pub next_occurrences_ms: Vec<u64>,
    pub tested: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AgentCalendarCommand {
    Read {
        routine_id: String,
    },
    Save {
        expected_revision: u64,
        routine_id: String,
        schedule: Option<AgentCalendarSchedule>,
    },
    Preview {
        schedule: AgentCalendarSchedule,
        after_ms: u64,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentToolReceipt {
    pub fingerprint: String,
    pub result: Option<String>,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if passes a reference.
const fn is_false(value: &bool) -> bool {
    !*value
}
