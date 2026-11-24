//! Frozen 0.1.169 CLI-local collaboration lifetime.
//!
//! The interactive CLI owns this journal under `~/.config/iface/collaboration`
//! while holding `instance.lock`. Provider bindings are created before the
//! adapters, then the exact runtime port is attached before any provider is
//! probed or launched.

use std::{path::Path, sync::Arc};

use interface_collaboration::{
    CollaborationAuditSink, CollaborationBrokerRuntime, CollaborationBrokerRuntimeOptions,
    CollaborationCoordinator, CollaborationJournal, CollaborationModelIngress,
    state::CollaborationPolicyOverrides,
};
use interface_contracts::{
    canonical_timestamp,
    engine::{CollaborationProviderBindings, EngineEvent},
    ipc::{
        CollaborationApprovalAction, CollaborationDraftStateUpdate, CollaborationPeerAction,
        CollaborationStatusDto, CollaborationSubmitRequest,
    },
};
use interface_host::collaboration::{
    CollaborationDesktopControl, CollaborationServiceError, CollaborationSessionHooks,
    DeferredCollaborationModelIngress, DesktopCollaborationProjection, DurableCollaborationService,
    ProjectedSendOutcome, SessionRuntimeCollaborationPort,
};
use interface_host::protocol::PushFrame;
use interface_runtime::SessionRuntime;
use tokio::sync::{Mutex, mpsc};
use uuid::Uuid;

type Service = DurableCollaborationService<SessionRuntimeCollaborationPort>;

/// Result of the initial journal/broker boundary. A transport failure may
/// degrade safely only when every recovered record is terminal and the writer
/// lease can be cleanly closed.
pub enum CollaborationPreparation {
    Ready(Box<PreparedCliCollaboration>),
    SafeUnavailable { diagnostic: String },
}

pub struct PreparedCliCollaboration {
    journal: CollaborationJournal,
    audit: CollaborationAuditSink,
    deferred: Arc<DeferredCollaborationModelIngress>,
    broker: Arc<CollaborationBrokerRuntime>,
    bindings: CollaborationProviderBindings,
    runtime_epoch: String,
}

impl PreparedCliCollaboration {
    #[must_use]
    pub const fn provider_bindings(&self) -> &CollaborationProviderBindings {
        &self.bindings
    }

    /// Attaches the already-bound provider transport to the exact CLI runtime
    /// and recovers the durable coordinator before any provider can launch.
    ///
    /// # Errors
    ///
    /// Fails closed when durable recovery or ingress attachment fails.
    pub async fn attach(
        self,
        runtime: Arc<SessionRuntime>,
    ) -> Result<CliCollaborationRuntime, String> {
        let Self {
            journal,
            audit,
            deferred,
            broker,
            bindings: _,
            runtime_epoch,
        } = self;
        let projection = Arc::new(DesktopCollaborationProjection::new());
        let port = Arc::new(SessionRuntimeCollaborationPort::new(
            runtime,
            Arc::clone(&projection),
        ));
        let (pushes, push_receiver) = mpsc::channel(64);
        if let Err(error) = port.set_pushes(pushes.clone()) {
            stop_and_drain(&broker).await;
            return Err(error.to_string());
        }
        let service = match Service::start(
            CollaborationCoordinator::with_generators(
                journal,
                Some(audit),
                CollaborationPolicyOverrides::default(),
                || canonical_timestamp(chrono::Utc::now()),
                |kind| format!("{kind}-{}", Uuid::new_v4()),
            ),
            Arc::clone(&port),
            projection,
            runtime_epoch,
            Some(pushes),
        )
        .await
        {
            Ok(service) => Arc::new(service),
            Err(error) => {
                stop_and_drain(&broker).await;
                return Err(error.to_string());
            }
        };
        let ingress: Arc<dyn CollaborationModelIngress> = service.clone();
        if let Err(error) = deferred.attach(ingress) {
            let stop = broker.stop();
            let _ = service.close().await;
            let _ = stop.await;
            let _ = broker.drain().await;
            return Err(error.to_string());
        }
        Ok(CliCollaborationRuntime {
            service,
            port,
            broker,
            pushes: Arc::new(Mutex::new(Some(push_receiver))),
        })
    }

    /// Tears down an initial broker when provider runtime construction failed.
    pub async fn rollback(self) {
        stop_and_drain(&self.broker).await;
        let mut journal = self.journal;
        let mut audit = self.audit;
        let _ = audit.close();
        let _ = journal.close();
    }
}

#[derive(Clone)]
pub struct CliCollaborationRuntime {
    service: Arc<Service>,
    port: Arc<SessionRuntimeCollaborationPort>,
    broker: Arc<CollaborationBrokerRuntime>,
    pushes: Arc<Mutex<Option<mpsc::Receiver<PushFrame>>>>,
}

#[allow(clippy::missing_errors_doc)]
impl CliCollaborationRuntime {
    /// Hands the CLI surface the sole collaboration push stream. The provider
    /// target pump remains host-owned; the TUI only projects these frames.
    pub async fn take_pushes(&self) -> Option<mpsc::Receiver<PushFrame>> {
        self.pushes.lock().await.take()
    }

    pub async fn list(&self) -> Result<Vec<CollaborationStatusDto>, String> {
        self.service.list().await.map_err(service_error)
    }

    pub async fn submit(
        &self,
        input: CollaborationSubmitRequest,
    ) -> Result<CollaborationStatusDto, String> {
        CollaborationDesktopControl::submit(self.service.as_ref(), input)
            .await
            .map_err(service_error)
    }

    pub async fn approve(
        &self,
        input: CollaborationApprovalAction,
    ) -> Result<CollaborationStatusDto, String> {
        self.service.approve(input).await.map_err(service_error)
    }

    pub async fn deny(
        &self,
        input: CollaborationApprovalAction,
    ) -> Result<CollaborationStatusDto, String> {
        self.service.deny(input).await.map_err(service_error)
    }

    pub async fn accept_peer(
        &self,
        input: CollaborationPeerAction,
    ) -> Result<CollaborationStatusDto, String> {
        self.service.accept_peer(input).await.map_err(service_error)
    }

    pub async fn discard_peer(
        &self,
        input: CollaborationPeerAction,
    ) -> Result<CollaborationStatusDto, String> {
        self.service
            .discard_peer(input)
            .await
            .map_err(service_error)
    }

    pub async fn cancel(&self, collaboration_id: String) -> Result<CollaborationStatusDto, String> {
        CollaborationDesktopControl::cancel(self.service.as_ref(), collaboration_id)
            .await
            .map_err(service_error)
    }

    pub fn set_draft_state(&self, update: CollaborationDraftStateUpdate) -> Result<(), String> {
        self.service.set_draft_state(update).map_err(service_error)
    }

    pub fn observe_event(&self, session_key: &str, event: &EngineEvent) -> Result<(), String> {
        self.service
            .observe_event(session_key, event)
            .map_err(service_error)
    }

    pub fn user_send_started(&self, session_key: &str) -> Result<(), String> {
        self.service
            .user_send_started(session_key)
            .map_err(service_error)
    }

    pub fn user_send_settled(
        &self,
        session_key: &str,
        text: &str,
        outcome: ProjectedSendOutcome,
    ) -> Result<(), String> {
        self.service
            .user_send_settled(session_key, text, outcome)
            .map_err(service_error)
    }

    pub fn activate_session(&self, session_key: &str) -> Result<(), String> {
        self.service
            .projection()
            .activate_session(session_key)
            .map_err(service_error)
    }

    pub fn observe_registry_change(&self, session_key: &str) -> Result<(), String> {
        self.port
            .observe_registry_change(session_key)
            .map_err(|error| error.to_string())
    }

    pub async fn is_locked_session(&self, session_key: &str) -> bool {
        self.port.is_locked_session(session_key).await
    }

    pub async fn guard_session_mutation(
        &self,
        session_key: &str,
        action: &str,
        allow_safe_stop: bool,
    ) -> Result<(), String> {
        CollaborationSessionHooks::guard_session_mutation(
            self.service.as_ref(),
            session_key,
            action,
            allow_safe_stop,
        )
        .await
        .map_err(|error| error.message)
    }

    /// Revokes broker ingress synchronously, then drains coordinator effects
    /// while provider handles are still alive.
    pub async fn quiesce_and_close(&self) -> Result<(), String> {
        let stopping = self.broker.stop();
        let service = self.service.close();
        let (broker, service) = tokio::join!(stopping, service);
        broker.map_err(|error| error.to_string())?;
        service.map_err(service_error)
    }

    /// Runs only after the provider runtime has shut down.
    pub async fn drain(&self) -> Result<(), String> {
        self.broker.drain().await.map_err(|error| error.to_string())
    }
}

/// Opens the CLI-local durable writer and broker before adapter construction.
///
/// # Errors
///
/// Fails closed for journal errors or for a broker failure while non-terminal
/// durable work may still require quarantine.
pub async fn prepare(root: &Path) -> Result<CollaborationPreparation, String> {
    let mut journal = CollaborationJournal::open(root).map_err(|error| error.to_string())?;
    let mut audit = CollaborationAuditSink::open(root).map_err(|error| error.to_string())?;
    let deferred = Arc::new(DeferredCollaborationModelIngress::new());
    let model_ingress: Arc<dyn CollaborationModelIngress> = deferred.clone();
    let broker = match CollaborationBrokerRuntime::start(CollaborationBrokerRuntimeOptions::frozen(
        root.to_path_buf(),
        model_ingress,
    ))
    .await
    {
        Ok(broker) => Arc::new(broker),
        Err(error) => {
            let all_terminal = journal
                .list()
                .map(|records| records.iter().all(|record| record.state.is_terminal()))
                .unwrap_or(false);
            let closed = journal.close().is_ok();
            let audit_closed = audit.close().is_ok();
            if all_terminal && closed && audit_closed {
                return Ok(CollaborationPreparation::SafeUnavailable {
                    diagnostic: error.to_string(),
                });
            }
            return Err(
                "Collaboration startup failed while durable state may still require quarantine."
                    .into(),
            );
        }
    };
    let bindings = broker.provider_bindings();
    Ok(CollaborationPreparation::Ready(Box::new(
        PreparedCliCollaboration {
            journal,
            audit,
            deferred,
            broker,
            bindings,
            runtime_epoch: Uuid::new_v4().to_string(),
        },
    )))
}

fn service_error(error: CollaborationServiceError) -> String {
    error.message
}

async fn stop_and_drain(broker: &CollaborationBrokerRuntime) {
    let _ = broker.stop().await;
    let _ = broker.drain().await;
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use interface_runtime::{EngineHistoryStore, HistoryLimits, RuntimeServices};

    use super::*;

    #[tokio::test]
    async fn two_phase_owner_recovers_before_provider_use_and_closes_in_order() {
        let temp = tempfile::tempdir().expect("temp");
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private temp");
        let collaboration_root = temp.path().join("collaboration");
        let prepared = match prepare(&collaboration_root).await.expect("prepare") {
            CollaborationPreparation::Ready(prepared) => prepared,
            CollaborationPreparation::SafeUnavailable { diagnostic } => {
                assert!(!diagnostic.is_empty());
                let sockets = collaboration_root.join("sockets");
                assert!(
                    !sockets.exists()
                        || std::fs::read_dir(sockets)
                            .expect("socket directory")
                            .next()
                            .is_none()
                );
                return;
            }
        };
        prepared
            .provider_bindings()
            .codex_descriptor
            .validate()
            .expect("provider descriptor");

        let runtime_root = temp.path().join("runtime");
        std::fs::create_dir(&runtime_root).expect("runtime root");
        std::fs::set_permissions(&runtime_root, std::fs::Permissions::from_mode(0o700))
            .expect("private runtime");
        let sessions = interface_registry::SessionRegistry::open(&runtime_root).expect("registry");
        let history_root = temp.path().join("history");
        std::fs::create_dir(&history_root).expect("history root");
        std::fs::set_permissions(&history_root, std::fs::Permissions::from_mode(0o700))
            .expect("private history");
        let history =
            EngineHistoryStore::open(&history_root, HistoryLimits::default()).expect("history");
        let runtime = SessionRuntime::builder(RuntimeServices::new(sessions, history))
            .build()
            .expect("runtime");
        let active = prepared
            .attach(Arc::new(runtime.clone()))
            .await
            .expect("attach");

        assert!(active.list().await.expect("list").is_empty());
        active.quiesce_and_close().await.expect("quiesce");
        runtime.shutdown().await.expect("runtime shutdown");
        active.drain().await.expect("broker drain");
        let sockets = collaboration_root.join("sockets");
        assert!(
            !sockets.exists()
                || std::fs::read_dir(sockets)
                    .expect("socket directory")
                    .next()
                    .is_none()
        );
    }
}
