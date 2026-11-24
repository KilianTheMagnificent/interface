//! Owner-approved project setup and recipes, independent of frozen thread schemas.
use crate::{SettingsError, leased::LeasedDocument};
use interface_contracts::workflows::{
    ProjectSetup, RecipeContextKind, RecipeDraft, SetupReport, SetupState, WorkflowCommand,
    WorkflowRecipe, WorkflowView,
};
use interface_storage::JsonLimits;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Component, Path},
};

pub const WORKFLOWS_FILE: &str = "workflows.v1.json";
const MAX_PROJECTS: usize = 128;
const MAX_RECIPES: usize = 32;

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Project {
    setup: ProjectSetup,
    recipes: Vec<WorkflowRecipe>,
    setups: Vec<SetupReport>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct State {
    version: u8,
    projects: BTreeMap<String, Project>,
}

pub struct WorkflowStore {
    document: LeasedDocument,
    state: State,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SetupPurpose {
    /// Git just created this workspace; a completed report can belong to an older directory.
    Created,
    /// An explicit retry of a retained failure or interruption.
    Retry,
}

fn invalid(message: &str) -> SettingsError {
    SettingsError::InvalidAgent(message.to_owned())
}
fn text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit && !value.contains('\0')
}
fn root_valid(root: &str) -> bool {
    text(root, 4096) && Path::new(root).is_absolute()
}

/// # Errors
/// Refuses an unbounded command list or deadline. Only a saved configuration is executable.
pub fn validate_setup(setup: &ProjectSetup) -> Result<(), SettingsError> {
    if !(1..=600).contains(&setup.timeout_seconds)
        || setup.commands.len() + setup.prerequisites.len() > 12
        || setup
            .commands
            .iter()
            .chain(&setup.prerequisites)
            .any(|command| !text(command, 4096))
    {
        return Err(invalid(
            "Use at most 12 setup/check commands, each below 4096 bytes, and a 1–600 second total deadline.",
        ));
    }
    Ok(())
}

fn relative_path(value: &str) -> bool {
    text(value, 4096)
        && Path::new(value)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        && !value
            .split('/')
            .any(|part| interface_common::paths::is_sensitive_name(&part.to_ascii_lowercase()))
}

fn recipe_valid(recipe: &WorkflowRecipe) -> bool {
    text(&recipe.name, 100)
        && text(&recipe.prompt, 64 * 1024)
        && text(&recipe.engine, 100)
        && text(&recipe.model, 256)
        && recipe.context.len() <= 24
        && recipe.context.iter().all(|item| match item.kind {
            RecipeContextKind::File | RecipeContextKind::Folder => relative_path(&item.value),
            RecipeContextKind::Url => {
                text(&item.value, 4096)
                    && item.value.starts_with("https://")
                    && !item.value.contains('@')
            }
        })
        && recipe
            .base_ref
            .as_ref()
            .is_none_or(|value| text(value, 512))
        && validate_setup(&recipe.setup).is_ok()
        && [&recipe.expected_outputs, &recipe.checks]
            .into_iter()
            .all(|values| values.len() <= 32 && values.iter().all(|value| text(value, 4096)))
}

fn requirement_suffix(recipe: &WorkflowRecipe) -> String {
    let mut suffix = String::new();
    for (heading, values) in [
        ("Expected outputs", &recipe.expected_outputs),
        ("Required checks", &recipe.checks),
    ] {
        if !values.is_empty() {
            suffix.push_str("\n\n");
            suffix.push_str(heading);
            suffix.push_str(":\n- ");
            suffix.push_str(&values.join("\n- "));
        }
    }
    suffix
}

fn normalize_recipe_prompt(recipe: &mut WorkflowRecipe, saved_recipes: &[WorkflowRecipe]) {
    if let Some(original) = saved_recipes
        .iter()
        .find(|saved| recipe.prompt == format!("{}{}", saved.prompt, requirement_suffix(saved)))
    {
        recipe.prompt.clone_from(&original.prompt);
    }
    if let Some(original) = saved_recipes.iter().find(|saved| saved.name == recipe.name) {
        // The owner may edit the prompt prefix and requirements together.
        // Remove only this recipe's known generated trailer before replacing it.
        let previous_suffix = requirement_suffix(original);
        if !previous_suffix.is_empty() && recipe.prompt.ends_with(&previous_suffix) {
            recipe
                .prompt
                .truncate(recipe.prompt.len() - previous_suffix.len());
        }
    }
    let suffix = requirement_suffix(recipe);
    if !suffix.is_empty() && recipe.prompt.ends_with(&suffix) {
        recipe.prompt.truncate(recipe.prompt.len() - suffix.len());
    }
}

fn state_valid(state: &State) -> bool {
    state.version == 1
        && state.projects.len() <= MAX_PROJECTS
        && state.projects.iter().all(|(root, project)| {
            root_valid(root)
                && validate_setup(&project.setup).is_ok()
                && project.recipes.len() <= MAX_RECIPES
                && project.recipes.iter().all(recipe_valid)
                && project.setups.len() <= 64
                && project.setups.iter().all(|report| {
                    root_valid(&report.cwd)
                        && report.steps.len() <= 12
                        && report.message.len() <= 4096
                        && report
                            .steps
                            .iter()
                            .all(|step| text(&step.command, 4096) && step.output.len() <= 32 * 1024)
                })
        })
}

impl WorkflowStore {
    /// # Errors
    /// Unsafe, corrupt, oversized, or already leased storage refuses all workflow effects.
    pub fn open(file: &Path) -> Result<Self, SettingsError> {
        // Never turn an unreadable saved command into silently successful no-setup.
        let (document, state) = LeasedDocument::open::<State>(
            file,
            ".workflows.lock",
            JsonLimits::new(8 * 1024 * 1024, 12, 100_000)?,
            "Workflow storage is uncertain. Restart Interface before using setup or recipes.",
        )?;
        let mut state = state.unwrap_or(State {
            version: 1,
            projects: BTreeMap::new(),
        });
        if !state_valid(&state) {
            return Err(invalid(
                "The saved workflow document is invalid; repair it before running setup.",
            ));
        }
        let mut interrupted = false;
        for project in state.projects.values_mut() {
            for report in &mut project.setups {
                if report.state == SetupState::Running {
                    report.state = SetupState::Interrupted;
                    report.message = "Interface stopped during setup. The commands may have changed this worktree; inspect it before starting new work. Setup was not replayed.".into();
                    interrupted = true;
                }
            }
        }
        let mut store = Self { document, state };
        if interrupted {
            store.document.commit(&store.state)?;
        }
        Ok(store)
    }

    #[must_use]
    pub fn view(&self, root: &str) -> WorkflowView {
        let project = self.state.projects.get(root).cloned().unwrap_or_default();
        WorkflowView {
            project_root: root.into(),
            setup: project.setup,
            recipes: project.recipes,
            last_setup: project.setups.last().cloned(),
            draft: None,
        }
    }

    /// # Errors
    /// Invalid input or uncertain storage never replaces the saved configuration.
    pub fn command(&mut self, command: WorkflowCommand) -> Result<WorkflowView, SettingsError> {
        let root = command.project_root().to_owned();
        if !root_valid(&root) {
            return Err(invalid("Choose an absolute project folder."));
        }
        if matches!(command, WorkflowCommand::Read { .. }) {
            return Ok(self.view(&root));
        }
        self.document.ensure_writable()?;
        let mut next = self.state.clone();
        if next.projects.len() >= MAX_PROJECTS && !next.projects.contains_key(&root) {
            return Err(invalid("The saved workflow project limit was reached."));
        }
        let project = next.projects.entry(root.clone()).or_default();
        let mut draft = None;
        match command {
            WorkflowCommand::Read { .. } => unreachable!(),
            WorkflowCommand::RetrySetup { .. } => {
                return Err(invalid("Setup must run through the workspace executor."));
            }
            WorkflowCommand::SaveSetup { setup, .. } => {
                validate_setup(&setup)?;
                project.setup = setup;
            }
            WorkflowCommand::SaveRecipe { mut recipe, .. } => {
                for item in &mut recipe.context {
                    if item.kind != RecipeContextKind::Url && Path::new(&item.value).is_absolute() {
                        item.value = Path::new(&item.value).strip_prefix(&root)
                            .map_err(|_| invalid("A recipe can attach only files and folders within its project."))?
                            .to_string_lossy().into_owned();
                    }
                }
                normalize_recipe_prompt(&mut recipe, &project.recipes);
                if !recipe_valid(&recipe) {
                    return Err(invalid(
                        "Use a name, prompt, provider/model, project-relative file/folder context or HTTPS URLs, and bounded outputs/checks.",
                    ));
                }
                let old = project
                    .recipes
                    .iter()
                    .position(|entry| entry.name == recipe.name);
                if let Some(index) = old {
                    project.recipes[index] = recipe;
                } else if project.recipes.len() < MAX_RECIPES {
                    project.recipes.push(recipe);
                } else {
                    return Err(invalid(
                        "This project already has 32 recipes; delete one first.",
                    ));
                }
            }
            WorkflowCommand::DeleteRecipe { name, .. } => {
                project.recipes.retain(|recipe| recipe.name != name);
            }
            WorkflowCommand::UseRecipe { name, .. } => {
                let recipe = project
                    .recipes
                    .iter()
                    .find(|recipe| recipe.name == name)
                    .ok_or_else(|| invalid("This recipe no longer exists."))?
                    .clone();
                project.setup = recipe.setup.clone();
                let prompt = format!("{}{}", recipe.prompt, requirement_suffix(&recipe));
                let mut context = recipe.context.clone();
                for item in &mut context {
                    if item.kind != RecipeContextKind::Url {
                        item.value = Path::new(&root)
                            .join(&item.value)
                            .to_string_lossy()
                            .into_owned();
                    }
                }
                draft = Some(RecipeDraft {
                    recipe,
                    prompt,
                    context,
                });
            }
        }
        self.document.commit(&next)?;
        self.state = next;
        let mut view = self.view(&root);
        view.draft = draft;
        Ok(view)
    }

    #[must_use]
    pub fn setup_for(&self, root: &str, cwd: &str) -> Option<SetupReport> {
        self.state
            .projects
            .get(root)?
            .setups
            .iter()
            .find(|report| report.cwd == cwd)
            .cloned()
    }

    /// # Errors
    /// Only an explicitly retained failure/interruption can be run again.
    pub fn ensure_retry(&self, root: &str, cwd: &str) -> Result<(), SettingsError> {
        if self.setup_for(root, cwd).is_some_and(|report| {
            matches!(report.state, SetupState::Failed | SetupState::Interrupted)
        }) {
            Ok(())
        } else {
            Err(invalid(
                "Only a failed or interrupted setup can be run again. A ready or running worktree is left alone.",
            ))
        }
    }

    /// Reserve a setup effect durably before any subprocess runs.
    /// # Errors
    /// No effects are allowed when the reserve cannot be committed.
    pub fn begin_setup(
        &mut self,
        root: &str,
        cwd: &str,
        purpose: SetupPurpose,
    ) -> Result<ProjectSetup, SettingsError> {
        if !root_valid(root) || !root_valid(cwd) {
            return Err(invalid("The setup folders are invalid."));
        }
        if purpose == SetupPurpose::Retry {
            // This check and the reservation share the store's exclusive access.
            // An earlier preflight cannot authorize a retry after another attempt finishes.
            self.ensure_retry(root, cwd)?;
        }
        if self
            .setup_for(root, cwd)
            .is_some_and(|report| report.state == SetupState::Running)
        {
            return Err(invalid("Setup is already running in this worktree."));
        }
        let setup = self.view(root).setup;
        if setup.commands.is_empty() && setup.prerequisites.is_empty() {
            return Ok(setup);
        }
        self.finish_setup(
            root,
            SetupReport {
                cwd: cwd.into(),
                state: SetupState::Running,
                steps: Vec::new(),
                message: "Checking prerequisites and preparing the new worktree…".into(),
            },
        )?;
        Ok(setup)
    }

    /// # Errors
    /// An uncertain result write cannot authorize a provider launch.
    pub fn finish_setup(&mut self, root: &str, report: SetupReport) -> Result<(), SettingsError> {
        let mut next = self.state.clone();
        let project = next.projects.entry(root.into()).or_default();
        project.setups.retain(|old| old.cwd != report.cwd);
        if project.setups.len() >= 64 {
            if let Some(index) = project
                .setups
                .iter()
                .position(|old| old.state == SetupState::Ready)
            {
                project.setups.remove(index);
            } else {
                return Err(invalid(
                    "Too many unresolved workspace setups. Resolve a failed setup before creating another.",
                ));
            }
        }
        project.setups.push(report);
        if !state_valid(&next) {
            return Err(invalid("The setup report exceeded its bounds."));
        }
        self.document.commit(&next)?;
        self.state = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use interface_contracts::workflows::RecipeContext;
    fn recipe() -> WorkflowRecipe {
        WorkflowRecipe {
            name: "Review".into(),
            prompt: "Review the changes".into(),
            engine: "codex".into(),
            model: "gpt-6".into(),
            context: vec![RecipeContext {
                kind: RecipeContextKind::File,
                value: "/project/src/lib.rs".into(),
            }],
            new_worktree: true,
            base_ref: Some("main".into()),
            setup: ProjectSetup {
                commands: vec!["npm ci".into()],
                ..ProjectSetup::default()
            },
            expected_outputs: vec!["review.md".into()],
            checks: vec!["npm test".into()],
        }
    }
    #[test]
    fn recipes_reopen_restore_context_and_require_explicit_use_before_setup_changes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(WORKFLOWS_FILE);
        let mut store = WorkflowStore::open(&file).unwrap();
        store
            .command(WorkflowCommand::SaveRecipe {
                project_root: "/project".into(),
                recipe: recipe(),
            })
            .unwrap();
        assert!(store.view("/project").setup.commands.is_empty());
        drop(store);
        let mut store = WorkflowStore::open(&file).unwrap();
        let view = store
            .command(WorkflowCommand::UseRecipe {
                project_root: "/project".into(),
                name: "Review".into(),
            })
            .unwrap();
        assert_eq!(view.setup.commands, ["npm ci"]);
        let draft = view.draft.unwrap();
        assert_eq!(draft.context[0].value, "/project/src/lib.rs");
        assert_eq!(draft.recipe.context[0].value, "src/lib.rs");
        assert!(draft.prompt.contains("Expected outputs:\n- review.md"));
        assert!(draft.prompt.contains("Required checks:\n- npm test"));
        assert_eq!(draft.recipe.engine, "codex");
        assert_eq!(draft.recipe.model, "gpt-6");
    }
    #[test]
    fn editing_a_loaded_recipe_replaces_old_generated_requirements_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = WorkflowStore::open(&dir.path().join(WORKFLOWS_FILE)).unwrap();
        store
            .command(WorkflowCommand::SaveRecipe {
                project_root: "/project".into(),
                recipe: recipe(),
            })
            .unwrap();
        let draft = store
            .command(WorkflowCommand::UseRecipe {
                project_root: "/project".into(),
                name: "Review".into(),
            })
            .unwrap()
            .draft
            .unwrap();
        let mut edited = draft.recipe;
        edited.prompt =
            draft
                .prompt
                .replacen("Review the changes", "Review the API changes carefully", 1);
        edited.expected_outputs = vec!["api-review.md".into()];
        edited.checks = vec!["cargo test".into()];
        store
            .command(WorkflowCommand::SaveRecipe {
                project_root: "/project".into(),
                recipe: edited,
            })
            .unwrap();
        let reused = store
            .command(WorkflowCommand::UseRecipe {
                project_root: "/project".into(),
                name: "Review".into(),
            })
            .unwrap()
            .draft
            .unwrap();
        assert_eq!(reused.recipe.prompt, "Review the API changes carefully");
        assert_eq!(
            reused.prompt,
            "Review the API changes carefully\n\nExpected outputs:\n- api-review.md\n\nRequired checks:\n- cargo test"
        );
    }

    #[test]
    fn setup_retry_rechecks_readiness_when_reserving_after_a_concurrent_completion() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = WorkflowStore::open(&dir.path().join(WORKFLOWS_FILE)).unwrap();
        store
            .command(WorkflowCommand::SaveSetup {
                project_root: "/project".into(),
                setup: recipe().setup,
            })
            .unwrap();
        let report = |state| SetupReport {
            cwd: "/worktree/one".into(),
            state,
            steps: vec![],
            message: "result".into(),
        };
        store
            .finish_setup("/project", report(SetupState::Failed))
            .unwrap();
        // Two callers can both pass the read-only preflight before Git verification.
        store.ensure_retry("/project", "/worktree/one").unwrap();
        store.ensure_retry("/project", "/worktree/one").unwrap();
        store
            .begin_setup("/project", "/worktree/one", SetupPurpose::Retry)
            .unwrap();
        assert!(
            store
                .begin_setup("/project", "/worktree/one", SetupPurpose::Retry)
                .is_err()
        );
        store
            .finish_setup("/project", report(SetupState::Ready))
            .unwrap();
        // The second caller cannot replay setup even after the first has completed.
        assert!(
            store
                .begin_setup("/project", "/worktree/one", SetupPurpose::Retry)
                .is_err()
        );
        assert_eq!(
            store.setup_for("/project", "/worktree/one").unwrap().state,
            SetupState::Ready
        );
    }

    #[test]
    fn created_worktree_replaces_old_ready_report_but_never_running_setup() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = WorkflowStore::open(&dir.path().join(WORKFLOWS_FILE)).unwrap();
        store
            .command(WorkflowCommand::SaveSetup {
                project_root: "/project".into(),
                setup: recipe().setup,
            })
            .unwrap();
        store
            .finish_setup(
                "/project",
                SetupReport {
                    cwd: "/worktree/recreated".into(),
                    state: SetupState::Ready,
                    steps: vec![],
                    message: "The old directory was ready".into(),
                },
            )
            .unwrap();
        assert!(
            store
                .begin_setup("/project", "/worktree/recreated", SetupPurpose::Retry)
                .is_err()
        );
        let setup = store
            .begin_setup("/project", "/worktree/recreated", SetupPurpose::Created)
            .unwrap();
        assert_eq!(setup.commands, ["npm ci"]);
        assert_eq!(
            store
                .setup_for("/project", "/worktree/recreated")
                .unwrap()
                .state,
            SetupState::Running
        );
        for purpose in [SetupPurpose::Created, SetupPurpose::Retry] {
            assert!(
                store
                    .begin_setup("/project", "/worktree/recreated", purpose)
                    .is_err(),
                "Neither creation nor retry may overtake an active setup"
            );
        }
    }

    #[test]
    fn setup_reserve_survives_restart_as_interrupted_without_replay() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(WORKFLOWS_FILE);
        let mut store = WorkflowStore::open(&file).unwrap();
        store
            .command(WorkflowCommand::SaveSetup {
                project_root: "/project".into(),
                setup: recipe().setup,
            })
            .unwrap();
        store
            .begin_setup("/project", "/worktree/one", SetupPurpose::Created)
            .unwrap();
        assert!(
            store
                .begin_setup("/project", "/worktree/one", SetupPurpose::Created)
                .is_err()
        );
        drop(store);
        let store = WorkflowStore::open(&file).unwrap();
        let report = store.setup_for("/project", "/worktree/one").unwrap();
        assert_eq!(report.state, SetupState::Interrupted);
        assert!(report.steps.is_empty());
        assert!(report.message.contains("not replayed"));
        assert!(WorkflowStore::open(&file).is_err(), "one writer");
    }
    #[test]
    fn recipe_context_cannot_escape_project_or_store_temporary_secret_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = WorkflowStore::open(&dir.path().join(WORKFLOWS_FILE)).unwrap();
        for path in [
            "/outside/file",
            "../secret",
            ".env",
            ".env.local",
            ".ssh/id_rsa",
        ] {
            let mut recipe = recipe();
            recipe.context[0].value = path.into();
            assert!(
                store
                    .command(WorkflowCommand::SaveRecipe {
                        project_root: "/project".into(),
                        recipe
                    })
                    .is_err(),
                "{path}"
            );
        }
        assert!(store.view("/project").recipes.is_empty());
    }
    #[test]
    fn corrupt_workflows_fail_closed_instead_of_silently_skipping_saved_setup() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(WORKFLOWS_FILE);
        std::fs::write(&file, b"{broken").unwrap();
        assert!(WorkflowStore::open(&file).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"{broken");
    }
    #[test]
    fn failed_setup_remains_gated_after_another_worktree_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = WorkflowStore::open(&dir.path().join(WORKFLOWS_FILE)).unwrap();
        for (cwd, state) in [
            ("/worktree/failed", SetupState::Failed),
            ("/worktree/ready", SetupState::Ready),
        ] {
            store
                .finish_setup(
                    "/project",
                    SetupReport {
                        cwd: cwd.into(),
                        state,
                        steps: vec![],
                        message: "result".into(),
                    },
                )
                .unwrap();
        }
        assert_eq!(
            store.view("/project").last_setup.unwrap().state,
            SetupState::Ready
        );
        assert_eq!(
            store
                .setup_for("/project", "/worktree/failed")
                .unwrap()
                .state,
            SetupState::Failed
        );
    }
}
