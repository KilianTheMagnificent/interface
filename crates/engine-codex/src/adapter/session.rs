//! The live session: its shared state, the `SessionHandle` implementation,
//! closing, and the inbound loop that routes the app-server's frames.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, atomic::Ordering},
};

use async_trait::async_trait;
use interface_common::sync::lock;
use interface_contracts::{
    engine::{
        CollaborationLease, CollaborationMcpStdioDescriptor, EngineEvent, EngineEventKind,
        EngineId, NoticeTone, SessionStatus,
    },
    provider_commands::app_owned_provider_command_error,
};
use interface_engine::{
    ApprovalResponse, ApprovalVerdict, BoundedEventStream, DeliveryOutcome, EngineError,
    EngineErrorKind, ReadySession, SessionHandle,
    jsonrpc::RouteEvent,
    session_core::{ReadyState, SessionCore},
    supervised::InboundReceiver,
};
use serde_json::Value;
use tokio::sync::{Mutex as AsyncMutex, watch};

use crate::{
    mapper::CodexEventMapper,
    protocol::{CodexMethod, CodexSessionState},
    transport::{CodexTransport, CodexTransportEvent},
};

use super::{
    AgentMode,
    approvals::{
        PendingApproval, approval_result, denial_result, deny_pending_approvals,
        handle_server_request, note_file_change_paths,
    },
    commands::{CommandRefreshState, SessionCommands},
    config::{ConfigChange, set_config, set_fast_mode},
    dispatch::handle_command_notification,
    models::FastTierCatalogue,
    turns::{note_turn_notification, send_prompt},
};

#[derive(Clone, Debug)]
pub(super) struct SessionConfig {
    pub(super) model: Option<String>,
    pub(super) resolved_model: Option<String>,
    pub(super) effort: Option<String>,
    /// None inherits the thread/provider setting, Some(None) explicitly
    /// restores Standard, and Some(Some(id)) selects that exact Fast tier.
    pub(super) service_tier: ServiceTierOverride,
    /// Only a mid-session override is repeated on turn/start.
    pub(super) approval_override: Option<String>,
    pub(super) approval_display: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) enum ServiceTierOverride {
    #[default]
    Inherit,
    Standard,
    Exact(String),
}

/// Required acquisition order: `config` → `fast_tiers` → `commands`, and
/// `config` → `protocol`. Nesting is rare — `set_config` takes `fast_tiers`,
/// `commands` and `protocol` while holding `config`, and opening takes
/// `fast_tiers` under `config` — but taking them in any other order
/// deadlocks. `core.emit` takes the core's own locks (see [`SessionCore`]);
/// it is never called while `commands` is held. No guard here is ever held
/// across an `.await`.
pub(super) struct SessionShared {
    pub(super) core: SessionCore<CodexTransportEvent>,
    pub(super) engine_id: EngineId,
    pub(super) session_id: Mutex<String>,
    pub(super) protocol: Mutex<CodexSessionState>,
    pub(super) config: Mutex<SessionConfig>,
    pub(super) cwd: String,
    pub(super) commands: Mutex<SessionCommands>,
    pub(super) command_refresh: watch::Sender<CommandRefreshState>,
    pub(super) pending_approvals: Mutex<HashMap<String, PendingApproval>>,
    pub(super) fast_tiers: Arc<Mutex<FastTierCatalogue>>,
    pub(super) effect_lock: AsyncMutex<()>,
    pub(super) collaboration_depth: u8,
    pub(super) collaboration_lease: Mutex<Option<Arc<dyn CollaborationLease>>>,
    pub(super) collaboration_source_key: Mutex<Option<String>>,
    /// Host-context block owed to a created thread's first turn. Resume hands
    /// the same instruction to `thread.resume` as `developer_instructions`.
    /// Taken exactly once: a turn whose outcome is unknown is never replayed.
    pub(super) pending_host_context: Mutex<Option<String>>,
    /// A gated agent session: Interface's policy answers every approval, so
    /// the approval policy can never be relaxed mid-session.
    pub(super) approval_gate: bool,
    /// Any agent session: its approval policy comes from the agent's settings.
    pub(super) agent_mode: Option<AgentMode>,
    pub(super) agent_tools: Option<CollaborationMcpStdioDescriptor>,
    /// The host's team tools factory, set only for a thread that may carry
    /// them (an ordinary code thread one hop up).
    pub(super) team_tools: Option<Arc<dyn interface_contracts::engine::TeamToolsFactory>>,
    /// This thread's `iface_team` binding, revoked when the session ends.
    pub(super) team_binding: Mutex<Option<Arc<interface_engine::team_tools::TeamBinding>>>,
    /// `itemId` → the paths its file change touches (`None` once any change
    /// was unreadable). Never nested with another lock.
    pub(super) file_change_paths: Mutex<VecDeque<(String, Option<Vec<String>>)>>,
    /// The connection's frame bound; a prompt no frame can carry is refused.
    pub(super) max_frame_bytes: usize,
}

pub(super) struct CodexSessionHandle {
    pub(super) shared: Arc<SessionShared>,
    pub(super) ready: watch::Receiver<ReadyState>,
    pub(super) events: Option<BoundedEventStream<EngineEvent>>,
}

#[async_trait]
impl SessionHandle for CodexSessionHandle {
    fn engine(&self) -> &EngineId {
        &self.shared.engine_id
    }

    fn session_id(&self) -> String {
        lock(&self.shared.session_id).clone()
    }

    async fn ready(&self) -> Result<ReadySession, EngineError> {
        self.shared.core.wait_ready(self.ready.clone()).await
    }

    fn take_events(&mut self) -> Result<BoundedEventStream<EngineEvent>, EngineError> {
        self.events.take().ok_or_else(|| {
            EngineError::definitely_unsent(
                EngineErrorKind::Lifecycle,
                "Codex event stream was already taken",
                false,
            )
        })
    }

    async fn send(&self, text: String) -> DeliveryOutcome<()> {
        if let Some(message) = app_owned_provider_command_error(&text) {
            return DeliveryOutcome::DefinitelyUnsent(EngineError::definitely_unsent(
                EngineErrorKind::InvalidInput,
                message,
                false,
            ));
        }
        if let Err(error) = self.shared.core.wait_ready(self.ready.clone()).await {
            return DeliveryOutcome::from_error(error);
        }
        send_prompt(&self.shared, text, false).await
    }

    async fn interrupt(&self) -> DeliveryOutcome<()> {
        if let Err(error) = self.shared.core.wait_ready(self.ready.clone()).await {
            return DeliveryOutcome::from_error(error);
        }
        let _guard = self.shared.effect_lock.lock().await;
        if self.shared.core.is_closed() {
            return self.shared.core.closed_outcome("interrupt");
        }
        let transport = match self.shared.core.transport() {
            Ok(transport) => transport,
            Err(error) => return DeliveryOutcome::from_error(error),
        };
        deny_pending_approvals(&self.shared, &transport, Some("Interrupted")).await;
        let params = match lock(&self.shared.protocol).interrupt_params() {
            Ok(Some(params)) => params,
            Ok(None) => return DeliveryOutcome::Confirmed(()),
            Err(error) => return DeliveryOutcome::from_error(error),
        };
        deny_pending_approvals(&self.shared, &transport, Some("Interrupted")).await;
        let turn_id = params.turn_id.clone();
        let outcome = transport
            .request(CodexMethod::TURN_INTERRUPT, &params)
            .await
            .map(|_| ());
        if outcome.is_confirmed() {
            lock(&self.shared.protocol).note_turn_interrupted(&turn_id);
        }
        outcome
    }

    async fn close(&self) -> Result<(), EngineError> {
        close_shared(&self.shared).await;
        Ok(())
    }

    async fn respond_to_approval(
        &self,
        approval_id: String,
        verdict: ApprovalVerdict,
        response: ApprovalResponse,
    ) -> DeliveryOutcome<()> {
        if self.shared.core.is_closed() {
            return self.shared.core.closed_outcome("answer approval");
        }
        let pending = {
            let mut approvals = lock(&self.shared.pending_approvals);
            match approvals.get_mut(&approval_id) {
                Some(pending) if !pending.responding => {
                    pending.responding = true;
                    Some(pending.clone())
                }
                _ => None,
            }
        };
        let Some(pending) = pending else {
            return DeliveryOutcome::DefinitelyUnsent(EngineError::definitely_unsent(
                EngineErrorKind::Lifecycle,
                format!("Codex approval {approval_id} is not pending"),
                false,
            ));
        };
        let outcome = async {
            let result = approval_result(&pending, verdict, &response);
            let transport = match self.shared.core.transport() {
                Ok(transport) => transport,
                Err(error) => return DeliveryOutcome::from_error(error),
            };
            transport.server_result(&pending.rpc_id, result).await
        }
        .await;
        if matches!(outcome, DeliveryOutcome::DefinitelyUnsent(_)) {
            // No byte reached the server, so its request is still open: keep
            // it answerable by a retry unless turn cleanup already cleared it.
            if let Some(entry) = lock(&self.shared.pending_approvals).get_mut(&approval_id) {
                entry.responding = false;
            }
        } else {
            let (removed, none_left) = {
                let mut approvals = lock(&self.shared.pending_approvals);
                let removed = approvals.remove(&approval_id).is_some();
                (removed, approvals.is_empty())
            };
            if outcome.is_confirmed() && removed && none_left {
                let _ = self.shared.core.emit(EngineEvent {
                    at: None,
                    event: EngineEventKind::StatusChanged {
                        status: SessionStatus::Working,
                    },
                });
            }
        }
        outcome
    }

    async fn set_model(&self, model: Option<String>) -> DeliveryOutcome<()> {
        set_config(&self.shared, ConfigChange::Model(model))
    }

    async fn set_effort(&self, effort: Option<String>) -> DeliveryOutcome<()> {
        set_config(&self.shared, ConfigChange::Effort(effort))
    }

    async fn set_fast_mode(&self, enabled: bool) -> DeliveryOutcome<()> {
        if let Err(error) = self.shared.core.wait_ready(self.ready.clone()).await {
            return DeliveryOutcome::from_error(error);
        }
        set_fast_mode(&self.shared, enabled).await
    }

    async fn set_permission_mode(&self, mode: String) -> DeliveryOutcome<()> {
        set_config(&self.shared, ConfigChange::Permission(mode))
    }
}

pub(super) async fn dispose_session_collaboration(shared: &Arc<SessionShared>) {
    let team = lock(&shared.team_binding).take();
    if let Some(team) = team {
        team.dispose().await;
    }
    let lease = lock(&shared.collaboration_lease).take();
    lock(&shared.collaboration_source_key).take();
    if let Some(lease) = lease {
        let _ = lease.dispose().await;
    }
}

pub(super) async fn close_shared(shared: &Arc<SessionShared>) {
    if shared.core.closed.swap(true, Ordering::AcqRel) {
        dispose_session_collaboration(shared).await;
        return;
    }
    lock(&shared.core.lifecycle).begin_close();
    dispose_session_collaboration(shared).await;
    let pending = std::mem::take(&mut *lock(&shared.pending_approvals));
    if let Ok(transport) = shared.core.transport() {
        for (_, pending) in pending {
            if pending.responding {
                continue;
            }
            let result = denial_result(&pending.method, None);
            let _ = transport.server_result(&pending.rpc_id, result).await;
        }
        transport.shutdown();
    }
    let _ = shared.core.emit(EngineEvent {
        at: None,
        event: EngineEventKind::SessionClosed,
    });
    shared.core.events.close();
    let _ = lock(&shared.core.lifecycle).mark_closed();
    shared
        .core
        .fail_pending_readiness(EngineError::definitely_unsent(
            EngineErrorKind::Lifecycle,
            "Codex session was closed before readiness",
            false,
        ));
}

pub(super) async fn session_inbound_loop(
    shared: Arc<SessionShared>,
    mut inbound: InboundReceiver<CodexTransportEvent>,
    transport: Arc<CodexTransport>,
) {
    let mut mapper = CodexEventMapper::default();
    let mut engine_events = Vec::new();
    'inbound: while let Some(event) = inbound.recv().await {
        if shared.core.is_closed() {
            break;
        }
        match event {
            CodexTransportEvent::Route(RouteEvent::Notification(notification)) => {
                if !notification_matches(
                    &shared,
                    &notification.method,
                    notification.params.as_ref(),
                ) {
                    continue;
                }
                handle_command_notification(
                    &shared,
                    &notification.method,
                    notification.params.as_ref(),
                );
                note_file_change_paths(&shared, &notification.method, notification.params.as_ref());
                note_turn_notification(&shared, &notification.method, notification.params.as_ref());
                engine_events.clear();
                mapper.map_into(
                    &mut engine_events,
                    &notification.method,
                    notification.params.as_ref(),
                );
                for engine_event in engine_events.drain(..) {
                    if matches!(
                        engine_event.event,
                        EngineEventKind::StatusChanged {
                            status: SessionStatus::Idle | SessionStatus::Failed,
                        }
                    ) {
                        lock(&shared.pending_approvals).clear();
                    }
                    if !shared.core.emit(engine_event) {
                        break 'inbound;
                    }
                }
            }
            CodexTransportEvent::Route(RouteEvent::ServerRequest(request)) => {
                handle_server_request(&shared, &transport, request).await;
            }
            CodexTransportEvent::Route(
                RouteEvent::ResponseSettled { .. } | RouteEvent::Diagnostic(_),
            ) => {}
            CodexTransportEvent::ProtocolFault(error) => {
                let _ = shared.core.emit(EngineEvent {
                    at: None,
                    event: EngineEventKind::Notice {
                        message: format!("Codex ignored a malformed protocol frame: {error}"),
                        tone: interface_contracts::engine::NoticeTone::Warning,
                    },
                });
            }
            CodexTransportEvent::Closed(message) => {
                for event in mapper.settle_open("Codex process ended") {
                    let _ = shared.core.emit(event);
                }
                shared.core.fail_transport(message);
                break;
            }
        }
    }
    shared
        .core
        .fail_transport("Codex inbound routing ended unexpectedly".into());
    dispose_session_collaboration(&shared).await;
}

pub(super) fn emit_status(shared: &SessionShared, status: SessionStatus) {
    let _ = shared.core.emit(EngineEvent {
        at: None,
        event: EngineEventKind::StatusChanged { status },
    });
}

pub(super) fn emit_notice(shared: &SessionShared, message: impl Into<String>, tone: NoticeTone) {
    let _ = shared.core.emit(EngineEvent {
        at: None,
        event: EngineEventKind::Notice {
            message: message.into(),
            tone,
        },
    });
}

fn notification_matches(shared: &SessionShared, method: &str, params: Option<&Value>) -> bool {
    let protocol = lock(&shared.protocol);
    let Some(expected) = protocol.thread_id() else {
        return true;
    };
    let actual = params
        .and_then(Value::as_object)
        .and_then(|params| params.get("threadId"))
        .and_then(Value::as_str)
        .or_else(|| {
            (method == "thread/started")
                .then(|| {
                    params?
                        .as_object()?
                        .get("thread")?
                        .as_object()?
                        .get("id")?
                        .as_str()
                })
                .flatten()
        });
    actual.is_none_or(|actual| actual == expected)
}
