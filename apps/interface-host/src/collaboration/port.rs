//! The host's collaboration effect port: `SessionRuntimeCollaborationPort`
//! binds the durable coordinator's effects to the session runtime, holding
//! target and source leases, and pumps each target's events into the live
//! projection.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex, OnceLock},
    time::Duration,
};

use async_trait::async_trait;
use interface_collaboration::{
    EffectCommand, EffectOutcome, EffectSuccess, PacketBuildResult, SourceSnapshot,
    coordinator::HumanPreviewMaterial,
    packet::{
        RESULT_BYTES, bounded_tail, build_collaboration_packet, peer_advisory, sanitize_text,
    },
    state::{
        CollaborationEngineId, CollaborationErrorCode, CollaborationLiveStatus,
        CollaborationMode as DomainMode, CollaborationRecordV1, CollaborationSourceV1,
        CollaborationTargetV1, CollaborationWorktreeOwnerV1, CollaborationWorktreeV1,
        ManagedWorktreeV1,
    },
};
use interface_contracts::{
    engine::{
        ConsultationSafetyEnforcement, ConsultationSafetyProfile, EngineEvent, EngineEventKind,
        EngineId, LaunchOptions, SessionStatus, SessionSummary,
    },
    ipc::{HISTORY_MAX_LIMIT, parse_key},
};
use interface_engine::{ApprovalResponse, ApprovalVerdict, DeliveryOutcome};
use interface_registry::ManagedWorktreeRegistration;
use interface_runtime::{
    ProviderReadiness, RuntimeDelivery, RuntimeDeliveryAudit, RuntimeDeliveryState,
    RuntimeEventFrame, RuntimeStreamState, SessionOpenOutcome, SessionRuntime,
};
use tokio::sync::{Mutex, Notify, mpsc};
use tokio_util::sync::CancellationToken;

use super::{
    AGENT_SOURCE_REFUSED, CollaborationAgentSessions, CollaborationPortError,
    CollaborationRuntimePort, DesktopCollaborationProjection,
};
use crate::{
    dispatcher::session_event_frame,
    protocol::PushFrame,
    registry_access,
    registry_push::{RegistryPushCoordinator, SharedRegistryPushes, emit_registry_from_runtime},
    worktree::ManagedWorktreeInventory,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SourceRegistryFingerprint {
    engine: EngineId,
    session_id: String,
    title: String,
    cwd: String,
    project_root: Option<String>,
    status: SessionStatus,
    model: Option<String>,
    #[allow(clippy::option_option)]
    effort: Option<Option<String>>,
    worktree_home: Option<String>,
    worktree_path: Option<String>,
    setting_sources: Option<Vec<String>>,
    disallowed_tools: Option<Vec<String>>,
}

impl SourceRegistryFingerprint {
    pub(super) fn from_summary(summary: &SessionSummary) -> Self {
        Self {
            engine: summary.engine.clone(),
            session_id: summary.session_id.clone(),
            title: summary.title.clone(),
            cwd: summary.cwd.clone(),
            project_root: summary.project_root.clone(),
            status: summary.status,
            model: summary.model.clone(),
            effort: summary.effort.clone(),
            worktree_home: summary.worktree_home.clone(),
            worktree_path: summary.worktree_path.clone(),
            setting_sources: summary.setting_sources.clone(),
            disallowed_tools: summary.disallowed_tools.clone(),
        }
    }

    /// Field-wise comparison against a borrowed registry row, so an unchanged
    /// source costs no clone at all.
    pub(super) fn matches(&self, summary: &SessionSummary) -> bool {
        self.engine == summary.engine
            && self.session_id == summary.session_id
            && self.title == summary.title
            && self.cwd == summary.cwd
            && self.project_root == summary.project_root
            && self.status == summary.status
            && self.model == summary.model
            && self.effort == summary.effort
            && self.worktree_home == summary.worktree_home
            && self.worktree_path == summary.worktree_path
            && self.setting_sources == summary.setting_sources
            && self.disallowed_tools == summary.disallowed_tools
    }
}

#[derive(Default)]
struct SourceRegistryRevision {
    fingerprint: Option<SourceRegistryFingerprint>,
    revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum TargetTerminal {
    Answer(String),
    Failed,
    Closed,
}

#[derive(Default)]
struct TargetObservation {
    consult_active: bool,
    answer: String,
    saw_activity: bool,
    terminal: Option<TargetTerminal>,
    approval_denial_failed: bool,
}

struct TargetLease {
    collaboration_id: String,
    mode: DomainMode,
    provider_session_id: String,
    generation: u64,
    approval_gated: bool,
    observation: Mutex<TargetObservation>,
    changed: Notify,
}

#[derive(Clone)]
struct SourceLease {
    collaboration_id: String,
    provider_session_id: String,
    generation: u64,
    runtime_epoch: String,
    target_session_key: String,
    target_provider_session_id: String,
}

struct SessionRuntimeCollaborationInner {
    runtime: Arc<SessionRuntime>,
    projection: Arc<DesktopCollaborationProjection>,
    pushes: OnceLock<mpsc::Sender<PushFrame>>,
    worktrees: OnceLock<Arc<dyn ManagedWorktreeInventory>>,
    registry_pushes: OnceLock<RegistryPushCoordinator>,
    targets: Mutex<HashMap<String, Arc<TargetLease>>>,
    sources: Mutex<HashMap<String, SourceLease>>,
    registry_revisions: StdMutex<HashMap<String, SourceRegistryRevision>>,
    agent_sessions: OnceLock<Arc<dyn CollaborationAgentSessions>>,
    /// Who else writes to a provider session (a running Code pane, or one
    /// opening for it). A consult or handoff never writes to such a session.
    claims: OnceLock<Arc<interface_runtime::SessionClaims>>,
    shutting_down: CancellationToken,
}

/// Production provider/runtime adapter for the frozen desktop collaboration
/// contract. Target sessions have a single host-owned event consumer and stay
/// fenced until a durable terminal/release callback promotes or disposes them.
#[derive(Clone)]
pub struct SessionRuntimeCollaborationPort {
    inner: Arc<SessionRuntimeCollaborationInner>,
}

impl SessionRuntimeCollaborationPort {
    #[must_use]
    pub fn new(
        runtime: Arc<SessionRuntime>,
        projection: Arc<DesktopCollaborationProjection>,
    ) -> Self {
        Self {
            inner: Arc::new(SessionRuntimeCollaborationInner {
                runtime,
                projection,
                pushes: OnceLock::new(),
                worktrees: OnceLock::new(),
                registry_pushes: OnceLock::new(),
                targets: Mutex::new(HashMap::new()),
                sources: Mutex::new(HashMap::new()),
                registry_revisions: StdMutex::new(HashMap::new()),
                agent_sessions: OnceLock::new(),
                claims: OnceLock::new(),
                shutting_down: CancellationToken::new(),
            }),
        }
    }

    pub fn set_pushes(
        &self,
        pushes: mpsc::Sender<PushFrame>,
    ) -> Result<(), CollaborationPortError> {
        let registry_pushes =
            SharedRegistryPushes::start(Arc::clone(&self.inner.runtime), pushes.clone());
        self.set_shared_pushes(pushes, registry_pushes)
    }

    /// Like [`Self::set_pushes`], but schedules whole-registry frames on the
    /// host's shared coalescer, which the host closes.
    pub fn set_shared_pushes(
        &self,
        pushes: mpsc::Sender<PushFrame>,
        registry_pushes: SharedRegistryPushes,
    ) -> Result<(), CollaborationPortError> {
        self.inner
            .pushes
            .set(pushes)
            .map_err(|_| port_state_corrupt())?;
        self.inner
            .registry_pushes
            .set(registry_pushes.0)
            .map_err(|_| port_state_corrupt())?;
        Ok(())
    }

    pub fn set_worktree_inventory(
        &self,
        worktrees: Arc<dyn ManagedWorktreeInventory>,
    ) -> Result<(), CollaborationPortError> {
        self.inner
            .worktrees
            .set(worktrees)
            .map_err(|_| port_state_corrupt())?;
        Ok(())
    }

    /// [`Self::set_shared_pushes`] while building the port.
    ///
    /// # Panics
    ///
    /// If this port already has its pushes.
    #[must_use]
    pub fn with_shared_pushes(
        self,
        pushes: mpsc::Sender<PushFrame>,
        registry_pushes: SharedRegistryPushes,
    ) -> Self {
        self.set_shared_pushes(pushes, registry_pushes)
            .expect("a port being built has no pushes yet");
        self
    }

    /// [`Self::set_worktree_inventory`] while building the port.
    ///
    /// # Panics
    ///
    /// If this port already has its worktree inventory.
    #[must_use]
    pub fn with_worktree_inventory(self, worktrees: Arc<dyn ManagedWorktreeInventory>) -> Self {
        self.set_worktree_inventory(worktrees)
            .expect("a port being built has no worktree inventory yet");
        self
    }

    /// [`Self::set_agent_sessions`] while building the port.
    ///
    /// # Panics
    ///
    /// If this port already has its agent check.
    #[must_use]
    pub fn with_agent_sessions(self, agent_sessions: Arc<dyn CollaborationAgentSessions>) -> Self {
        self.set_agent_sessions(agent_sessions)
            .expect("a port being built has no agent check yet");
        self
    }

    #[must_use]
    pub fn projection(&self) -> Arc<DesktopCollaborationProjection> {
        Arc::clone(&self.inner.projection)
    }

    /// Installs the agent check; without it no source is treated as an agent's.
    pub fn set_agent_sessions(
        &self,
        agent_sessions: Arc<dyn CollaborationAgentSessions>,
    ) -> Result<(), CollaborationPortError> {
        self.inner
            .agent_sessions
            .set(agent_sessions)
            .map_err(|_| port_state_corrupt())
    }

    /// Installs the one-writer check (review B4); without it no session is
    /// held by anyone else. The host attaches it once the terminal owner,
    /// which knows the panes, exists.
    pub fn set_session_claims(
        &self,
        claims: Arc<interface_runtime::SessionClaims>,
    ) -> Result<(), CollaborationPortError> {
        self.inner
            .claims
            .set(claims)
            .map_err(|_| port_state_corrupt())
    }

    /// Refuses a session another writer holds: a running Code pane, or one
    /// it is being handed to. Definitely unsent; nothing reached a provider.
    pub(super) fn refuse_claimed(&self, session_key: &str) -> Result<(), CollaborationPortError> {
        match self
            .inner
            .claims
            .get()
            .and_then(|claims| claims.claim(session_key))
        {
            Some(claim) => Err(CollaborationPortError::new(
                CollaborationErrorCode::SourceNotLive,
                claim.message(),
                true,
            )),
            None => Ok(()),
        }
    }

    /// Refuses a source that belongs to an agent. The check reads the agent store and resolves
    /// paths, so it runs off the async workers; a failed check refuses.
    pub(super) async fn refuse_agent_source(
        &self,
        session_key: &str,
        canonical_cwd: &str,
    ) -> Result<(), CollaborationPortError> {
        let Some(agent_sessions) = self.inner.agent_sessions.get().cloned() else {
            return Ok(());
        };
        let (session_key, canonical_cwd) = (session_key.to_owned(), PathBuf::from(canonical_cwd));
        let is_agent = tokio::task::spawn_blocking(move || {
            agent_sessions.is_agent_session(&session_key, &canonical_cwd)
        })
        .await
        .unwrap_or(true);
        if is_agent {
            return Err(CollaborationPortError::new(
                CollaborationErrorCode::TargetNotSupported,
                AGENT_SOURCE_REFUSED,
                false,
            ));
        }
        Ok(())
    }

    /// Exact fence for the central runtime router. Ordinary send/resume/model,
    /// lifecycle, archive/import, and destructive operations must refuse while
    /// this returns true.
    pub async fn is_locked_session(&self, session_key: &str) -> bool {
        self.inner.targets.lock().await.contains_key(session_key)
            || self.inner.sources.lock().await.contains_key(session_key)
    }

    /// Samples one source row after a registry mutation. The host should call
    /// this for source-scoped registry changes so change-then-revert still
    /// invalidates an approval without borrowing unrelated global revisions.
    pub fn observe_registry_change(&self, session_key: &str) -> Result<(), CollaborationPortError> {
        // Take the revision map first so the registry row can be compared in
        // place: the `SessionSummary` and its twelve fingerprint fields are
        // only cloned when this source actually changed. Nothing on the
        // `registry_revisions` side ever reaches back for the session
        // registry, so this is the only nesting of the two locks.
        let mut revisions = self
            .inner
            .registry_revisions
            .lock()
            .map_err(|_| port_state_corrupt())?;
        // Only sources already resolved are sampled, as 0.1.169's
        // registry.onChange did: resolving one starts its revision at 1 from
        // the row as it is then, so earlier changes need no record. This runs
        // per engine event of every session, and most are never sources.
        let Some(known) = revisions.get_mut(session_key) else {
            return Ok(());
        };
        let sampled = with_registry_summary(&self.inner.runtime, session_key, |summary| {
            let summary = summary?;
            if known
                .fingerprint
                .as_ref()
                .is_some_and(|fingerprint| fingerprint.matches(summary))
            {
                return None;
            }
            Some(SourceRegistryFingerprint::from_summary(summary))
        })?;
        if let Some(fingerprint) = sampled {
            bump_registry_revision(known, fingerprint)?;
        }
        Ok(())
    }

    async fn source_for_effect(
        &self,
        source: &CollaborationSourceV1,
    ) -> Result<Option<SourceSnapshot>, CollaborationPortError> {
        let Some(epoch) = source.runtime_epoch.as_deref() else {
            return Err(port_state_corrupt());
        };
        let current = self
            .resolve_source(&source.session_key, epoch, &self.inner.projection)
            .await?;
        Ok(current.filter(|snapshot| source_matches_snapshot(source, snapshot)))
    }

    async fn exact_target(
        &self,
        collaboration_id: &str,
        target: &CollaborationTargetV1,
    ) -> Option<(String, Arc<TargetLease>)> {
        let (key, lease) = self.target_identity(collaboration_id, target).await?;
        let history = self
            .inner
            .runtime
            .history_session_state(&key)
            .await
            .ok()
            .flatten()?;
        (history.attachment_count == lease.generation
            && history.stream_state == RuntimeStreamState::Active)
            .then_some((key, lease))
    }

    async fn target_identity(
        &self,
        collaboration_id: &str,
        target: &CollaborationTargetV1,
    ) -> Option<(String, Arc<TargetLease>)> {
        let key = target.session_key.as_ref()?;
        let provider_session_id = target.provider_session_id.as_ref()?;
        let lease = self.inner.targets.lock().await.get(key).cloned()?;
        if lease.collaboration_id != collaboration_id
            || lease.provider_session_id != *provider_session_id
        {
            return None;
        }
        Some((key.clone(), lease))
    }

    async fn execute_prepare(
        &self,
        collaboration_id: String,
        mode: DomainMode,
        target_collaboration_depth: u8,
        source: CollaborationSourceV1,
        target_engine: CollaborationEngineId,
        cancellation: CancellationToken,
    ) -> EffectOutcome {
        let required_depth = u8::from(mode == DomainMode::Consult);
        if target_collaboration_depth != required_depth || target_engine == source.engine {
            return proven(CollaborationErrorCode::TargetNotSupported);
        }
        if let Err(error) = self.refuse_claimed(&source.session_key) {
            return proven(error.code);
        }
        if cancellation.is_cancelled() || self.inner.shutting_down.is_cancelled() {
            return proven(CollaborationErrorCode::BrokerUnavailable);
        }
        let Some(current) = (match self.source_for_effect(&source).await {
            Ok(value) => value,
            Err(error) => return proven(error.code),
        }) else {
            return proven(CollaborationErrorCode::ApprovalStale);
        };
        if current.live_status == CollaborationLiveStatus::Failed {
            return proven(CollaborationErrorCode::SourceNotLive);
        }

        let engine = match engine_id(target_engine) {
            Ok(engine) => engine,
            Err(error) => return proven(error.code),
        };
        let mut launch = LaunchOptions {
            cwd: current.canonical_cwd.clone(),
            prompt: String::new(),
            name: Some(format!(
                "{} from {}",
                if mode == DomainMode::Consult {
                    "Consult"
                } else {
                    "Handoff"
                },
                source.engine.as_str()
            )),
            collaboration_depth: Some(target_collaboration_depth),
            ..LaunchOptions::default()
        };
        if target_engine == CollaborationEngineId::Claude {
            launch.setting_sources = registry_summary(&self.inner.runtime, &source.session_key)
                .ok()
                .flatten()
                .and_then(|summary| summary.setting_sources);
        }
        let mut approval_gated = false;
        if mode == DomainMode::Consult {
            let readiness = self.inner.runtime.probe_provider(&engine).await;
            let ProviderReadiness::Ready { info, .. } = readiness else {
                return proven(CollaborationErrorCode::TargetUnhealthy);
            };
            let Some(ConsultationSafetyProfile::Available {
                available: true,
                enforcement,
                launch_options,
            }) = info.capabilities.consultation_safety
            else {
                return proven(CollaborationErrorCode::TargetNotSupported);
            };
            approval_gated = enforcement == ConsultationSafetyEnforcement::ApprovalGated;
            launch.permission_mode = launch_options.permission_mode;
            launch.sandbox_mode = launch_options.sandbox_mode;
            launch.setting_sources = launch_options.setting_sources.or(launch.setting_sources);
            launch.disallowed_tools = launch_options.disallowed_tools;
        }
        if cancellation.is_cancelled() || self.inner.shutting_down.is_cancelled() {
            return proven(CollaborationErrorCode::BrokerUnavailable);
        }
        if self
            .source_for_effect(&source)
            .await
            .ok()
            .flatten()
            .is_none()
        {
            return proven(CollaborationErrorCode::ApprovalStale);
        }
        if let Err(error) = self
            .refuse_agent_source(&current.session_key, &current.canonical_cwd)
            .await
        {
            return proven(error.code);
        }

        let opened = self.inner.runtime.start_session(&engine, launch).await;
        let ready = match opened {
            SessionOpenOutcome::Ready(ready) => ready,
            SessionOpenOutcome::DefinitelyUnsent(_) | SessionOpenOutcome::DefinitelyRejected(_) => {
                return proven(CollaborationErrorCode::TargetUnhealthy);
            }
            SessionOpenOutcome::OutcomeUnknown(_) => {
                return ambiguous(CollaborationErrorCode::ProviderOutcomeUnknown, None);
            }
            SessionOpenOutcome::ProviderReadyLocalFailure { provider, .. } => {
                return ambiguous(
                    CollaborationErrorCode::ProviderOutcomeUnknown,
                    Some(provider.session_id),
                );
            }
        };
        let Ok(Some(history)) = self.inner.runtime.history_session_state(&ready.key).await else {
            let _ = self.inner.runtime.close_session(&ready.key).await;
            return ambiguous(
                CollaborationErrorCode::ProviderOutcomeUnknown,
                Some(ready.session_id),
            );
        };
        let lease = Arc::new(TargetLease {
            collaboration_id: collaboration_id.clone(),
            mode,
            provider_session_id: ready.session_id.clone(),
            generation: history.attachment_count,
            approval_gated,
            observation: Mutex::new(TargetObservation::default()),
            changed: Notify::new(),
        });
        {
            let mut targets = self.inner.targets.lock().await;
            if targets.contains_key(&ready.key) {
                drop(targets);
                let _ = self.inner.runtime.close_session(&ready.key).await;
                return ambiguous(
                    CollaborationErrorCode::ProviderOutcomeUnknown,
                    Some(ready.session_id),
                );
            }
            targets.insert(ready.key.clone(), Arc::clone(&lease));
        }
        let Ok(stream) = self.inner.runtime.take_session_events(&ready.key).await else {
            let closed = close_runtime_session(&self.inner.runtime, &ready.key).await;
            if closed {
                self.inner.targets.lock().await.remove(&ready.key);
            }
            return ambiguous(
                CollaborationErrorCode::ProviderOutcomeUnknown,
                Some(ready.session_id),
            );
        };
        tokio::spawn(pump_collaboration_target(
            Arc::clone(&self.inner),
            ready.key.clone(),
            Arc::clone(&lease),
            stream,
        ));

        if cancellation.is_cancelled()
            || self.inner.shutting_down.is_cancelled()
            || self
                .source_for_effect(&source)
                .await
                .ok()
                .flatten()
                .is_none()
        {
            let closed = close_runtime_session(&self.inner.runtime, &ready.key).await;
            if closed {
                remove_exact_target(&self.inner, &ready.key, &lease).await;
            }
            return ambiguous(
                CollaborationErrorCode::ProviderOutcomeUnknown,
                Some(ready.session_id),
            );
        }
        succeeded(
            EffectSuccess::PreparedTarget(interface_collaboration::PreparedTarget {
                session_key: ready.key,
                provider_session_id: ready.session_id.clone(),
                reconciliation_marker: None,
            }),
            Some(ready.session_id),
        )
    }

    async fn execute_run(
        &self,
        collaboration_id: String,
        target: CollaborationTargetV1,
        packet_body: String,
        cancellation: CancellationToken,
    ) -> EffectOutcome {
        let Some((key, lease)) = self.exact_target(&collaboration_id, &target).await else {
            return proven(CollaborationErrorCode::TargetUnhealthy);
        };
        if self.refuse_claimed(&key).is_err() {
            return proven(CollaborationErrorCode::TargetUnhealthy);
        }
        if lease.mode != DomainMode::Consult
            || cancellation.is_cancelled()
            || self.inner.shutting_down.is_cancelled()
        {
            return proven(CollaborationErrorCode::TargetUnhealthy);
        }
        {
            let mut observation = lease.observation.lock().await;
            if observation.approval_denial_failed {
                drop(observation);
                let _ = close_runtime_session(&self.inner.runtime, &key).await;
                return proven(CollaborationErrorCode::TargetUnhealthy);
            }
            observation.consult_active = true;
            observation.answer.clear();
            observation.saw_activity = false;
            observation.terminal = None;
        }

        let delivery = self.inner.runtime.send(&key, packet_body).await;
        match delivery_disposition(&delivery) {
            DeliveryDisposition::Succeeded => {}
            DeliveryDisposition::ProvenUnsent => {
                let _ = close_runtime_session(&self.inner.runtime, &key).await;
                return proven(CollaborationErrorCode::TargetUnhealthy);
            }
            DeliveryDisposition::Ambiguous => {
                return ambiguous(
                    CollaborationErrorCode::ProviderOutcomeUnknown,
                    Some(lease.provider_session_id.clone()),
                );
            }
        }

        let terminal = loop {
            let notified = lease.changed.notified();
            if let Some(terminal) = lease.observation.lock().await.terminal.clone() {
                break terminal;
            }
            tokio::select! {
                () = notified => {}
                () = cancellation.cancelled() => {
                    return ambiguous(
                        CollaborationErrorCode::ProviderOutcomeUnknown,
                        Some(lease.provider_session_id.clone()),
                    );
                }
                () = self.inner.shutting_down.cancelled() => {
                    return ambiguous(
                        CollaborationErrorCode::ProviderOutcomeUnknown,
                        Some(lease.provider_session_id.clone()),
                    );
                }
            }
        };
        let denial_failed = lease.observation.lock().await.approval_denial_failed;
        if denial_failed {
            let _ = close_runtime_session(&self.inner.runtime, &key).await;
            return ambiguous(
                CollaborationErrorCode::ProviderOutcomeUnknown,
                Some(lease.provider_session_id.clone()),
            );
        }
        let TargetTerminal::Answer(answer) = terminal else {
            let _ = close_runtime_session(&self.inner.runtime, &key).await;
            return ambiguous(
                CollaborationErrorCode::ProviderOutcomeUnknown,
                Some(lease.provider_session_id.clone()),
            );
        };
        let _cleanup_confirmed = close_runtime_session(&self.inner.runtime, &key).await;
        succeeded(
            EffectSuccess::ConsultResult(answer),
            Some(lease.provider_session_id.clone()),
        )
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_deliver(
        &self,
        collaboration_id: String,
        source: CollaborationSourceV1,
        target: CollaborationTargetV1,
        queue_entry_id: String,
        result_id: String,
        result_digest: String,
        body: String,
        expected_source_guard_hash: String,
        cancellation: CancellationToken,
    ) -> EffectOutcome {
        if cancellation.is_cancelled() || self.inner.shutting_down.is_cancelled() {
            return proven(CollaborationErrorCode::BrokerUnavailable);
        }
        // The advisory is a send: never to a session another writer holds.
        if let Err(error) = self.refuse_claimed(&source.session_key) {
            return proven(error.code);
        }
        let Some(current) = (match self.source_for_effect(&source).await {
            Ok(current) => current,
            Err(error) => return proven(error.code),
        }) else {
            return proven(CollaborationErrorCode::ApprovalStale);
        };
        let Ok(actual_guard) = interface_collaboration::source_dispatch_guard_hash(&current) else {
            return proven(CollaborationErrorCode::StateCorrupt);
        };
        if actual_guard != expected_source_guard_hash
            || current.live_status != CollaborationLiveStatus::Idle
            || current.pending_approval_count != 0
            || current.queue_depth != 0
            || current.delivery_unknown
        {
            return proven(CollaborationErrorCode::ApprovalStale);
        }
        let Some(target_key) = target.session_key.as_ref() else {
            return proven(CollaborationErrorCode::StateCorrupt);
        };
        let Some(held) = self
            .inner
            .projection
            .held_peer(&source.session_key, &queue_entry_id)
            .ok()
            .flatten()
        else {
            return proven(CollaborationErrorCode::SourceQueueNotEmpty);
        };
        if held.collaboration_id != collaboration_id
            || held.result_id != result_id
            || held.result_digest != result_digest
            || held.target_session_key != *target_key
            || held.body != body
        {
            return proven(CollaborationErrorCode::StateCorrupt);
        }
        // The advisory goes to the source on the raw runtime, past the agent guard.
        if let Err(error) = self
            .refuse_agent_source(&current.session_key, &current.canonical_cwd)
            .await
        {
            return proven(error.code);
        }
        let advisory = peer_advisory(
            &held.collaboration_id,
            held.target_engine,
            &held.objective,
            &held.body,
        );
        let delivery = self.inner.runtime.send(&source.session_key, advisory).await;
        match delivery_disposition(&delivery) {
            DeliveryDisposition::Succeeded => {
                let session_id = target
                    .provider_session_id
                    .clone()
                    .unwrap_or_else(|| target_key.clone());
                let event = EngineEvent {
                    at: None,
                    event: EngineEventKind::PeerResult {
                        operation_id: collaboration_id,
                        source_engine: match engine_id(held.target_engine) {
                            Ok(engine) => engine,
                            Err(_) => return proven(CollaborationErrorCode::StateCorrupt),
                        },
                        source_session_id: session_id,
                        objective: held.objective,
                        text: bounded_tail(&sanitize_text(&held.body), RESULT_BYTES),
                        untrusted: true,
                    },
                };
                if self
                    .inner
                    .projection
                    .observe_event(&source.session_key, &event)
                    .is_err()
                {
                    return ambiguous(CollaborationErrorCode::DeliveryUnknown, Some(result_id));
                }
                succeeded(EffectSuccess::Unit, Some(result_id))
            }
            DeliveryDisposition::ProvenUnsent => proven(CollaborationErrorCode::ApprovalStale),
            DeliveryDisposition::Ambiguous => {
                let _ = self.inner.projection.fail_closed(&source.session_key);
                ambiguous(CollaborationErrorCode::DeliveryUnknown, Some(result_id))
            }
        }
    }

    async fn execute_park(
        &self,
        collaboration_id: String,
        source: CollaborationSourceV1,
        target: CollaborationTargetV1,
        expected_source_guard_hash: String,
        cancellation: CancellationToken,
    ) -> EffectOutcome {
        if cancellation.is_cancelled() || self.inner.shutting_down.is_cancelled() {
            return proven(CollaborationErrorCode::BrokerUnavailable);
        }
        if let Err(error) = self.refuse_claimed(&source.session_key) {
            return proven(error.code);
        }
        let Some(current) = (match self.source_for_effect(&source).await {
            Ok(current) => current,
            Err(error) => return proven(error.code),
        }) else {
            return proven(CollaborationErrorCode::ApprovalStale);
        };
        let Ok(approval_guard) = current.to_source().and_then(|value| {
            interface_collaboration::state::source_guard_hash_for_approval(&value)
                .map_err(interface_collaboration::CoordinatorError::from)
        }) else {
            return proven(CollaborationErrorCode::StateCorrupt);
        };
        if approval_guard != expected_source_guard_hash
            || current.live_status != CollaborationLiveStatus::Idle
            || current.pending_approval_count != 0
            || current.queue_depth != 0
            || current.delivery_unknown
        {
            return proven(CollaborationErrorCode::ApprovalStale);
        }
        let Some((target_key, target_lease)) = self.exact_target(&collaboration_id, &target).await
        else {
            return proven(CollaborationErrorCode::TargetUnhealthy);
        };
        if target_lease.mode != DomainMode::Handoff {
            return proven(CollaborationErrorCode::TargetUnhealthy);
        }
        let Some(source_provider_session_id) = source.provider_session_id.clone() else {
            return proven(CollaborationErrorCode::StateCorrupt);
        };
        let Some(generation) = source.live_handle_generation else {
            return proven(CollaborationErrorCode::StateCorrupt);
        };
        let Some(runtime_epoch) = source.runtime_epoch.clone() else {
            return proven(CollaborationErrorCode::StateCorrupt);
        };
        let source_lease = SourceLease {
            collaboration_id: collaboration_id.clone(),
            provider_session_id: source_provider_session_id.clone(),
            generation,
            runtime_epoch,
            target_session_key: target_key,
            target_provider_session_id: target_lease.provider_session_id.clone(),
        };
        {
            let mut sources = self.inner.sources.lock().await;
            if sources.contains_key(&source.session_key) {
                return proven(CollaborationErrorCode::SourceNotIdle);
            }
            sources.insert(source.session_key.clone(), source_lease.clone());
        }
        let delivery = self.inner.runtime.close_session(&source.session_key).await;
        match delivery_disposition(&delivery) {
            DeliveryDisposition::Succeeded => {
                succeeded(EffectSuccess::Unit, Some(source_provider_session_id))
            }
            DeliveryDisposition::ProvenUnsent => {
                remove_exact_source(&self.inner, &source.session_key, &source_lease).await;
                proven(CollaborationErrorCode::SourceNotLive)
            }
            DeliveryDisposition::Ambiguous => ambiguous(
                CollaborationErrorCode::SourceParkUnknown,
                Some(source_provider_session_id),
            ),
        }
    }

    async fn execute_transfer(
        &self,
        collaboration_id: String,
        source: CollaborationSourceV1,
        target: CollaborationTargetV1,
        cancellation: CancellationToken,
    ) -> EffectOutcome {
        if cancellation.is_cancelled() || self.inner.shutting_down.is_cancelled() {
            return proven(CollaborationErrorCode::BrokerUnavailable);
        }
        let Some((target_key, lease)) = self.exact_target(&collaboration_id, &target).await else {
            return proven(CollaborationErrorCode::TargetUnhealthy);
        };
        if lease.mode != DomainMode::Handoff {
            return proven(CollaborationErrorCode::TargetUnhealthy);
        }
        if source.worktree_owner_revision.is_some() {
            return proven(CollaborationErrorCode::ManagedWorktreeTransferUnavailable);
        }
        if !exact_source_lease(
            &self.inner,
            &collaboration_id,
            &source,
            &target_key,
            &lease.provider_session_id,
        )
        .await
        {
            return proven(CollaborationErrorCode::SourceNotLive);
        }
        succeeded(EffectSuccess::Unit, None)
    }

    async fn execute_activate(
        &self,
        collaboration_id: String,
        source: CollaborationSourceV1,
        target: CollaborationTargetV1,
        packet_body: String,
        cancellation: CancellationToken,
    ) -> EffectOutcome {
        let Some((target_key, lease)) = self.exact_target(&collaboration_id, &target).await else {
            return proven(CollaborationErrorCode::TargetUnhealthy);
        };
        if self.refuse_claimed(&target_key).is_err() {
            return proven(CollaborationErrorCode::TargetUnhealthy);
        }
        if lease.mode != DomainMode::Handoff
            || cancellation.is_cancelled()
            || self.inner.shutting_down.is_cancelled()
        {
            return proven(CollaborationErrorCode::TargetUnhealthy);
        }
        if !exact_source_lease(
            &self.inner,
            &collaboration_id,
            &source,
            &target_key,
            &lease.provider_session_id,
        )
        .await
        {
            return proven(CollaborationErrorCode::SourceNotLive);
        }
        let delivery = self.inner.runtime.send(&target_key, packet_body).await;
        match delivery_disposition(&delivery) {
            DeliveryDisposition::Succeeded => {
                succeeded(EffectSuccess::Unit, Some(lease.provider_session_id.clone()))
            }
            DeliveryDisposition::ProvenUnsent => proven(CollaborationErrorCode::TargetUnhealthy),
            DeliveryDisposition::Ambiguous => ambiguous(
                CollaborationErrorCode::ProviderOutcomeUnknown,
                Some(lease.provider_session_id.clone()),
            ),
        }
    }
}

#[async_trait]
impl CollaborationRuntimePort for SessionRuntimeCollaborationPort {
    async fn resolve_source(
        &self,
        session_key: &str,
        runtime_epoch: &str,
        projection: &DesktopCollaborationProjection,
    ) -> Result<Option<SourceSnapshot>, CollaborationPortError> {
        if self.inner.targets.lock().await.contains_key(session_key) {
            return Ok(None);
        }
        let (engine_name_value, provider_session_id) =
            parse_key(session_key).map_err(|_| port_state_corrupt())?;
        let engine = CollaborationEngineId::from_wire(engine_name_value.as_str())
            .ok_or_else(port_state_corrupt)?;
        let Some(summary) = registry_summary(&self.inner.runtime, session_key)? else {
            return Ok(None);
        };
        if summary.engine != engine_name_value || summary.session_id != provider_session_id {
            return Err(port_state_corrupt());
        }
        let applied = projection.history_sequence(session_key).map_err(|error| {
            CollaborationPortError::new(CollaborationErrorCode::StateCorrupt, error.message, false)
        })?;
        let Some((history, events)) = self
            .inner
            .runtime
            .history_events_after(session_key, applied)
            .await
            .map_err(|error| {
                port_runtime_error(CollaborationErrorCode::BrokerUnavailable, error)
            })?
        else {
            return Ok(None);
        };
        if history.stream_state != RuntimeStreamState::Active
            || history.delivery_state == RuntimeDeliveryState::Closed
        {
            return Ok(None);
        }
        projection
            .hydrate_history(session_key, history.omitted_events_through, &events)
            .map_err(|error| {
                CollaborationPortError::new(
                    CollaborationErrorCode::StateCorrupt,
                    error.message,
                    false,
                )
            })?;
        let projected = projection.source_state(session_key).map_err(|error| {
            CollaborationPortError::new(CollaborationErrorCode::StateCorrupt, error.message, false)
        })?;
        let registry_revision = registry_revision(&self.inner, session_key, &summary)?;
        let canonical_cwd = canonical_path(&summary.cwd).await?;
        let project_candidate = if let Some(root) = summary.project_root.as_deref() {
            PathBuf::from(root)
        } else {
            // An ancestor walk of `.git` probes stays off the async workers.
            let start = canonical_cwd.clone();
            tokio::task::spawn_blocking(move || find_project_root(&start))
                .await
                .ok()
                .flatten()
                .unwrap_or_else(|| canonical_cwd.clone())
        };
        let canonical_project_root = canonical_path_buf(&project_candidate).await?;
        let worktree = resolve_worktree(&self.inner, session_key, registry_revision).await?;
        let delivery_unknown = projected.delivery_unknown
            || matches!(
                history.delivery_state,
                RuntimeDeliveryState::OutcomeUnknown | RuntimeDeliveryState::PersistenceFailed
            );
        let live_status = if delivery_unknown {
            CollaborationLiveStatus::Failed
        } else {
            live_status(summary.status)
        };
        Ok(Some(SourceSnapshot {
            engine,
            session_key: session_key.to_owned(),
            provider_session_id,
            title: (!summary.title.is_empty()).then_some(summary.title),
            registry_revision,
            runtime_epoch: runtime_epoch.to_owned(),
            live_handle_generation: history.attachment_count,
            live_status,
            canonical_cwd: path_string(&canonical_cwd)?,
            canonical_project_root: path_string(&canonical_project_root)?,
            pending_approval_set_hash: projected.pending_approval_set_hash,
            pending_approval_count: projected.pending_approval_count,
            queue_revision: projected.queue_revision,
            queue_depth: projected.queue_depth,
            delivery_revision: projected.delivery_revision,
            delivery_unknown,
            worktree: Some(worktree),
        }))
    }

    async fn check_target(
        &self,
        source: &SourceSnapshot,
        target: CollaborationEngineId,
        mode: DomainMode,
    ) -> Result<(), CollaborationPortError> {
        self.refuse_agent_source(&source.session_key, &source.canonical_cwd)
            .await?;
        self.refuse_claimed(&source.session_key)?;
        if source.engine == target {
            return Err(CollaborationPortError::new(
                CollaborationErrorCode::TargetNotSupported,
                "source and target engines must differ",
                true,
            ));
        }
        let engine = engine_id(target)?;
        if !self
            .inner
            .runtime
            .registered_engines()
            .iter()
            .any(|candidate| candidate == &engine)
        {
            return Err(CollaborationPortError::new(
                CollaborationErrorCode::TargetNotSupported,
                "target engine is not registered",
                true,
            ));
        }
        if mode == DomainMode::Consult {
            match self.inner.runtime.probe_provider(&engine).await {
                ProviderReadiness::Ready { info, .. } => {
                    match info.capabilities.consultation_safety.as_ref() {
                        Some(ConsultationSafetyProfile::Available {
                            available: true, ..
                        }) => {}
                        _ => {
                            return Err(CollaborationPortError::new(
                                CollaborationErrorCode::TargetNotSupported,
                                "target does not advertise supervised consultation safety",
                                true,
                            ));
                        }
                    }
                }
                ProviderReadiness::Unavailable { error, .. } => {
                    return Err(port_runtime_error(
                        CollaborationErrorCode::TargetUnhealthy,
                        error,
                    ));
                }
            }
        }
        Ok(())
    }

    async fn build_packet(
        &self,
        material: &HumanPreviewMaterial,
        source: &SourceSnapshot,
        projection: &DesktopCollaborationProjection,
    ) -> Result<PacketBuildResult, CollaborationPortError> {
        let stored = self
            .inner
            .runtime
            .read_provider_history(&source.session_key, HISTORY_MAX_LIMIT)
            .await
            .map_err(|error| {
                port_runtime_error(CollaborationErrorCode::BrokerUnavailable, error)
            })?;
        let live = projection
            .live_transcript(&source.session_key)
            .map_err(|error| {
                CollaborationPortError::new(
                    CollaborationErrorCode::StateCorrupt,
                    error.message,
                    false,
                )
            })?;
        let events = combine_provider_and_live_transcript(stored.events, live);
        build_collaboration_packet(material, source, &events).map_err(|_| port_state_corrupt())
    }

    async fn execute(
        &self,
        command: EffectCommand,
        cancellation: CancellationToken,
    ) -> EffectOutcome {
        match command {
            EffectCommand::PrepareTarget {
                collaboration_id,
                mode,
                target_collaboration_depth,
                source,
                target_engine,
                ..
            } => {
                self.execute_prepare(
                    collaboration_id,
                    mode,
                    target_collaboration_depth,
                    source,
                    target_engine,
                    cancellation,
                )
                .await
            }
            EffectCommand::RunTarget {
                collaboration_id,
                target,
                packet_body,
                ..
            } => {
                self.execute_run(collaboration_id, target, packet_body, cancellation)
                    .await
            }
            EffectCommand::DeliverResult {
                collaboration_id,
                source,
                target,
                queue_entry_id,
                result_id,
                result_digest,
                body,
                expected_source_guard_hash,
                ..
            } => {
                self.execute_deliver(
                    collaboration_id,
                    source,
                    target,
                    queue_entry_id,
                    result_id,
                    result_digest,
                    body,
                    expected_source_guard_hash,
                    cancellation,
                )
                .await
            }
            EffectCommand::ParkSource {
                collaboration_id,
                source,
                target,
                expected_source_guard_hash,
                ..
            } => {
                self.execute_park(
                    collaboration_id,
                    source,
                    target,
                    expected_source_guard_hash,
                    cancellation,
                )
                .await
            }
            EffectCommand::TransferOwnership {
                collaboration_id,
                source,
                target,
                ..
            } => {
                self.execute_transfer(collaboration_id, source, target, cancellation)
                    .await
            }
            EffectCommand::ActivateTarget {
                collaboration_id,
                source,
                target,
                packet_body,
                ..
            } => {
                self.execute_activate(collaboration_id, source, target, packet_body, cancellation)
                    .await
            }
        }
    }

    async fn release_target(
        &self,
        record: &CollaborationRecordV1,
        _cancellation: CancellationToken,
    ) -> Result<(), CollaborationPortError> {
        let Some(key) = record.target.session_key.as_deref() else {
            return Ok(());
        };
        if let Some((_, lease)) = self
            .target_identity(&record.collaboration_id, &record.target)
            .await
        {
            remove_exact_target(&self.inner, key, &lease).await;
        }
        release_source_leases(&self.inner, &record.collaboration_id, &record.target).await;
        Ok(())
    }

    async fn dispose_target(
        &self,
        record: &CollaborationRecordV1,
        _cancellation: CancellationToken,
    ) -> Result<(), CollaborationPortError> {
        if let Some((key, lease)) = self
            .target_identity(&record.collaboration_id, &record.target)
            .await
        {
            let closed = close_runtime_session(&self.inner.runtime, &key).await
                || runtime_session_is_closed(&self.inner.runtime, &key).await;
            if closed {
                remove_exact_target(&self.inner, &key, &lease).await;
            }
        }
        release_source_leases(&self.inner, &record.collaboration_id, &record.target).await;
        Ok(())
    }

    async fn shutdown(&self) -> Result<(), CollaborationPortError> {
        self.inner.shutting_down.cancel();
        let targets = self
            .inner
            .targets
            .lock()
            .await
            .iter()
            .map(|(key, lease)| (key.clone(), Arc::clone(lease)))
            .collect::<Vec<_>>();
        let mut incomplete = false;
        for (key, lease) in targets {
            let closed = match tokio::time::timeout(
                Duration::from_secs(10),
                close_runtime_session(&self.inner.runtime, &key),
            )
            .await
            {
                Ok(closed) => closed || runtime_session_is_closed(&self.inner.runtime, &key).await,
                Err(_) => false,
            };
            if closed {
                remove_exact_target(&self.inner, &key, &lease).await;
            } else {
                incomplete = true;
            }
        }
        if incomplete {
            Err(CollaborationPortError::new(
                CollaborationErrorCode::BrokerUnavailable,
                "one or more collaboration targets remain quarantined after shutdown",
                true,
            ))
        } else {
            Ok(())
        }
    }
}

pub(crate) fn combine_provider_and_live_transcript(
    mut stored: Vec<EngineEvent>,
    live: Vec<EngineEvent>,
) -> Vec<EngineEvent> {
    stored.extend(live);
    let limit = usize::try_from(HISTORY_MAX_LIMIT).unwrap_or(usize::MAX);
    let skip = stored.len().saturating_sub(limit);
    stored.drain(..skip);
    stored
}

// `Option::cloned` cannot be named here: the shared/mutable inherent impls make
// the path ambiguous, and the turbofish form is not generic over the lifetime
// `with_registry_summary` needs.
#[allow(clippy::redundant_closure_for_method_calls)]
fn registry_summary(
    runtime: &SessionRuntime,
    session_key: &str,
) -> Result<Option<SessionSummary>, CollaborationPortError> {
    with_registry_summary(runtime, session_key, |summary| summary.cloned())
}

fn with_registry_summary<T>(
    runtime: &SessionRuntime,
    session_key: &str,
    read: impl FnOnce(Option<&SessionSummary>) -> T,
) -> Result<T, CollaborationPortError> {
    registry_access::read(runtime, |registry| read(registry.get(session_key)))
        .map_err(|_| port_state_corrupt())
}

fn registry_revision(
    inner: &SessionRuntimeCollaborationInner,
    session_key: &str,
    summary: &SessionSummary,
) -> Result<u64, CollaborationPortError> {
    let mut revisions = inner
        .registry_revisions
        .lock()
        .map_err(|_| port_state_corrupt())?;
    let entry = revisions.entry(session_key.to_owned()).or_default();
    if entry.revision != 0
        && entry
            .fingerprint
            .as_ref()
            .is_some_and(|fingerprint| fingerprint.matches(summary))
    {
        return Ok(entry.revision);
    }
    bump_registry_revision(entry, SourceRegistryFingerprint::from_summary(summary))
}

/// Advances one source's registry revision after its fingerprint changed. A
/// fresh row starts at 1; every later change increments inside the JS safe
/// integer range the desktop contract pins.
fn bump_registry_revision(
    entry: &mut SourceRegistryRevision,
    fingerprint: SourceRegistryFingerprint,
) -> Result<u64, CollaborationPortError> {
    entry.revision = if entry.revision == 0 {
        1
    } else {
        entry
            .revision
            .checked_add(1)
            .filter(|value| *value <= interface_collaboration::state::JS_MAX_SAFE_INTEGER)
            .ok_or_else(port_state_corrupt)?
    };
    entry.fingerprint = Some(fingerprint);
    Ok(entry.revision)
}

async fn resolve_worktree(
    inner: &SessionRuntimeCollaborationInner,
    session_key: &str,
    owner_revision: u64,
) -> Result<CollaborationWorktreeV1, CollaborationPortError> {
    let Some(inventory) = inner.worktrees.get() else {
        return Ok(unmanaged_worktree());
    };
    let record = inventory
        .records()
        .map_err(|error| {
            CollaborationPortError::new(CollaborationErrorCode::StateCorrupt, error.message, false)
        })?
        .into_iter()
        .find(|record| record.key == session_key);
    let Some(record) = record else {
        return Ok(unmanaged_worktree());
    };
    if inventory.verify(record.clone()).await.map_err(|error| {
        CollaborationPortError::new(CollaborationErrorCode::StateCorrupt, error.message, false)
    })? != ManagedWorktreeRegistration::Active
    {
        return Err(CollaborationPortError::new(
            CollaborationErrorCode::StateCorrupt,
            "managed worktree identity could not be verified",
            false,
        ));
    }
    let canonical_path = canonical_path_buf(&record.path).await?;
    Ok(CollaborationWorktreeV1::Managed(ManagedWorktreeV1 {
        version: 1,
        managed: true,
        canonical_path: path_string(&canonical_path)?,
        branch: record.branch,
        owner_revision,
        transfer_id: None,
        release_in_progress: false,
        owner: CollaborationWorktreeOwnerV1::Source {
            session_key: session_key.to_owned(),
        },
    }))
}

fn unmanaged_worktree() -> CollaborationWorktreeV1 {
    CollaborationWorktreeV1::Unmanaged(interface_collaboration::state::UnmanagedWorktreeV1 {
        version: 1,
        managed: false,
    })
}

async fn canonical_path(value: &str) -> Result<PathBuf, CollaborationPortError> {
    canonical_path_buf(Path::new(value)).await
}

async fn canonical_path_buf(value: &Path) -> Result<PathBuf, CollaborationPortError> {
    tokio::fs::canonicalize(value).await.map_err(|error| {
        CollaborationPortError::new(
            CollaborationErrorCode::SourceNotFound,
            format!("could not canonicalize source path: {error}"),
            true,
        )
    })
}

fn find_project_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|candidate| candidate.join(".git").exists())
        .map(Path::to_path_buf)
}

fn path_string(value: &Path) -> Result<String, CollaborationPortError> {
    value
        .to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(port_state_corrupt)
}

fn live_status(status: SessionStatus) -> CollaborationLiveStatus {
    match status {
        SessionStatus::Working => CollaborationLiveStatus::Working,
        SessionStatus::Waiting => CollaborationLiveStatus::Waiting,
        SessionStatus::Idle => CollaborationLiveStatus::Idle,
        SessionStatus::Failed => CollaborationLiveStatus::Failed,
    }
}

fn engine_id(value: CollaborationEngineId) -> Result<EngineId, CollaborationPortError> {
    EngineId::new(value.as_str()).map_err(|_| port_state_corrupt())
}

fn port_state_corrupt() -> CollaborationPortError {
    CollaborationPortError::new(
        CollaborationErrorCode::StateCorrupt,
        "collaboration runtime state is corrupt or unavailable",
        false,
    )
}

fn port_runtime_error(
    code: CollaborationErrorCode,
    error: impl std::fmt::Display,
) -> CollaborationPortError {
    CollaborationPortError::new(code, error.to_string(), true)
}

fn source_matches_snapshot(source: &CollaborationSourceV1, snapshot: &SourceSnapshot) -> bool {
    let Ok(current) = snapshot.to_source() else {
        return false;
    };
    let Ok(expected_hash) = interface_collaboration::state::source_guard_hash_for_approval(source)
    else {
        return false;
    };
    let Ok(current_hash) = interface_collaboration::state::source_guard_hash_for_approval(&current)
    else {
        return false;
    };
    expected_hash == current_hash
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DeliveryDisposition {
    Succeeded,
    ProvenUnsent,
    Ambiguous,
}

fn delivery_disposition(delivery: &RuntimeDelivery) -> DeliveryDisposition {
    match (&delivery.outcome, &delivery.audit) {
        (DeliveryOutcome::Confirmed(()), RuntimeDeliveryAudit::Persisted { .. }) => {
            DeliveryDisposition::Succeeded
        }
        (DeliveryOutcome::DefinitelyUnsent(_) | DeliveryOutcome::DefinitelyRejected(_), _) => {
            DeliveryDisposition::ProvenUnsent
        }
        _ => DeliveryDisposition::Ambiguous,
    }
}

fn succeeded(value: EffectSuccess, provider_effect_id: Option<String>) -> EffectOutcome {
    EffectOutcome::Succeeded {
        value,
        provider_effect_id,
    }
}

fn proven(code: CollaborationErrorCode) -> EffectOutcome {
    EffectOutcome::ProvenUnsent { code }
}

fn ambiguous(code: CollaborationErrorCode, provider_effect_id: Option<String>) -> EffectOutcome {
    EffectOutcome::Ambiguous {
        code: Some(code),
        provider_effect_id,
    }
}

async fn close_runtime_session(runtime: &SessionRuntime, key: &str) -> bool {
    delivery_disposition(&runtime.close_session(key).await) == DeliveryDisposition::Succeeded
}

async fn runtime_session_is_closed(runtime: &SessionRuntime, key: &str) -> bool {
    runtime
        .history_session_state(key)
        .await
        .ok()
        .flatten()
        .is_some_and(|history| {
            history.stream_state == RuntimeStreamState::Closed
                || history.delivery_state == RuntimeDeliveryState::Closed
        })
}

async fn remove_exact_target(
    inner: &SessionRuntimeCollaborationInner,
    key: &str,
    expected: &Arc<TargetLease>,
) {
    let mut targets = inner.targets.lock().await;
    if targets
        .get(key)
        .is_some_and(|current| Arc::ptr_eq(current, expected))
    {
        targets.remove(key);
    }
}

async fn remove_exact_source(
    inner: &SessionRuntimeCollaborationInner,
    key: &str,
    expected: &SourceLease,
) {
    let mut sources = inner.sources.lock().await;
    if sources.get(key).is_some_and(|current| {
        current.collaboration_id == expected.collaboration_id
            && current.provider_session_id == expected.provider_session_id
            && current.generation == expected.generation
            && current.runtime_epoch == expected.runtime_epoch
            && current.target_session_key == expected.target_session_key
            && current.target_provider_session_id == expected.target_provider_session_id
    }) {
        sources.remove(key);
    }
}

async fn exact_source_lease(
    inner: &SessionRuntimeCollaborationInner,
    collaboration_id: &str,
    source: &CollaborationSourceV1,
    target_session_key: &str,
    target_provider_session_id: &str,
) -> bool {
    let Some(provider_session_id) = source.provider_session_id.as_deref() else {
        return false;
    };
    let Some(generation) = source.live_handle_generation else {
        return false;
    };
    let Some(runtime_epoch) = source.runtime_epoch.as_deref() else {
        return false;
    };
    inner
        .sources
        .lock()
        .await
        .get(&source.session_key)
        .is_some_and(|lease| {
            lease.collaboration_id == collaboration_id
                && lease.provider_session_id == provider_session_id
                && lease.generation == generation
                && lease.runtime_epoch == runtime_epoch
                && lease.target_session_key == target_session_key
                && lease.target_provider_session_id == target_provider_session_id
        })
}

async fn release_source_leases(
    inner: &SessionRuntimeCollaborationInner,
    collaboration_id: &str,
    target: &CollaborationTargetV1,
) {
    let mut sources = inner.sources.lock().await;
    sources.retain(|_, lease| {
        !(lease.collaboration_id == collaboration_id
            && target
                .session_key
                .as_deref()
                .is_some_and(|key| key == lease.target_session_key)
            && target
                .provider_session_id
                .as_deref()
                .is_some_and(|id| id == lease.target_provider_session_id))
    });
}

async fn pump_collaboration_target(
    inner: Arc<SessionRuntimeCollaborationInner>,
    key: String,
    lease: Arc<TargetLease>,
    mut events: interface_engine::BoundedEventStream<RuntimeEventFrame>,
) {
    loop {
        match events.recv().await {
            Ok(Some(RuntimeEventFrame::Event { value })) => {
                push_runtime_event(&inner, &value.session_key, &value.event).await;
                let _ = inner
                    .projection
                    .observe_event(&value.session_key, &value.event);
                if target_is_current(&inner, &key, &lease).await {
                    observe_target_event(&inner, &key, &lease, &value.event).await;
                }
                schedule_registry_push(&inner);
            }
            Ok(Some(RuntimeEventFrame::Terminal { .. }) | None) => {
                if target_is_current(&inner, &key, &lease).await {
                    let mut observation = lease.observation.lock().await;
                    if observation.consult_active && observation.terminal.is_none() {
                        observation.terminal = Some(TargetTerminal::Closed);
                    }
                    drop(observation);
                    lease.changed.notify_waiters();
                }
                flush_registry_push(&inner).await;
                break;
            }
            Err(error) => {
                tracing::error!(
                    %error,
                    session_key = key,
                    "collaboration target event overflow"
                );
                let mut observation = lease.observation.lock().await;
                observation.approval_denial_failed = true;
                if observation.consult_active && observation.terminal.is_none() {
                    observation.terminal = Some(TargetTerminal::Closed);
                }
                drop(observation);
                lease.changed.notify_waiters();
                flush_registry_push(&inner).await;
                break;
            }
        }
    }
}

async fn target_is_current(
    inner: &SessionRuntimeCollaborationInner,
    key: &str,
    expected: &Arc<TargetLease>,
) -> bool {
    inner
        .targets
        .lock()
        .await
        .get(key)
        .is_some_and(|current| Arc::ptr_eq(current, expected))
}

async fn observe_target_event(
    inner: &SessionRuntimeCollaborationInner,
    key: &str,
    lease: &Arc<TargetLease>,
    event: &EngineEvent,
) {
    if let EngineEventKind::ApprovalRequested { approval_id, .. } = &event.event
        && lease.mode == DomainMode::Consult
    {
        {
            let mut observation = lease.observation.lock().await;
            if observation.consult_active {
                observation.saw_activity = true;
            }
            if !lease.approval_gated || inner.shutting_down.is_cancelled() {
                observation.approval_denial_failed = true;
            }
        }
        lease.changed.notify_waiters();
        if inner.shutting_down.is_cancelled() {
            return;
        }
        let response = ApprovalResponse {
            note: Some(
                "Peer consultation runs under supervised read-only policy; Interface denied this tool."
                    .to_owned(),
            ),
            answers: Vec::new(),
        };
        let denied = tokio::time::timeout(
            Duration::from_secs(10),
            inner.runtime.respond_to_approval(
                key,
                approval_id.clone(),
                ApprovalVerdict::Deny,
                response,
            ),
        )
        .await
        .is_ok_and(|delivery| delivery_disposition(&delivery) == DeliveryDisposition::Succeeded);
        if !denied {
            lease.observation.lock().await.approval_denial_failed = true;
            lease.changed.notify_waiters();
        }
        return;
    }

    let mut observation = lease.observation.lock().await;
    if !observation.consult_active || observation.terminal.is_some() {
        return;
    }
    match &event.event {
        EngineEventKind::MessageDelta {
            text,
            agent_key: None,
        } => {
            push_bounded_utf8(&mut observation.answer, text, RESULT_BYTES);
            observation.saw_activity = true;
        }
        EngineEventKind::StatusChanged {
            status: SessionStatus::Working | SessionStatus::Waiting,
        } => observation.saw_activity = true,
        EngineEventKind::Error {
            retryable: false, ..
        }
        | EngineEventKind::StatusChanged {
            status: SessionStatus::Failed,
        } => observation.terminal = Some(TargetTerminal::Failed),
        EngineEventKind::StatusChanged {
            status: SessionStatus::Idle,
        } if observation.saw_activity => {
            observation.terminal = Some(if observation.answer.trim().is_empty() {
                TargetTerminal::Failed
            } else {
                TargetTerminal::Answer(observation.answer.clone())
            });
        }
        EngineEventKind::SessionClosed => observation.terminal = Some(TargetTerminal::Closed),
        _ => {}
    }
    let changed = observation.terminal.is_some()
        || matches!(
            event.event,
            EngineEventKind::MessageDelta { .. }
                | EngineEventKind::StatusChanged {
                    status: SessionStatus::Working | SessionStatus::Waiting
                }
        );
    drop(observation);
    if changed {
        lease.changed.notify_waiters();
    }
}

pub(super) fn push_bounded_utf8(current: &mut String, chunk: &str, max_bytes: usize) {
    current.push_str(chunk);
    if current.len() > max_bytes {
        let mut end = max_bytes;
        while end > 0 && !current.is_char_boundary(end) {
            end -= 1;
        }
        current.truncate(end);
    }
}

async fn push_runtime_event(
    inner: &SessionRuntimeCollaborationInner,
    session_key: &str,
    event: &EngineEvent,
) {
    let Some(pushes) = inner.pushes.get() else {
        return;
    };
    let Ok(frame) = session_event_frame(session_key, event) else {
        return;
    };
    let _ = pushes.send(frame).await;
}

fn schedule_registry_push(inner: &SessionRuntimeCollaborationInner) {
    if let Some(coordinator) = inner.registry_pushes.get() {
        let _ = coordinator.schedule();
    }
}

async fn flush_registry_push(inner: &SessionRuntimeCollaborationInner) {
    if let Some(coordinator) = inner.registry_pushes.get() {
        let _ = coordinator.flush().await;
        return;
    }
    if let Some(pushes) = inner.pushes.get() {
        let _ = emit_registry_from_runtime(inner.runtime.as_ref(), pushes).await;
    }
}
