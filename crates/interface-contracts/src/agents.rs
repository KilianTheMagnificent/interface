//! Versioned Agent workspace. Time is Unix milliseconds, bounded to JS-safe integers.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentShape {
    Droplet,
    Blossom,
    Pebble,
    Triangle,
    Diamond,
    Arch,
    Star,
    Hexagon,
    Square,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentColor {
    Teal,
    Coral,
    Sky,
    Sand,
    Violet,
    Moss,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentProfile {
    pub id: String,
    pub name: String,
    pub engine: String,
    pub model: String,
    pub effort: String,
    pub instructions: String,
    pub color: AgentColor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<AgentShape>,
    /// Rust-owned: set when the home folder is created. Renderer values are ignored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
    /// Rust-owned: changed only by `SaveAgentFolders`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<String>,
    /// SHA-256 (lowercase hex) of the instruction files Interface last wrote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions_file_sha256: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentRoutine {
    pub id: String,
    pub agent_id: String,
    pub name: String,
    pub prompt: String,
    pub cwd: String,
    pub enabled: bool,
    pub start_at_ms: u64,
    /// Fixed elapsed-time interval, not a wall-clock calendar schedule.
    pub interval_minutes: Option<u32>,
    pub next_run_at_ms: Option<u64>,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentRunStatus {
    Queued,
    Starting,
    Running,
    Waiting,
    Succeeded,
    Failed,
    Unknown,
}
impl AgentRunStatus {
    #[must_use]
    pub const fn blocks_agent(self) -> bool {
        matches!(
            self,
            Self::Queued | Self::Starting | Self::Running | Self::Waiting | Self::Unknown
        )
    }
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentRun {
    pub id: String,
    pub routine_id: String,
    pub agent_id: String,
    pub name: String,
    pub scheduled_at_ms: u64,
    pub updated_at_ms: u64,
    pub status: AgentRunStatus,
    pub session_key: Option<String>,
    pub detail: String,
    // Snapshot the execution inputs. Editing a profile/routine never changes an admitted run.
    pub profile: AgentProfile,
    pub prompt: String,
    pub cwd: String,
    pub began: bool,
}
pub const AGENT_WORKSPACE_VERSION: u8 = 2;

/// How an agent's threads and routines get permission to act. Rust-owned and
/// stored beside the roster, never on the profile; an absent entry is `Gated`.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentPermissionMode {
    /// Interface's approval gate and policy, then the owner.
    #[default]
    Gated,
    /// The provider's own auto mode decides; what it escalates goes to the owner.
    Auto,
    /// No permission checks and no sandbox.
    Bypass,
}

impl AgentPermissionMode {
    /// The launch access for this mode; `None` launches behind the approval gate.
    #[must_use]
    pub const fn access(self) -> Option<crate::engine::AgentAccess> {
        match self {
            Self::Gated => None,
            Self::Auto => Some(crate::engine::AgentAccess::Auto),
            Self::Bypass => Some(crate::engine::AgentAccess::Bypass),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentThreadStatus {
    #[default]
    Idle,
    Working,
    Waiting,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentThread {
    pub session_key: String,
    pub started_at_ms: u64,
    pub status: AgentThreadStatus,
    #[serde(default)]
    pub interrupted: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentPastThread {
    pub session_key: String,
    pub started_at_ms: u64,
    pub ended_at_ms: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentThreadState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<AgentThread>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub past: Vec<AgentPastThread>,
    #[serde(default)]
    pub last_read_at_ms: u64,
    #[serde(default)]
    pub last_activity_at_ms: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentStatus {
    Idle,
    Working,
    NeedsAttention,
    Unread,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentPendingApproval {
    pub agent_id: String,
    pub session_key: String,
    pub approval_id: String,
    pub tool: String,
    pub summary: String,
    pub reason: String,
    pub asked_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentFileText {
    pub text: String,
    pub sha256: String,
    pub too_large: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentFiles {
    pub home: Option<String>,
    pub exists: bool,
    pub marker_ok: bool,
    pub instructions: Option<AgentFileText>,
    pub instructions_conflict: bool,
    pub memory: Option<AgentFileText>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ApprovalFacts {
    Read {
        paths: Vec<String>,
    },
    Search {
        paths: Vec<String>,
    },
    Write {
        paths: Vec<String>,
    },
    Delete {
        paths: Vec<String>,
    },
    Shell {
        command: String,
        cwd: Option<String>,
    },
    WebSearch {},
    WebFetch {
        url: String,
        method: String,
    },
    Mcp {
        server: String,
        tool: String,
    },
    /// Harness-internal bookkeeping with no external effect (todo lists, subagent spawn).
    Internal {
        tool: String,
    },
    Unknown {
        tool: String,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentDecisionKind {
    Allowed,
    Asked,
    OwnerAllowed,
    OwnerDenied,
    FailedClosed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentDeliveryOutcome {
    Sent,
    Unsent,
    Unknown,
    Pending,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentDecision {
    pub at_ms: u64,
    pub session_key: String,
    pub approval_id: String,
    pub tool: String,
    pub summary: String,
    pub facts: ApprovalFacts,
    pub decision: AgentDecisionKind,
    pub reason: String,
    pub outcome: AgentDeliveryOutcome,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentWorkspace {
    pub version: u8,
    pub revision: u64,
    pub agents: Vec<AgentProfile>,
    pub assignments: BTreeMap<String, String>,
    pub routines: Vec<AgentRoutine>,
    pub runs: Vec<AgentRun>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub threads: BTreeMap<String, AgentThreadState>,
}
impl Default for AgentWorkspace {
    fn default() -> Self {
        Self {
            version: AGENT_WORKSPACE_VERSION,
            revision: 0,
            agents: Vec::new(),
            assignments: BTreeMap::new(),
            routines: Vec::new(),
            runs: Vec::new(),
            threads: BTreeMap::new(),
        }
    }
}

/// Every `iface:agentControl` answer. Never persisted.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentControlResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work: Option<crate::agent_work::AgentWorkView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar: Option<crate::agent_work::AgentCalendarView>,
    #[serde(flatten)]
    pub workspace: AgentWorkspace,
    pub statuses: BTreeMap<String, AgentStatus>,
    pub pending_approvals: Vec<AgentPendingApproval>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<AgentFiles>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decisions: Option<Vec<AgentDecision>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
    /// Agents whose mode is not `Gated`, by id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub permission_modes: BTreeMap<String, AgentPermissionMode>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AgentCommand {
    Calendar {
        agent_id: String,
        command: crate::agent_work::AgentCalendarCommand,
    },
    Work {
        agent_id: String,
        command: crate::agent_work::AgentWorkCommand,
    },
    Read {},
    ImportLegacy {
        agents: Vec<AgentProfile>,
        assignments: BTreeMap<String, String>,
    },
    SaveAgent {
        expected_revision: u64,
        profile: AgentProfile,
    },
    RemoveAgent {
        expected_revision: u64,
        id: String,
    },
    Assign {
        expected_revision: u64,
        session_key: String,
        agent_id: String,
    },
    SaveRoutine {
        expected_revision: u64,
        routine: AgentRoutine,
    },
    RemoveRoutine {
        expected_revision: u64,
        id: String,
    },
    RunNow {
        expected_revision: u64,
        routine_id: String,
        request_id: String,
    },
    AcknowledgeRun {
        expected_revision: u64,
        id: String,
    },
    /// Host-only. Creates the agent's thread with `text` as its first prompt.
    StartAgentThread {
        agent_id: String,
        text: String,
        request_id: String,
    },
    /// Host-only (protocol 38). The owner's one-off task for the agent: a gated run in `cwd`
    /// with the agent's home and granted folders, its permission mode and its job, launched
    /// at once with `text` as the prompt. `title` names it (the owner's own words); `model` and
    /// `effort` are what the owner picked at launch, for this task only. One `request_id`
    /// launches at most once.
    StartAgentTask {
        agent_id: String,
        text: String,
        cwd: String,
        request_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recovery: Option<crate::recovery::PromptRecoveryInput>,
    },
    NewThread {
        expected_revision: u64,
        agent_id: String,
    },
    MarkRead {
        agent_id: String,
    },
    ReadAgentFiles {
        agent_id: String,
    },
    SaveAgentMemory {
        agent_id: String,
        expected_sha256: String,
        text: String,
    },
    OverwriteAgentInstructionsFile {
        agent_id: String,
        expected_sha256: String,
    },
    SaveAgentFolders {
        expected_revision: u64,
        agent_id: String,
        folders: Vec<String>,
    },
    ReadAgentDecisions {
        agent_id: String,
        limit: u16,
    },
    RecreateAgentHome {
        agent_id: String,
    },
    RevealAgentHome {
        agent_id: String,
    },
    /// Takes effect at the agent's next thread start, routine run or resume.
    SavePermissionMode {
        agent_id: String,
        mode: AgentPermissionMode,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_commands_and_state_round_trip_and_reject_foreign_effect_fields() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/agent-workspace-v14.json"
        ))
        .unwrap();
        for value in fixture["commands"].as_array().unwrap() {
            let command: AgentCommand = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(command).unwrap(), *value);
            let mut invalid = value.clone();
            invalid["executeShell"] = serde_json::json!("not a command field");
            assert!(serde_json::from_value::<AgentCommand>(invalid).is_err());
        }
        let state: AgentWorkspace = serde_json::from_value(fixture["state"].clone()).unwrap();
        assert_eq!(serde_json::to_value(state).unwrap(), fixture["state"]);
    }

    #[test]
    fn v38_agent_task_start_round_trips_and_rejects_unknown_fields() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/agent-task-start-v38.json"
        ))
        .unwrap();
        assert_eq!(fixture["protocolVersion"], 38);
        for value in fixture["commands"].as_array().unwrap() {
            let command: AgentCommand = serde_json::from_value(value.clone()).unwrap();
            assert!(matches!(command, AgentCommand::StartAgentTask { .. }));
            assert_eq!(serde_json::to_value(command).unwrap(), *value);
            // The renderer cannot choose the gate, the folders or the session kind.
            for (field, foreign) in [
                ("approvalGate", serde_json::json!(false)),
                ("additionalDirectories", serde_json::json!(["/"])),
                ("permissionMode", serde_json::json!("bypassPermissions")),
                ("kind2", serde_json::json!("chat")),
            ] {
                let mut invalid = value.clone();
                invalid[field] = foreign;
                assert!(
                    serde_json::from_value::<AgentCommand>(invalid).is_err(),
                    "{field}"
                );
            }
        }
        for missing in ["agentId", "text", "cwd", "requestId"] {
            let mut invalid = fixture["commands"][0].clone();
            invalid.as_object_mut().unwrap().remove(missing);
            assert!(
                serde_json::from_value::<AgentCommand>(invalid).is_err(),
                "{missing}"
            );
        }
    }

    #[test]
    fn v23_fixture_round_trips_commands_state_and_response() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/agent-workspace-v23.json"
        ))
        .unwrap();
        for value in fixture["commands"].as_array().unwrap() {
            let command: AgentCommand = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(command).unwrap(), *value);
        }
        let state: AgentWorkspace = serde_json::from_value(fixture["state"].clone()).unwrap();
        assert_eq!(state.version, AGENT_WORKSPACE_VERSION);
        assert_eq!(serde_json::to_value(&state).unwrap(), fixture["state"]);
        let response: AgentControlResponse =
            serde_json::from_value(fixture["response"].clone()).unwrap();
        assert_eq!(serde_json::to_value(response).unwrap(), fixture["response"]);
    }

    #[test]
    fn approval_facts_reject_foreign_fields() {
        assert!(
            serde_json::from_value::<ApprovalFacts>(
                serde_json::json!({"kind":"shell","command":"ls","cwd":null,"sudo":true})
            )
            .is_err()
        );
    }

    #[test]
    fn v32_permission_modes_round_trip_and_map_to_launch_access() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/agent-permission-modes-v32.json"
        ))
        .unwrap();
        for value in fixture["commands"].as_array().unwrap() {
            let command: AgentCommand = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(command).unwrap(), *value);
        }
        let response: AgentControlResponse =
            serde_json::from_value(fixture["response"].clone()).unwrap();
        assert_eq!(
            response.permission_modes.get("research"),
            Some(&AgentPermissionMode::Bypass)
        );
        assert_eq!(serde_json::to_value(response).unwrap(), fixture["response"]);
        assert!(
            serde_json::from_value::<AgentCommand>(serde_json::json!(
                {"kind":"savePermissionMode","agentId":"research","mode":"yolo"}
            ))
            .is_err()
        );
        // An empty map stays off the wire, so older readers see today's response.
        let v23: serde_json::Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/agent-workspace-v23.json"
        ))
        .unwrap();
        let plain: AgentControlResponse = serde_json::from_value(v23["response"].clone()).unwrap();
        assert!(plain.permission_modes.is_empty());
        assert!(
            serde_json::to_value(plain)
                .unwrap()
                .get("permissionModes")
                .is_none()
        );
        assert_eq!(AgentPermissionMode::default(), AgentPermissionMode::Gated);
        assert_eq!(AgentPermissionMode::Gated.access(), None);
        assert_eq!(
            AgentPermissionMode::Auto.access(),
            Some(crate::engine::AgentAccess::Auto)
        );
        assert_eq!(
            AgentPermissionMode::Bypass.access(),
            Some(crate::engine::AgentAccess::Bypass)
        );
    }
}
