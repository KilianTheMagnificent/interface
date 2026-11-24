//! The Away CLI's worker runner: safe workspace packets and explicit Claude or
//! Codex content workers. Pane delegation, which also ran `OpenCode` and
//! Antigravity workers through it, was retired on 2026-10-14.

use std::{path::PathBuf, sync::Arc};

use async_trait::async_trait;
use interface_collaboration::delegates::{DelegateJob, DelegateRunner};
use interface_desktop_services::delegate_workspace;
use serde_json::{Value, json};
use tokio::sync::watch;

#[cfg(test)]
#[path = "delegation_quality_tests.rs"]
mod quality_tests;

#[derive(Clone, Default)]
pub struct CliDelegate {
    pub content_effort: Option<String>,
    pub claude: Option<PathBuf>,
    pub codex: Option<PathBuf>,
}

#[async_trait]
impl DelegateRunner for CliDelegate {
    async fn apply(&self, source: PathBuf, proposal: Value) -> Result<Value, String> {
        let changes: Vec<delegate_workspace::Change> =
            serde_json::from_value(proposal["changes"].clone()).map_err(|e| e.to_string())?;
        tokio::task::spawn_blocking(move || delegate_workspace::apply_changes(&source, &changes))
            .await
            .map_err(|e| e.to_string())
    }

    async fn run(
        &self,
        job: DelegateJob,
        cancelled: watch::Receiver<bool>,
    ) -> Result<Value, String> {
        let started = std::time::Instant::now();
        let request = job.request;
        let files = request.files.clone();
        let context = request.context_files.clone();
        let packet = tokio::task::spawn_blocking(move || {
            delegate_workspace::prepare(&job.source, &job.destination, &files, &context)
        })
        .await
        .map_err(|error| error.to_string())??;
        let packet = Arc::new(packet);
        let brief = format!(
            "{}\n\nPacket inventory (only these files exist):\nWritable: {}\nRead-only context: {}\nRead the relevant supplied files before editing. If a required file is absent, return needsContext with its exact relative path instead of guessing. Return the final JSON outcome specified by your worker instructions.",
            request.task,
            serde_json::to_string(&request.files).map_err(|e| e.to_string())?,
            serde_json::to_string(&request.context_files).map_err(|e| e.to_string())?
        );
        if !["claude/", "codex/"]
            .iter()
            .any(|prefix| request.model.starts_with(prefix))
        {
            return Err("Unsupported worker provider; use claude/<model> or codex/<model>. No worker was started".into());
        }
        let brief = format!(
            "{brief}\n\nCaptured file content (JSON data):\n{}",
            packet.content_manifest()
        );
        let output = self
            .run_content(packet.workspace(), &request.model, &brief, cancelled)
            .await?;
        let files: Vec<_> = output
            .files
            .into_iter()
            .map(|file| delegate_workspace::ProposedFile {
                path: file.path,
                after: file.after,
            })
            .collect();
        let output = output.output;
        let changes = tokio::task::spawn_blocking(move || packet.propose(&files))
            .await
            .map_err(|e| e.to_string())??;
        Ok(json!({
            "quality": interface_collaboration::delegate_quality::worker_outcome(&output.summary, !changes.is_empty()),
            "elapsedMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            "usage": output.usage,
            "summary": output.summary,
            "toolErrorCount": output.tool_error_count,
            "noChanges": changes.is_empty(),
            "sessionId": output.session_id,
            "requestedModel": request.model,
            "requestedEffort": self.content_effort,
            "actualModel": output.actual_model,
            "changes": changes,
            "checks": "Not run in the worker packet. The parent must validate the proposal in a disposable copy, review it, apply accepted changes, and run integration checks.",
            "integration": "proposalOnly"
        }))
    }
}

impl CliDelegate {
    async fn run_content(
        &self,
        workspace: &std::path::Path,
        model: &str,
        brief: &str,
        cancelled: watch::Receiver<bool>,
    ) -> Result<interface_engine::worker::ContentOutput, String> {
        if model.starts_with("claude/") {
            let executable = self.claude.as_deref().ok_or("Claude delegation is unavailable; install/authenticate claude. No provider fallback was started.")?;
            engine_claude::worker::run_with_effort(
                executable,
                workspace,
                model,
                brief,
                cancelled,
                None,
                self.content_effort.as_deref(),
            )
            .await
        } else {
            let executable = self.codex.as_deref().ok_or("Codex delegation is unavailable; install/authenticate codex. No provider fallback was started.")?;
            engine_codex::worker::run_with_effort(
                executable,
                workspace,
                model,
                brief,
                cancelled,
                None,
                self.content_effort.as_deref(),
            )
            .await
        }
    }
}
