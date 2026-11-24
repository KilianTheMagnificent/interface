//! Adapter over saved workflow rules and the bounded setup executor.
use super::ThreadHost;
use crate::dispatcher::DispatchError;
use interface_contracts::{
    threads::ThreadControlResponse,
    workflows::{SetupReport, SetupState, WorkflowCommand},
};
use interface_settings::workflows::SetupPurpose;

impl ThreadHost {
    pub(super) async fn workflow_command(
        &self,
        command: WorkflowCommand,
    ) -> Result<ThreadControlResponse, DispatchError> {
        self.inner
            .ports
            .authority
            .may_open(command.project_root())
            .await?;
        if let WorkflowCommand::RetrySetup { project_root, cwd } = command {
            let root = project_root.clone();
            let target = cwd.clone();
            let known = self
                .with_store_quiet(move |store, _| Ok(store.workflows()?.setup_for(&root, &target)))
                .await?;
            if known.is_none() {
                return Err(DispatchError::new(
                    "threads-setup-unknown",
                    "Choose a worktree whose setup Interface recorded.",
                    false,
                    true,
                ));
            }
            let root = project_root.clone();
            let target = cwd.clone();
            self.with_store_quiet(move |store, _| {
                store.workflows()?.ensure_retry(&root, &target)?;
                Ok(())
            })
            .await?;
            if self.worktree_user(&cwd).await?.is_some() {
                return Err(DispatchError::new(
                    "threads-setup-in-use",
                    "A session is using this worktree. Stop and archive it before rerunning setup.",
                    false,
                    true,
                ));
            }
            let request = interface_review::thread_git::GitWorkspaceRequest {
                project_root: project_root.clone().into(),
                mode: interface_review::thread_git::PrepareMode::Existing,
                base_ref: None,
                worktree_path: Some(cwd.into()),
                hint: String::new(),
            };
            let prepared = self
                .git(
                    move |git| git.prepare_workspace(&request),
                    "threads-setup-worktree",
                    "The retained worktree could not be verified",
                    true,
                )
                .await?;
            let report = self
                .setup_workspace(
                    project_root.clone(),
                    prepared.cwd.to_string_lossy().into_owned(),
                    SetupPurpose::Retry,
                )
                .await?;
            let (workflow, view) = self
                .with_store(move |store, _| Ok(store.workflows()?.view(&project_root)))
                .await?;
            return Ok(ThreadControlResponse {
                workflow: Some(workflow),
                setup: Some(report),
                ..view.into()
            });
        }
        let (workflow, view) = self
            .with_store(move |store, _| Ok(store.workflows()?.command(command)?))
            .await?;
        Ok(ThreadControlResponse {
            workflow: Some(workflow),
            ..view.into()
        })
    }

    pub(super) async fn setup_workspace(
        &self,
        root: String,
        cwd: String,
        purpose: SetupPurpose,
    ) -> Result<SetupReport, DispatchError> {
        let saved_root = root.clone();
        let saved_cwd = cwd.clone();
        let setup = self
            .with_store_quiet(move |store, _| {
                Ok(store
                    .workflows()?
                    .begin_setup(&saved_root, &saved_cwd, purpose)?)
            })
            .await?;
        let report = tokio::task::spawn_blocking(move ||
            interface_review::workspace_setup::prepare(std::path::Path::new(&cwd), &setup))
            .await.map_err(|_| DispatchError::new("threads-setup-interrupted",
                "Workspace setup stopped unexpectedly. Inspect the worktree; setup was not replayed.", false, false))?;
        let saved = report.clone();
        self.with_store_quiet(move |store, _| {
            store.workflows()?.finish_setup(&root, saved)?;
            Ok(())
        })
        .await
        .map_err(|mut error| {
            // Commands may have changed files even if the result write failed.
            error.replay_safe = false;
            error.retryable = false;
            error
        })?;
        Ok(report)
    }

    pub(super) fn setup_ready(report: &SetupReport) -> bool {
        report.state == SetupState::Ready
    }
}
