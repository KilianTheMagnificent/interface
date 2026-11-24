//! Saved, explicitly approved workspace preparation and reusable launch drafts.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectSetup {
    /// Shell commands the owner explicitly saved. Never inferred from a repository file.
    pub prerequisites: Vec<String>,
    pub commands: Vec<String>,
    pub timeout_seconds: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipeContext {
    pub kind: RecipeContextKind,
    /// Project-relative file/folder, or an HTTPS URL. No cached bytes or credentials.
    pub value: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RecipeContextKind {
    File,
    Folder,
    Url,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowRecipe {
    pub name: String,
    pub prompt: String,
    pub engine: String,
    pub model: String,
    pub context: Vec<RecipeContext>,
    pub new_worktree: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_ref: Option<String>,
    pub setup: ProjectSetup,
    pub expected_outputs: Vec<String>,
    /// Instructions for the agent, not a claim these checks ran.
    pub checks: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SetupState {
    Running,
    Ready,
    Failed,
    Interrupted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetupStep {
    pub command: String,
    pub prerequisite: bool,
    pub output: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetupReport {
    pub cwd: String,
    pub state: SetupState,
    pub steps: Vec<SetupStep>,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecipeDraft {
    pub recipe: WorkflowRecipe,
    pub prompt: String,
    /// Revalidated descriptors; the ordinary launch path still authorizes their reads.
    pub context: Vec<RecipeContext>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowView {
    pub project_root: String,
    pub setup: ProjectSetup,
    pub recipes: Vec<WorkflowRecipe>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_setup: Option<SetupReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<RecipeDraft>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum WorkflowCommand {
    Read {
        project_root: String,
    },
    RetrySetup {
        project_root: String,
        cwd: String,
    },
    SaveSetup {
        project_root: String,
        setup: ProjectSetup,
    },
    SaveRecipe {
        project_root: String,
        recipe: WorkflowRecipe,
    },
    DeleteRecipe {
        project_root: String,
        name: String,
    },
    /// Explicitly applies the recipe's saved setup and returns a draft. Does not launch.
    UseRecipe {
        project_root: String,
        name: String,
    },
}

impl WorkflowCommand {
    #[must_use]
    pub fn project_root(&self) -> &str {
        match self {
            Self::Read { project_root }
            | Self::RetrySetup { project_root, .. }
            | Self::SaveSetup { project_root, .. }
            | Self::SaveRecipe { project_root, .. }
            | Self::DeleteRecipe { project_root, .. }
            | Self::UseRecipe { project_root, .. } => project_root,
        }
    }
}

impl Default for ProjectSetup {
    fn default() -> Self {
        Self {
            prerequisites: Vec::new(),
            commands: Vec::new(),
            timeout_seconds: 120,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    #[test]
    fn shared_v49_workflows_round_trip_and_reject_nested_unknown_fields() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../electron/src/fixtures/workflows-v49.json"
        ))
        .unwrap();
        assert_eq!(fixture["protocolVersion"], 49);
        for command in fixture["commands"].as_array().unwrap() {
            let decoded: crate::threads::ThreadCommand =
                serde_json::from_value(command.clone()).unwrap();
            assert_eq!(serde_json::to_value(decoded).unwrap(), *command);
            let mut extra = command.clone();
            extra["command"]["unexpected"] = Value::Bool(true);
            assert!(serde_json::from_value::<crate::threads::ThreadCommand>(extra).is_err());
        }
        let view: WorkflowView = serde_json::from_value(fixture["workflow"].clone()).unwrap();
        assert_eq!(serde_json::to_value(view).unwrap(), fixture["workflow"]);
        for pointer in [
            "",
            "/setup",
            "/recipes/0",
            "/recipes/0/context/0",
            "/lastSetup",
            "/lastSetup/steps/0",
            "/draft",
            "/draft/context/0",
        ] {
            let mut extra = fixture["workflow"].clone();
            extra
                .pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("unexpected".into(), Value::Bool(true));
            assert!(
                serde_json::from_value::<WorkflowView>(extra).is_err(),
                "{pointer}"
            );
        }
        for report in fixture["reports"].as_array().unwrap() {
            let decoded: SetupReport = serde_json::from_value(report.clone()).unwrap();
            assert_eq!(serde_json::to_value(decoded).unwrap(), *report);
        }
    }
}
