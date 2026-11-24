#![allow(
    clippy::missing_errors_doc,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::type_complexity
)]

mod projection;
pub use projection::{
    DesktopCollaborationProjection, HeldPeerProjection, ProjectedSendOutcome, ProjectedSourceState,
};
mod port;
pub use port::SessionRuntimeCollaborationPort;
pub(crate) use port::combine_provider_and_live_transcript;
mod service;
pub use service::{DeferredCollaborationModelIngress, DurableCollaborationService};
mod translation;
#[cfg(test)]
use translation::state_dto;
use translation::{
    decode, decode_approval, decode_draft, decode_empty, decode_peer, decode_submit, encode,
    engine_domain, mode_domain, status_dto, validate_identifier,
};

use std::{path::Path, sync::Arc};

use async_trait::async_trait;
use interface_collaboration::{
    ApprovalPreview, EffectCommand, EffectOutcome, PacketBuildResult, PendingPeer,
    PendingPeerState, SessionParticipants, SourceSnapshot,
    coordinator::HumanPreviewMaterial,
    state::{
        CollaborationEngineId, CollaborationErrorCode, CollaborationMode as DomainMode,
        CollaborationReceiptV1, CollaborationRecordV1, CollaborationState as DomainState,
    },
};
use interface_contracts::{
    engine::EngineEvent,
    ipc::{
        CHANNELS, CollaborationApprovalAction, CollaborationApprovalDto,
        CollaborationDraftStateUpdate, CollaborationEngine, CollaborationErrorDto,
        CollaborationMode, CollaborationPeerAction, CollaborationPeerDto, CollaborationPeerState,
        CollaborationReceiptDto, CollaborationState, CollaborationStatusDto,
        CollaborationSubmitRequest, parse_key,
    },
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::dispatcher::{DispatchError, HostDispatcher, INVALID_PAYLOAD_CODE};

pub const COLLABORATION_OPERATIONS: [&str; 9] = [
    CHANNELS.list_collaborations,
    CHANNELS.submit_collaboration,
    CHANNELS.get_collaboration_status,
    CHANNELS.approve_collaboration,
    CHANNELS.deny_collaboration,
    CHANNELS.accept_collaboration_peer,
    CHANNELS.discard_collaboration_peer,
    CHANNELS.cancel_collaboration,
    CHANNELS.set_collaboration_draft_state,
];

const INVALID_REQUEST_MESSAGE: &str =
    "Interface received an invalid collaboration request; no provider action was taken.";
const UNAVAILABLE_MESSAGE: &str = "Cross-model collaboration is unavailable. Restart Interface after checking its private app-data permissions; no provider action was taken.";
const MAX_IDENTIFIER_BYTES: usize = 512;
const MAX_LIVE_TRANSCRIPT_EVENTS: usize = 512;

#[derive(Clone, Debug, thiserror::Error)]
#[error("{message}")]
pub struct CollaborationServiceError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub replay_safe: bool,
}

impl CollaborationServiceError {
    fn invalid_request() -> Self {
        Self {
            code: INVALID_PAYLOAD_CODE.to_owned(),
            message: INVALID_REQUEST_MESSAGE.to_owned(),
            retryable: false,
            replay_safe: true,
        }
    }

    fn unavailable() -> Self {
        Self {
            code: "collaboration-unavailable".to_owned(),
            message: UNAVAILABLE_MESSAGE.to_owned(),
            retryable: true,
            replay_safe: true,
        }
    }

    fn encoding() -> Self {
        Self {
            code: "response-encoding".to_owned(),
            message: "The Rust collaboration result could not be encoded.".to_owned(),
            retryable: false,
            replay_safe: false,
        }
    }

    fn domain(error: interface_collaboration::CoordinatorError) -> Self {
        match error {
            interface_collaboration::CoordinatorError::Ingress {
                code, retryable, ..
            } => Self {
                code: format!("COLLABORATION_{}", code.as_str()),
                message: code.safe_next_action().to_owned(),
                retryable,
                replay_safe: !code.is_ambiguous(),
            },
            other => Self {
                code: "COLLABORATION_STATE_CORRUPT".to_owned(),
                message: CollaborationErrorCode::StateCorrupt
                    .safe_next_action()
                    .to_owned(),
                retryable: false,
                replay_safe: false,
            }
            .with_diagnostic(&other.to_string()),
        }
    }

    fn with_diagnostic(self, diagnostic: &str) -> Self {
        tracing::error!(diagnostic, "collaboration domain failure");
        self
    }
}

impl From<CollaborationServiceError> for DispatchError {
    fn from(error: CollaborationServiceError) -> Self {
        Self::new(
            error.code,
            error.message,
            error.retryable,
            error.replay_safe,
        )
    }
}

/// Central fence and source-projection hooks used by ordinary session owners.
///
/// Collaboration targets and unsettled sources are host-controlled. Keeping
/// this one interface shared by the live runtime, session-management, and
/// worktree owners prevents a newly ported command from bypassing the durable
/// coordinator merely because it lives in another dispatcher.
#[async_trait]
pub trait CollaborationSessionHooks: Send + Sync + 'static {
    async fn guard_session_mutation(
        &self,
        session_key: &str,
        action: &str,
        allow_safe_stop: bool,
    ) -> Result<(), DispatchError>;

    async fn guard_bulk_mutation(&self, action: &str) -> Result<(), DispatchError>;

    fn user_send_started(&self, session_key: &str) -> Result<(), DispatchError>;

    fn user_send_settled(
        &self,
        session_key: &str,
        text: &str,
        outcome: ProjectedSendOutcome,
    ) -> Result<(), DispatchError>;

    fn observe_event(&self, session_key: &str, event: &EngineEvent) -> Result<(), DispatchError>;

    fn observe_registry_change(&self, session_key: &str) -> Result<(), DispatchError>;
}

#[async_trait]
pub trait CollaborationDesktopControl: Send + Sync + 'static {
    async fn list(&self) -> Result<Vec<CollaborationStatusDto>, CollaborationServiceError>;
    async fn submit(
        &self,
        input: CollaborationSubmitRequest,
    ) -> Result<CollaborationStatusDto, CollaborationServiceError>;
    async fn status(
        &self,
        collaboration_id: String,
    ) -> Result<CollaborationStatusDto, CollaborationServiceError>;
    async fn approve(
        &self,
        action: CollaborationApprovalAction,
    ) -> Result<CollaborationStatusDto, CollaborationServiceError>;
    async fn deny(
        &self,
        action: CollaborationApprovalAction,
    ) -> Result<CollaborationStatusDto, CollaborationServiceError>;
    async fn accept_peer(
        &self,
        action: CollaborationPeerAction,
    ) -> Result<CollaborationStatusDto, CollaborationServiceError>;
    async fn discard_peer(
        &self,
        action: CollaborationPeerAction,
    ) -> Result<CollaborationStatusDto, CollaborationServiceError>;
    async fn cancel(
        &self,
        collaboration_id: String,
    ) -> Result<CollaborationStatusDto, CollaborationServiceError>;
    fn set_draft_state(
        &self,
        update: CollaborationDraftStateUpdate,
    ) -> Result<(), CollaborationServiceError>;
    async fn close(&self) -> Result<(), CollaborationServiceError>;
}

pub struct CollaborationHostDispatcher {
    control: Option<Arc<dyn CollaborationDesktopControl>>,
}

impl CollaborationHostDispatcher {
    #[must_use]
    pub const fn new(control: Arc<dyn CollaborationDesktopControl>) -> Self {
        Self {
            control: Some(control),
        }
    }

    /// A dispatcher with no collaboration service: every operation decodes its
    /// payload, then refuses with `CollaborationServiceError::unavailable()`.
    #[must_use]
    pub const fn unavailable() -> Self {
        Self { control: None }
    }

    fn control(&self) -> Result<&Arc<dyn CollaborationDesktopControl>, CollaborationServiceError> {
        self.control
            .as_ref()
            .ok_or_else(CollaborationServiceError::unavailable)
    }

    pub async fn close(&self) -> Result<(), DispatchError> {
        if let Some(control) = &self.control {
            control.close().await?;
        }
        Ok(())
    }
}

#[async_trait]
impl HostDispatcher for CollaborationHostDispatcher {
    async fn dispatch(&self, operation: &str, payload: Value) -> Result<Value, DispatchError> {
        let result = match operation {
            op if op == CHANNELS.list_collaborations => {
                decode_empty(&payload)?;
                encode(self.control()?.list().await?)
            }
            op if op == CHANNELS.submit_collaboration => {
                let input = decode_submit(payload)?;
                encode(self.control()?.submit(input).await?)
            }
            op if op == CHANNELS.get_collaboration_status => {
                let input = decode::<CollaborationIdPayload>(payload)?;
                validate_identifier(&input.collaboration_id)?;
                encode(self.control()?.status(input.collaboration_id).await?)
            }
            op if op == CHANNELS.approve_collaboration => {
                let action = decode_approval(payload)?;
                encode(self.control()?.approve(action).await?)
            }
            op if op == CHANNELS.deny_collaboration => {
                let action = decode_approval(payload)?;
                encode(self.control()?.deny(action).await?)
            }
            op if op == CHANNELS.accept_collaboration_peer => {
                let action = decode_peer(payload)?;
                encode(self.control()?.accept_peer(action).await?)
            }
            op if op == CHANNELS.discard_collaboration_peer => {
                let action = decode_peer(payload)?;
                encode(self.control()?.discard_peer(action).await?)
            }
            op if op == CHANNELS.cancel_collaboration => {
                let input = decode::<CollaborationIdPayload>(payload)?;
                validate_identifier(&input.collaboration_id)?;
                encode(self.control()?.cancel(input.collaboration_id).await?)
            }
            op if op == CHANNELS.set_collaboration_draft_state => {
                let update = decode_draft(payload)?;
                self.control()?.set_draft_state(update)?;
                Ok(Value::Null)
            }
            _ => Err(CollaborationServiceError {
                code: "operation-not-owned".to_owned(),
                message: "That operation is not owned by the Rust collaboration service."
                    .to_owned(),
                retryable: false,
                replay_safe: true,
            }),
        };
        result.map_err(Into::into)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CollaborationIdPayload {
    collaboration_id: String,
}

#[derive(Clone, Debug, thiserror::Error)]
#[error("{message}")]
pub struct CollaborationPortError {
    pub code: CollaborationErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl CollaborationPortError {
    #[must_use]
    pub fn new(code: CollaborationErrorCode, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
        }
    }
}

/// The refusal for a consult or handoff whose source belongs to an agent.
pub const AGENT_SOURCE_REFUSED: &str = "Agents cannot consult or hand off to another model: their work stays behind the agent approval gate.";

/// Agent sessions run behind the approval gate. A consult or handoff from one would start its
/// target on the raw runtime, ungated and in the agent's home, so the broker refuses them.
pub trait CollaborationAgentSessions: Send + Sync + 'static {
    /// Whether `session_key`, working in `canonical_cwd`, belongs to an agent: its current
    /// thread or a run, a past thread, or any session inside the agents root.
    fn is_agent_session(&self, session_key: &str, canonical_cwd: &Path) -> bool;
}

#[async_trait]
pub trait CollaborationRuntimePort: Send + Sync + 'static {
    async fn resolve_source(
        &self,
        session_key: &str,
        runtime_epoch: &str,
        projection: &DesktopCollaborationProjection,
    ) -> Result<Option<SourceSnapshot>, CollaborationPortError>;

    async fn check_target(
        &self,
        source: &SourceSnapshot,
        target: CollaborationEngineId,
        mode: DomainMode,
    ) -> Result<(), CollaborationPortError>;

    async fn build_packet(
        &self,
        material: &HumanPreviewMaterial,
        source: &SourceSnapshot,
        projection: &DesktopCollaborationProjection,
    ) -> Result<PacketBuildResult, CollaborationPortError>;

    async fn execute(
        &self,
        command: EffectCommand,
        cancellation: CancellationToken,
    ) -> EffectOutcome;

    async fn hold_peer(
        &self,
        peer: &PendingPeer,
        projection: &DesktopCollaborationProjection,
    ) -> Result<(), CollaborationPortError> {
        projection
            .hold_peer(HeldPeerProjection {
                collaboration_id: peer.collaboration_id.clone(),
                queue_entry_id: peer.queue_entry_id.clone(),
                result_id: peer.result_id.clone(),
                result_digest: peer.result_digest.clone(),
                source_session_key: peer.source_session_key.clone(),
                source_engine: peer.source_engine,
                target_session_key: peer.target_session_key.clone(),
                target_engine: peer.target_engine,
                objective: peer.objective.clone(),
                body: peer.body.clone(),
            })
            .map_err(|error| {
                CollaborationPortError::new(
                    CollaborationErrorCode::StateCorrupt,
                    error.message,
                    false,
                )
            })
    }

    async fn remove_held_peer(
        &self,
        peer: &PendingPeer,
        projection: &DesktopCollaborationProjection,
    ) -> Result<(), CollaborationPortError> {
        projection
            .remove_peer(
                &peer.source_session_key,
                &peer.collaboration_id,
                &peer.queue_entry_id,
                &peer.result_id,
            )
            .map_err(|error| {
                CollaborationPortError::new(
                    CollaborationErrorCode::StateCorrupt,
                    error.message,
                    false,
                )
            })
    }

    async fn release_target(
        &self,
        _record: &CollaborationRecordV1,
        _cancellation: CancellationToken,
    ) -> Result<(), CollaborationPortError> {
        Ok(())
    }

    async fn dispose_target(
        &self,
        _record: &CollaborationRecordV1,
        _cancellation: CancellationToken,
    ) -> Result<(), CollaborationPortError> {
        Ok(())
    }

    async fn shutdown(&self) -> Result<(), CollaborationPortError> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "collaboration_tests.rs"]
mod tests;
