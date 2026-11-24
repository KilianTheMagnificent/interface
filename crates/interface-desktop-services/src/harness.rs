//! Harness operations over the existing workspace authority and review services.
use super::{DesktopService, DesktopServiceError, agent_now_ms, encode, map_settings_error};
use interface_contracts::{
    harness::{HarnessCommand, HarnessState, TaskEvidence},
    ipc::RepositoryReview,
};
use interface_review::{RepositoryReviewOptions, read_repository_review_with};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// The command catalog every harness reply carries. It is static, so it is
/// encoded once per process.
static HARNESS_CATALOG: std::sync::LazyLock<Value> =
    std::sync::LazyLock::new(|| json!(interface_contracts::command_catalog::command_catalog()));

impl DesktopService {
    fn with_harness<T>(
        &self,
        operation: impl FnOnce(
            &mut interface_settings::HarnessStore,
        ) -> Result<T, interface_settings::SettingsError>,
    ) -> Result<T, DesktopServiceError> {
        let mut guard = self
            .harness
            .lock()
            .map_err(|_| failure("Harness storage lock failed."))?;
        let store = guard.as_mut().ok_or_else(|| {
            failure("Harness history is unavailable; existing data was preserved.")
        })?;
        operation(store).map_err(map_settings_error)
    }
    #[allow(clippy::too_many_lines)]
    pub(super) fn harness_command(
        &self,
        payload: Value,
        session_roots: &[PathBuf],
    ) -> Result<Value, DesktopServiceError> {
        let command: HarnessCommand =
            serde_json::from_value(payload).map_err(|_| failure("Invalid harness command."))?;
        let mut detail = json!({});
        let state = match command {
            HarnessCommand::Read {} | HarnessCommand::Catalog {} => {
                self.with_harness(|store| Ok(store.snapshot()))?
            }
            HarnessCommand::RecordBranch { branch } => {
                self.harness_root(&branch.root, session_roots)?;
                self.with_harness(|store| store.record_branch(branch))?
            }
            HarnessCommand::ReadDraft { target } => {
                let state = self.harness_snapshot()?;
                detail["draft"] = json!(
                    state
                        .drafts
                        .iter()
                        .find(|draft| draft.target == target)
                        .cloned()
                        .unwrap_or(interface_contracts::harness::SharedDraft {
                            target,
                            revision: 0,
                            text: String::new()
                        })
                );
                state
            }
            HarnessCommand::SaveDraft {
                target,
                expected_revision,
                text,
            } => {
                let state = self.harness_save_draft(target.clone(), expected_revision, text)?;
                detail["draft"] = json!(state.drafts.iter().find(|draft| draft.target == target));
                state
            }
            HarnessCommand::RecordAttention { item } => {
                self.with_harness(|store| store.record(item))?
            }
            HarnessCommand::MarkSeen { ids } => self.with_harness(|store| store.seen(&ids))?,
            HarnessCommand::Resolve { id, target } => {
                self.with_harness(|store| store.resolve(&id, &target))?
            }
            HarnessCommand::CaptureResult {
                id,
                objective,
                target,
                root,
            } => {
                if id.len() > 512 || objective.len() > 8000 {
                    return Err(failure("Result fields exceed their limits."));
                }
                let root = self.harness_root(&root, session_roots)?;
                let (review, candidate, findings) = self.harness_candidate(&root, session_roots)?;
                let task = TaskEvidence {
                    id,
                    objective,
                    target,
                    root: root.to_string_lossy().into_owned(),
                    baseline: review.head.clone().unwrap_or_else(|| "not reported".into()),
                    candidate,
                    changed_paths: review
                        .files
                        .iter()
                        .filter(|file| file.omitted.is_none())
                        .map(|file| file.path.clone())
                        .collect(),
                    checks: vec![],
                    check_candidate: None,
                    findings,
                    state: "captured; inspect required".into(),
                    at: agent_now_ms(),
                };
                detail = json!({"result":task,"review":review,"stale":false,"checks":interface_review::discover_review_checks(&root).ok()});
                self.with_harness(|store| store.evidence(task))?
            }
            HarnessCommand::RunChecks { id, selected } => {
                if selected.is_empty() || selected.len() > 5 {
                    return Err(failure("Select one to five discovered project checks."));
                }
                let state = self.with_harness(|store| Ok(store.snapshot()))?;
                let original = state
                    .tasks
                    .iter()
                    .find(|task| task.id == id)
                    .ok_or_else(|| failure("Result no longer available."))?;
                let root = self.harness_root(&original.root, session_roots)?;
                let (_, before, _) = self.harness_candidate(&root, session_roots)?;
                if before.starts_with("partial:") {
                    return Err(failure(
                        "Candidate coverage is incomplete. Inspect omissions before running checks.",
                    ));
                }
                if before != original.candidate {
                    return Err(failure(
                        "Files changed. Capture a fresh result before running its checks.",
                    ));
                }
                let checks = interface_review::run_review_checks(
                    &root,
                    &selected,
                    &interface_review::ReviewCheckRunOptions {
                        runner: self.review.check_runner.as_deref(),
                        ..Default::default()
                    },
                )
                .map_err(|_| failure("Project checks could not run."))?;
                let (review, after, _) = self.harness_candidate(&root, session_roots)?;
                let mut task = original.clone();
                task.id = uuid::Uuid::new_v4().to_string();
                task.at = agent_now_ms();
                task.checks = checks;
                task.check_candidate = Some(before.clone());
                task.state = if before == after {
                    "checks recorded; inspect required"
                } else {
                    "files changed during checks; evidence is stale"
                }
                .into();
                detail = json!({"result":task,"review":review,"stale":before != after});
                self.with_harness(|store| store.evidence(task))?
            }
            HarnessCommand::InspectResult { id } => {
                let state = self.with_harness(|store| Ok(store.snapshot()))?;
                let task = state
                    .tasks
                    .iter()
                    .find(|task| task.id == id)
                    .ok_or_else(|| failure("Result no longer available."))?;
                let root = self.harness_root(&task.root, session_roots)?;
                let (review, candidate, _) = self.harness_candidate(&root, session_roots)?;
                detail = json!({"result":task,"review":review,"stale":task.candidate != candidate,"checks":interface_review::discover_review_checks(&root).ok()});
                state
            }
            HarnessCommand::Context { root, query } => {
                if query.len() > 512 {
                    return Err(failure("Context query is too long."));
                }
                let root = self.harness_root(&root, session_roots)?;
                let authority = self.workspace_authority_snapshot(session_roots);
                let inventory = interface_workspace::read_customization_inventory(
                    &root,
                    self.customization_user_root.as_deref(),
                );
                let routes = authority
                    .read_text(&root.join("docs/agent-map.json"))
                    .ok()
                    .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                    .and_then(|value| value.get("routes").and_then(Value::as_array).cloned())
                    .unwrap_or_default();
                let terms = query
                    .split_whitespace()
                    .map(str::to_lowercase)
                    .collect::<Vec<_>>();
                let mut suggestions: Vec<String> = Vec::new();
                'routes: for route in routes.iter().take(256) {
                    let searchable =
                        format!("{} {} {}", route["id"], route["summary"], route["keywords"])
                            .to_lowercase();
                    if !terms.is_empty() && !terms.iter().any(|term| searchable.contains(term)) {
                        continue;
                    }
                    for path in route["entrypoints"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                    {
                        if suggestions.len() == 20 {
                            break 'routes;
                        }
                        // The map is untrusted project data. Every suggested file uses the ordinary
                        // read policy; the cheap checks run first so a full list reads nothing more.
                        if safe_route_path(path)
                            && !suggestions.iter().any(|suggested| suggested == path)
                            && authority.read_text(&root.join(path)).is_ok()
                        {
                            suggestions.push(path.to_owned());
                        }
                    }
                }
                detail = json!({"context":{"root":root,"inventory":inventory,"suggestions":suggestions,"usage":"Provider usage is shown separately when reported.","omissions":["Instruction discovery does not prove a provider loaded a source.","File suggestions contain paths only; attach explicitly to include content."]}});
                self.with_harness(|store| Ok(store.snapshot()))?
            }
            HarnessCommand::Presets { root } => {
                let root = self.harness_root(&root, session_roots)?;
                let checks = interface_review::discover_review_checks(&root)
                    .map_err(|_| failure("Could not discover project checks."))?;
                detail = json!({"presets":[
                    {"id":"inspect","label":"Local checkout · inspect","root":root,"isolate":false,"setup":"No setup required for read-only inspection"},
                    {"id":"implement","label":"New worktree · implement","root":root,"isolate":true,"setup":"Create a separate worktree before launching"},
                    {"id":"review","label":"Existing worktree · review","root":root,"isolate":false,"setup":"Use the selected existing worktree; do not rerun setup"}
                ],"checks":checks});
                self.with_harness(|store| Ok(store.snapshot()))?
            }
        };
        detail["version"] = json!(1);
        detail["state"] = encode("harness", state)?;
        detail["catalog"] = HARNESS_CATALOG.clone();
        Ok(detail)
    }
    fn harness_root(
        &self,
        root: &str,
        session_roots: &[PathBuf],
    ) -> Result<PathBuf, DesktopServiceError> {
        let authority = self.workspace_authority_snapshot(session_roots);
        Self::exact_authorized_root(&authority, root, "inspect task context")
    }
    fn harness_candidate(
        &self,
        root: &Path,
        session_roots: &[PathBuf],
    ) -> Result<(RepositoryReview, String, Vec<String>), DesktopServiceError> {
        let review = read_repository_review_with(
            root,
            &RepositoryReviewOptions {
                runner: self.review.git_runner.as_deref(),
                ..RepositoryReviewOptions::default()
            },
        );
        if !review.repository || review.error.is_some() {
            return Err(failure(
                "Repository snapshot is unavailable; no result was recorded.",
            ));
        }
        let authority = self.workspace_authority_snapshot(session_roots);
        let mut partial =
            review.truncated || review.metadata_complete == Some(false) || review.head.is_none();
        let mut hashes = vec![review.head.clone().unwrap_or_default()];
        let mut findings = vec!["Snapshot includes checkout changes, which may include other contributors' edits. It does not attribute authorship.".into()];
        for file in &review.files {
            if file.omitted.is_some() {
                partial = true;
                findings.push(format!("Omitted file: {}", file.path));
                continue;
            }
            let path = root.join(&file.path);
            let content = authority.read_snapshot(&path).ok();
            if content.is_none()
                && file.status != interface_contracts::ipc::RepositoryReviewFileStatus::Deleted
            {
                partial = true;
            }
            // Include full authorized bytes through the existing snapshot hash, not a truncated diff hash.
            hashes.push(format!(
                "{}:{:?}:{}",
                file.path,
                file.states,
                content.map_or_else(
                    || "unavailable-or-deleted".into(),
                    |snapshot| snapshot.revision
                )
            ));
        }
        if review.truncated {
            findings.push(
                "File listing was truncated; this result does not cover the entire checkout."
                    .into(),
            );
        }
        hashes.sort();
        let digest = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&hashes)
                    .map_err(|_| failure("Could not bind candidate evidence."))?
            )
        );
        if partial {
            findings.push(
                "Candidate coverage is incomplete; checks cannot validate this snapshot.".into(),
            );
        }
        Ok((
            review,
            if partial {
                format!("partial:{digest}")
            } else {
                digest
            },
            findings,
        ))
    }
    /// Persists only a draft, with compare-and-set protection. Never sends provider input.
    /// # Errors
    /// Refuses unavailable storage or a conflicting draft revision.
    pub fn harness_save_draft(
        &self,
        target: interface_contracts::harness::AttentionTarget,
        revision: u64,
        text: String,
    ) -> Result<HarnessState, DesktopServiceError> {
        self.with_harness(|store| store.save_draft(target, revision, text))
    }
    /// # Errors
    /// Refuses unavailable durable storage.
    pub fn harness_snapshot(&self) -> Result<HarnessState, DesktopServiceError> {
        self.with_harness(|store| Ok(store.snapshot()))
    }
    /// One target's saved draft without copying the rest of the harness.
    /// # Errors
    /// Refuses unavailable durable storage.
    pub fn harness_draft(
        &self,
        target: &interface_contracts::harness::AttentionTarget,
    ) -> Result<Option<interface_contracts::harness::SharedDraft>, DesktopServiceError> {
        self.with_harness(|store| Ok(store.draft(target)))
    }
    /// The newest matching task evidence without copying the rest of the
    /// harness.
    /// # Errors
    /// Refuses unavailable durable storage.
    pub fn harness_recent_tasks(
        &self,
        keep: impl Fn(&TaskEvidence) -> bool,
        limit: usize,
    ) -> Result<Vec<TaskEvidence>, DesktopServiceError> {
        self.with_harness(|store| Ok(store.recent_tasks(keep, limit)))
    }
}
fn failure(message: &str) -> DesktopServiceError {
    DesktopServiceError::new("harness-unavailable", message, false)
}

fn safe_route_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 4096
        && !path.chars().any(char::is_control)
        && Path::new(path)
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
}
#[cfg(test)]
mod tests {
    #[test]
    fn routes_cannot_escape_or_name_absolute_files() {
        for path in [
            "../other/file",
            "/tmp/file",
            "docs/../../file",
            "",
            "docs/./../file",
            "a\nfile",
        ] {
            assert!(!super::safe_route_path(path), "{path}");
        }
        assert!(super::safe_route_path("crates/interface-host/src/lib.rs"));
    }
}
