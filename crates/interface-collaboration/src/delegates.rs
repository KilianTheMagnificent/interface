//! The worker runner the Away CLI uses: a request, the job a runner receives
//! and the runner trait. Accepted requests are never replayed after an
//! ambiguous process/host failure.
//!
//! The pane-delegation service that owned background jobs for terminal panes
//! (`DelegateService`, `delegates.v1.json`, the workflow store) was retired on
//! 2026-10-14; see `docs/archive/pane-delegation-20261014/README.md`.

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::watch;

#[derive(Clone, Debug)]
pub struct DelegateRequest {
    pub task: String,
    pub files: Vec<String>,
    pub context_files: Vec<String>,
    pub model: String,
}

#[derive(Clone)]
pub struct DelegateJob {
    pub source: PathBuf,
    /// This path does not exist until the snapshot adapter creates it.
    pub destination: PathBuf,
    pub request: DelegateRequest,
}

/// Provider and filesystem adapters return a frozen proposal, never publish
/// into the parent's source directory, and settle only after the child exits.
#[async_trait]
pub trait DelegateRunner: Send + Sync {
    async fn run(
        &self,
        job: DelegateJob,
        cancelled: watch::Receiver<bool>,
    ) -> Result<Value, String>;

    /// Explicit parent-reviewed application. No provider is called here.
    async fn apply(&self, _source: PathBuf, _proposal: Value) -> Result<Value, String> {
        Err("Proposal application is unavailable in this runner".into())
    }
}
