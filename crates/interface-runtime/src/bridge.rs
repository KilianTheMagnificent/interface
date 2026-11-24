use interface_contracts::engine::{
    EngineId, EngineSessionListing, EngineUsage, LaunchOptions, ProviderCommandCatalogue,
    ProviderCommandScope, ResumeOptions, SessionSummary,
};
use interface_contracts::ipc::{EngineOpSupport, SessionListingSnapshot, SessionSearchResponse};
use interface_engine::{ApprovalResponse, ApprovalVerdict, DeliveryOutcome, EngineError};
use serde::{Deserialize, Serialize};

use crate::{
    HistorySession, ImportReport, ProviderReadiness, RuntimeDelivery, RuntimeDeliveryAudit,
    RuntimeError, RuntimeHistorySnapshot, RuntimeSendLiveOutcome, SessionOpenOutcome,
    SessionRuntime,
};

/// Typed, transport-neutral commands for a long-lived Rust host process.
///
/// Electron IPC, a local socket, and the native CLI can carry this same schema;
/// no command assumes a particular desktop shell or renderer implementation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "method", content = "params", rename_all = "camelCase")]
pub enum RuntimeBridgeCommand {
    RegisteredEngines,
    ProbeProvider {
        engine: EngineId,
    },
    ProbeAllProviders,
    ListSessions,
    ListSessionSnapshot,
    ListArchivedSessions,
    ListProviderCommands {
        scope: ProviderCommandScope,
    },
    RefreshProviderSessions {
        engine: EngineId,
    },
    StartSession {
        engine: EngineId,
        options: LaunchOptions,
    },
    ResumeSession {
        engine: EngineId,
        provider_session_id: String,
        cwd: String,
        options: ResumeOptions,
    },
    Send {
        session_key: String,
        text: String,
    },
    CancelSession {
        session_key: String,
    },
    RespondToApproval {
        session_key: String,
        approval_id: String,
        verdict: BridgeApprovalVerdict,
        response: BridgeApprovalResponse,
    },
    SetModel {
        session_key: String,
        model: Option<String>,
    },
    SetEffort {
        session_key: String,
        effort: Option<String>,
    },
    SetFastMode {
        session_key: String,
        enabled: bool,
    },
    SetPermissionMode {
        session_key: String,
        mode: String,
    },
    CloseSession {
        session_key: String,
    },
    ForgetSession {
        session_key: String,
    },
    ArchiveSession {
        session_key: String,
        archived: bool,
    },
    RenameSession {
        session_key: String,
        title: String,
    },
    DeleteSession {
        session_key: String,
    },
    ForkSession {
        session_key: String,
    },
    AdoptSession {
        session_key: String,
        adopt: bool,
    },
    SetEngineArchived {
        session_key: String,
        archived: bool,
    },
    ReadUsage,
    EngineOps,
    SearchSessions {
        query: String,
    },
    AdoptAll {
        adopt: bool,
    },
    SendLive {
        session_key: String,
        text: String,
    },
    HistorySession {
        session_key: String,
    },
    HistorySnapshot,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BridgeApprovalVerdict {
    Allow,
    Deny,
}

impl From<BridgeApprovalVerdict> for ApprovalVerdict {
    fn from(value: BridgeApprovalVerdict) -> Self {
        match value {
            BridgeApprovalVerdict::Allow => Self::Allow,
            BridgeApprovalVerdict::Deny => Self::Deny,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BridgeApprovalResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default)]
    pub answers: Vec<(String, String)>,
}

impl From<BridgeApprovalResponse> for ApprovalResponse {
    fn from(value: BridgeApprovalResponse) -> Self {
        Self {
            note: value.note,
            answers: value.answers,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeDeliveryDisposition {
    Confirmed,
    DefinitelyUnsent,
    DefinitelyRejected,
    OutcomeUnknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeDeliveryReceipt {
    pub disposition: RuntimeDeliveryDisposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<EngineError>,
    pub audit: RuntimeDeliveryAudit,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", content = "value", rename_all = "kebab-case")]
pub enum RuntimeSendLiveResponse {
    NotLive,
    Delivery(RuntimeDeliveryReceipt),
}

impl From<RuntimeDelivery> for RuntimeDeliveryReceipt {
    fn from(value: RuntimeDelivery) -> Self {
        let (disposition, error) = match value.outcome {
            DeliveryOutcome::Confirmed(()) => (RuntimeDeliveryDisposition::Confirmed, None),
            DeliveryOutcome::DefinitelyUnsent(error) => {
                (RuntimeDeliveryDisposition::DefinitelyUnsent, Some(error))
            }
            DeliveryOutcome::DefinitelyRejected(error) => {
                (RuntimeDeliveryDisposition::DefinitelyRejected, Some(error))
            }
            DeliveryOutcome::OutcomeUnknown(error) => {
                (RuntimeDeliveryDisposition::OutcomeUnknown, Some(error))
            }
        };
        Self {
            disposition,
            error,
            audit: value.audit,
        }
    }
}

impl RuntimeDeliveryReceipt {
    #[must_use]
    pub const fn replay_is_safe(&self) -> bool {
        matches!(
            self.disposition,
            RuntimeDeliveryDisposition::DefinitelyUnsent
        )
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "result", content = "value", rename_all = "camelCase")]
pub enum RuntimeBridgeResponse {
    RegisteredEngines(Vec<EngineId>),
    ProviderReadiness(ProviderReadiness),
    ProvidersReadiness(Vec<ProviderReadiness>),
    Sessions(Vec<SessionSummary>),
    SessionSnapshot(SessionListingSnapshot),
    ArchivedSessions(Vec<SessionSummary>),
    ProviderCommands(ProviderCommandCatalogue),
    ProviderSessions(EngineSessionListing),
    SessionOpen(SessionOpenOutcome),
    Delivery(RuntimeDeliveryReceipt),
    Acknowledged,
    ForkedSession { session_key: String },
    Usage(Vec<EngineUsage>),
    EngineOps(Vec<EngineOpSupport>),
    SessionSearch(SessionSearchResponse),
    ImportReport(ImportReport),
    SendLive(RuntimeSendLiveResponse),
    HistorySession(Box<Option<HistorySession>>),
    HistorySnapshot(RuntimeHistorySnapshot),
    Error(RuntimeError),
}

impl SessionRuntime {
    /// Dispatches one typed bridge command without exposing provider handles.
    ///
    /// Provider events are delivered separately through [`SessionRuntime::take_session_events`]
    /// and serialized as [`crate::RuntimeEvent`] by the host's IPC transport.
    #[allow(clippy::too_many_lines)]
    pub async fn handle_bridge_command(
        &self,
        command: RuntimeBridgeCommand,
    ) -> RuntimeBridgeResponse {
        match command {
            RuntimeBridgeCommand::RegisteredEngines => {
                RuntimeBridgeResponse::RegisteredEngines(self.registered_engines())
            }
            RuntimeBridgeCommand::ProbeProvider { engine } => {
                RuntimeBridgeResponse::ProviderReadiness(self.probe_provider(&engine).await)
            }
            RuntimeBridgeCommand::ProbeAllProviders => {
                RuntimeBridgeResponse::ProvidersReadiness(self.probe_all_providers().await)
            }
            RuntimeBridgeCommand::ListSessions => self.list_sessions().await.map_or_else(
                RuntimeBridgeResponse::Error,
                RuntimeBridgeResponse::Sessions,
            ),
            RuntimeBridgeCommand::ListSessionSnapshot => {
                self.list_session_snapshot().await.map_or_else(
                    RuntimeBridgeResponse::Error,
                    RuntimeBridgeResponse::SessionSnapshot,
                )
            }
            RuntimeBridgeCommand::ListArchivedSessions => {
                self.list_archived_sessions().await.map_or_else(
                    RuntimeBridgeResponse::Error,
                    RuntimeBridgeResponse::ArchivedSessions,
                )
            }
            RuntimeBridgeCommand::ListProviderCommands { scope } => {
                self.list_provider_commands(scope).await.map_or_else(
                    RuntimeBridgeResponse::Error,
                    RuntimeBridgeResponse::ProviderCommands,
                )
            }
            RuntimeBridgeCommand::RefreshProviderSessions { engine } => {
                self.refresh_provider_sessions(&engine).await.map_or_else(
                    RuntimeBridgeResponse::Error,
                    RuntimeBridgeResponse::ProviderSessions,
                )
            }
            RuntimeBridgeCommand::StartSession { engine, options } => {
                RuntimeBridgeResponse::SessionOpen(self.start_session(&engine, options).await)
            }
            RuntimeBridgeCommand::ResumeSession {
                engine,
                provider_session_id,
                cwd,
                options,
            } => RuntimeBridgeResponse::SessionOpen(
                self.resume_session(&engine, provider_session_id, cwd, options)
                    .await,
            ),
            RuntimeBridgeCommand::Send { session_key, text } => {
                RuntimeBridgeResponse::Delivery(self.send(&session_key, text).await.into())
            }
            RuntimeBridgeCommand::CancelSession { session_key } => {
                RuntimeBridgeResponse::Delivery(self.cancel_session(&session_key).await.into())
            }
            RuntimeBridgeCommand::RespondToApproval {
                session_key,
                approval_id,
                verdict,
                response,
            } => RuntimeBridgeResponse::Delivery(
                self.respond_to_approval(
                    &session_key,
                    approval_id,
                    verdict.into(),
                    response.into(),
                )
                .await
                .into(),
            ),
            RuntimeBridgeCommand::SetModel { session_key, model } => {
                RuntimeBridgeResponse::Delivery(self.set_model(&session_key, model).await.into())
            }
            RuntimeBridgeCommand::SetEffort {
                session_key,
                effort,
            } => {
                RuntimeBridgeResponse::Delivery(self.set_effort(&session_key, effort).await.into())
            }
            RuntimeBridgeCommand::SetFastMode {
                session_key,
                enabled,
            } => RuntimeBridgeResponse::Delivery(
                self.set_fast_mode(&session_key, enabled).await.into(),
            ),
            RuntimeBridgeCommand::SetPermissionMode { session_key, mode } => {
                RuntimeBridgeResponse::Delivery(
                    self.set_permission_mode(&session_key, mode).await.into(),
                )
            }
            RuntimeBridgeCommand::CloseSession { session_key } => {
                RuntimeBridgeResponse::Delivery(self.close_session(&session_key).await.into())
            }
            RuntimeBridgeCommand::ForgetSession { session_key } => {
                management_ack(self.forget_session(&session_key).await)
            }
            RuntimeBridgeCommand::ArchiveSession {
                session_key,
                archived,
            } => management_ack(self.archive_session(&session_key, archived).await),
            RuntimeBridgeCommand::RenameSession { session_key, title } => {
                management_ack(self.rename_stored_session(&session_key, title).await)
            }
            RuntimeBridgeCommand::DeleteSession { session_key } => {
                management_ack(self.delete_stored_session(&session_key).await)
            }
            RuntimeBridgeCommand::ForkSession { session_key } => self
                .fork_stored_session(&session_key)
                .await
                .map_or_else(RuntimeBridgeResponse::Error, |session_key| {
                    RuntimeBridgeResponse::ForkedSession { session_key }
                }),
            RuntimeBridgeCommand::AdoptSession { session_key, adopt } => {
                management_ack(self.adopt_stored_session(&session_key, adopt).await)
            }
            RuntimeBridgeCommand::SetEngineArchived {
                session_key,
                archived,
            } => management_ack(self.set_engine_archived(&session_key, archived).await),
            RuntimeBridgeCommand::ReadUsage => {
                RuntimeBridgeResponse::Usage(self.read_all_usage().await)
            }
            RuntimeBridgeCommand::EngineOps => RuntimeBridgeResponse::EngineOps(self.engine_ops()),
            RuntimeBridgeCommand::SearchSessions { query } => {
                self.search_sessions(query).await.map_or_else(
                    RuntimeBridgeResponse::Error,
                    RuntimeBridgeResponse::SessionSearch,
                )
            }
            RuntimeBridgeCommand::AdoptAll { adopt } => {
                self.adopt_all(adopt, None).await.map_or_else(
                    RuntimeBridgeResponse::Error,
                    RuntimeBridgeResponse::ImportReport,
                )
            }
            RuntimeBridgeCommand::SendLive { session_key, text } => {
                let response = match self.send_live(&session_key, text).await {
                    RuntimeSendLiveOutcome::NotLive => RuntimeSendLiveResponse::NotLive,
                    RuntimeSendLiveOutcome::Delivery(delivery) => {
                        RuntimeSendLiveResponse::Delivery(delivery.into())
                    }
                };
                RuntimeBridgeResponse::SendLive(response)
            }
            RuntimeBridgeCommand::HistorySession { session_key } => self
                .history_session(&session_key)
                .await
                .map_or_else(RuntimeBridgeResponse::Error, |history| {
                    RuntimeBridgeResponse::HistorySession(Box::new(history))
                }),
            RuntimeBridgeCommand::HistorySnapshot => self.history_snapshot().await.map_or_else(
                RuntimeBridgeResponse::Error,
                RuntimeBridgeResponse::HistorySnapshot,
            ),
        }
    }
}

fn management_ack(result: Result<(), RuntimeError>) -> RuntimeBridgeResponse {
    match result {
        Ok(()) => RuntimeBridgeResponse::Acknowledged,
        Err(error) => RuntimeBridgeResponse::Error(error),
    }
}
