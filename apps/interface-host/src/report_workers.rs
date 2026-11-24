//! Native pane adapter and shared `OpenCode` report runner.
use async_trait::async_trait;
use interface_collaboration::workers::{Job, Report, ResultData, Runner};
use interface_storage::PrivateDirectory;
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
use tokio::sync::watch;
pub struct OpenCodeRunner {
    pub executable: PathBuf,
}
#[async_trait]
impl Runner for OpenCodeRunner {
    fn validate_policy(
        &self,
        root: &Path,
        policy: &interface_collaboration::workers::Policy,
    ) -> Result<(), String> {
        interface_collaboration::workers::grants::assert_current(root, policy)
    }
    fn capture(
        &self,
        source: &Path,
        _destination: &Path,
        files: &[String],
    ) -> Result<BTreeMap<String, String>, String> {
        // Only the frozen text is needed: the durable service freezes it before
        // admission and the provider gets no filesystem tools, so no on-disk
        // packet is built and deleted.
        interface_desktop_services::delegate_workspace::capture_read_only(source, files)
    }
    async fn run(&self, job: Job, cancelled: watch::Receiver<bool>) -> Result<ResultData, String> {
        let directory =
            PrivateDirectory::open_or_create(&job.destination).map_err(|e| e.to_string())?;
        let workspace = PrivateDirectory::open_or_create(directory.path().join("workspace"))
            .map_err(|e| e.to_string())?;
        let prompt = json!({"schemaVersion":1,"kind":job.record.request.kind,"brief":job.record.request.brief,
            "acceptanceCriteria":job.record.request.acceptance_criteria,"files":job.record.files,"packetSha256":job.record.packet_sha256}).to_string();
        let output = engine_acp::opencode_worker::run_report(
            &self.executable,
            workspace.path(),
            &job.record.request.model,
            &prompt,
            job.record.request.wall_seconds,
            job.record.request.max_steps,
            cancelled,
        )
        .await?;
        if std::fs::read_dir(workspace.path())
            .map_err(|e| e.to_string())?
            .next()
            .is_some()
        {
            return Err("invalidReport: report worker wrote unexpected files".into());
        }
        let report = Report::parse(&output.summary).ok();
        Ok(ResultData {
            report,
            original_output: output.summary,
            reported_model: output.actual_model,
            provider_session: output.session_id,
            usage: serde_json::to_value(output.usage).map_err(|e| e.to_string())?,
        })
    }
}

use crate::{
    mcp_transport::{Peer, ToolOutput},
    pane_mcp::PaneTools,
};
use interface_collaboration::workers::{Policy, Service, grants};
use interface_contracts::terminal::{TerminalGenerationRequest, TerminalLaunchProfileId};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, sync::Arc};

pub(crate) struct NativeWorkers {
    pub(crate) service: Arc<Service>,
    root: PathBuf,
    /// Worker calls that went on to identify their caller.
    #[cfg(test)]
    pub(crate) caller_lookups: std::sync::atomic::AtomicUsize,
}
impl NativeWorkers {
    pub(crate) fn open(root: PathBuf, executable: PathBuf) -> Result<Self, String> {
        let service = Service::open(
            &root,
            Policy {
                source: root.clone(),
                files: BTreeSet::new(),
                contributor_data_allowed: false,
            },
            Arc::new(OpenCodeRunner { executable }),
        )?;
        Ok(Self {
            service,
            root,
            #[cfg(test)]
            caller_lookups: std::sync::atomic::AtomicUsize::new(0),
        })
    }
    pub(crate) async fn call(
        &self,
        panes: &PaneTools,
        name: &str,
        input: serde_json::Value,
        peer: Peer,
    ) -> Result<ToolOutput, String> {
        // Providers ask for a notice on every tool call. Only a caller in a
        // conversation with an unreceived result can get a non-empty one; when
        // no running pane is bound to such a conversation, answer before the
        // process-tree walks that identify the caller's pane and conversation.
        if name == "__worker_notice"
            && let Some(unreceived) = self.service.unreceived_owners().await
            && (unreceived.is_empty()
                || !bound_to_any(&unreceived, &panes.terminal.bound_provider_sessions()))
        {
            return Ok(ToolOutput::Text(
                interface_collaboration::workers::empty_notice().to_string(),
            ));
        }
        #[cfg(test)]
        self.caller_lookups
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let caller = panes.caller(peer.clone()).await?;
        let pid = peer.pid.ok_or("scopeDenied: missing caller process")?;
        let request = TerminalGenerationRequest {
            id: caller.id,
            generation: caller.generation,
        };
        let (profile, session) = panes
            .terminal
            .report_worker_conversation(request, pid)
            .await
            .map_err(|_| "scopeDenied: provider conversation is not confirmed yet")?;
        let owner = conversation_owner(profile.as_str(), &session);
        // Collection survives cwd changes. Only new exports need a current source grant.
        let policy = if matches!(name, "worker_models" | "worker_start" | "worker_followup") {
            let source = panes
                .terminal
                .delegate_directory(request, pid, None)
                .await
                .map_err(|e| e.message)?;
            grants::read(&self.root, &source)?
        } else {
            Policy {
                source: self.root.clone(),
                files: BTreeSet::new(),
                contributor_data_allowed: false,
            }
        };
        let confirmed = panes
            .terminal
            .report_worker_conversation(request, pid)
            .await
            .map_err(|_| "scopeDenied: provider conversation changed")?;
        if confirmed != (profile, session) {
            return Err("scopeDenied: provider conversation changed".into());
        }
        let result = if name == "__worker_notice" {
            self.service.notice(&owner).await
        } else {
            self.service
                .call_scoped(&owner, &policy, name, input)
                .await?
        };
        Ok(ToolOutput::Text(result.to_string()))
    }
}
/// Whether a pane bound to one of `sessions` could be one of `owners`. Each
/// session is tried as both providers a worker conversation can belong to,
/// so this never misses the profile `report_worker_conversation` confirms.
fn bound_to_any(owners: &BTreeSet<String>, sessions: &[String]) -> bool {
    sessions.iter().any(|session| {
        [
            TerminalLaunchProfileId::Claude,
            TerminalLaunchProfileId::Codex,
        ]
        .into_iter()
        .any(|provider| owners.contains(&conversation_owner(provider.as_str(), session)))
    })
}
pub(crate) fn conversation_owner(provider: &str, session: &str) -> String {
    // Host-private storage isolates Interface runtime installations. No epoch or pane id:
    // a verified resume in another pane must recover the same inbox.
    format!(
        "{:x}",
        Sha256::digest(json!(["worker-conversation-v1", provider, session]).to_string())
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_bound_session_matches_its_conversation_under_either_provider_only() {
        let claude = "11111111-1111-4111-8111-111111111111";
        let codex = "01a06e8d-c9ac-78e3-be73-19f2255b8932";
        let other = "22222222-2222-4222-8222-222222222222";
        let owners = BTreeSet::from([
            conversation_owner("claude", claude),
            conversation_owner("codex", codex),
        ]);
        assert!(bound_to_any(&owners, &[other.into(), claude.into()]));
        assert!(bound_to_any(&owners, &[codex.into()]));
        assert!(!bound_to_any(&owners, &[other.into()]));
        assert!(!bound_to_any(&owners, &[]));
    }
    #[test]
    fn inbox_identity_uses_provider_conversation_not_a_launcher_or_pane() {
        let session = "11111111-1111-4111-8111-111111111111";
        assert_ne!(
            conversation_owner("claude", session),
            conversation_owner("codex", session)
        );
        assert_ne!(
            conversation_owner("claude", session),
            conversation_owner("claude", "22222222-2222-4222-8222-222222222222")
        );
    }
    #[test]
    fn snapshots_refuse_private_and_symlink_inputs_and_never_edit_source() {
        let root = tempfile::tempdir_in("/private/tmp").unwrap();
        let source = root.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("safe.txt"), "public fixture").unwrap();
        let runner = OpenCodeRunner {
            executable: PathBuf::from("/unused"),
        };
        assert_eq!(
            runner
                .capture(&source, &root.path().join("packet"), &["safe.txt".into()])
                .unwrap()["safe.txt"],
            "public fixture"
        );
        assert!(
            runner
                .capture(&source, &root.path().join("private"), &[".env".into()])
                .is_err()
        );
        std::os::unix::fs::symlink(source.join("safe.txt"), source.join("linked.txt")).unwrap();
        assert!(
            runner
                .capture(&source, &root.path().join("linked"), &["linked.txt".into()])
                .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(source.join("safe.txt")).unwrap(),
            "public fixture"
        );
    }
}
