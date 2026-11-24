//! Prompts and turns: sending a prompt, steering a running turn, starting a
//! new one, and the turn bookkeeping notifications drive.

use std::{borrow::Cow, sync::Arc};

use interface_common::sync::lock;
use interface_contracts::engine::{NoticeTone, SessionStatus};
use interface_engine::{DeliveryOutcome, EngineError, EngineErrorKind, jsonrpc::text_fits_frame};
use serde_json::Value;

use crate::{
    protocol::{CodexMethod, TextInput, TurnStartParams, TurnSteerParams, turn_id_from_response},
    transport::{CodexTransport, CodexTransportEvent},
};

use super::{
    MAX_PROMPT_BYTES,
    commands::{ensure_skill_catalogue, skill_command_key},
    dispatch::{command_notice, dispatch_provider_command},
    session::{ServiceTierOverride, SessionShared, emit_notice, emit_status},
};

fn thread_id(shared: &SessionShared) -> Result<String, EngineError> {
    lock(&shared.protocol)
        .thread_id()
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            EngineError::definitely_unsent(
                EngineErrorKind::Lifecycle,
                "Codex thread id is not ready",
                true,
            )
        })
}

pub(super) fn thread_request_target(
    shared: &SessionShared,
) -> Result<(Arc<CodexTransport>, String), EngineError> {
    Ok((shared.core.transport()?, thread_id(shared)?))
}

pub(super) fn report_control_failure<T>(
    shared: &SessionShared,
    outcome: &DeliveryOutcome<T>,
    unknown_message: impl Into<String>,
    failed_message: impl Into<String>,
) {
    if outcome.error().is_none() {
        return;
    }
    let unknown = matches!(outcome, DeliveryOutcome::OutcomeUnknown(_));
    emit_notice(
        shared,
        if unknown {
            unknown_message.into()
        } else {
            failed_message.into()
        },
        NoticeTone::Warning,
    );
    if !unknown {
        emit_status(shared, SessionStatus::Idle);
    }
}

pub(super) async fn start_new_turn(
    shared: &Arc<SessionShared>,
    input: Vec<TextInput>,
) -> DeliveryOutcome<()> {
    let thread_id = match thread_id(shared) {
        Ok(thread_id) => thread_id,
        Err(error) => return DeliveryOutcome::from_error(error),
    };
    let transport = match shared.core.transport() {
        Ok(transport) => transport,
        Err(error) => return DeliveryOutcome::from_error(error),
    };
    let config = lock(&shared.config).clone();
    let outcome = transport
        .request(
            CodexMethod::TURN_START,
            &TurnStartParams {
                thread_id,
                input: Cow::Owned(input),
                model: config.model,
                effort: config.effort,
                service_tier: match config.service_tier {
                    ServiceTierOverride::Inherit => None,
                    ServiceTierOverride::Standard => Some(None),
                    ServiceTierOverride::Exact(tier) => Some(Some(tier)),
                },
                approval_policy: config.approval_override,
            },
        )
        .await;
    lock(&shared.protocol).note_turn_start_outcome(&outcome);
    outcome.map(|_| ())
}

pub(super) async fn send_prompt(
    shared: &Arc<SessionShared>,
    text: String,
    working_already: bool,
) -> DeliveryOutcome<()> {
    if !sendable_prompt(&text, shared.max_frame_bytes) {
        return DeliveryOutcome::DefinitelyUnsent(EngineError::definitely_unsent(
            EngineErrorKind::InvalidInput,
            "Codex prompt must be non-empty and bounded",
            false,
        ));
    }
    let _guard = shared.effect_lock.lock().await;
    if shared.core.is_closed() {
        return shared.core.closed_outcome("send");
    }
    if let Err(error) = lock(&shared.core.lifecycle).require_sendable() {
        let error: EngineError = error.into();
        return DeliveryOutcome::from_error(error);
    }
    if let Err(error) = lock(&shared.protocol).require_new_prompt_safe() {
        return DeliveryOutcome::from_error(error);
    }
    let requested_skill = skill_command_key(&text);
    if let Some(skill) = requested_skill.as_deref()
        && !ensure_skill_catalogue(shared, skill).await
    {
        command_notice(
            shared,
            working_already,
            "Codex could not load the command catalogue. The skill command was not sent.",
            NoticeTone::Warning,
        );
        return DeliveryOutcome::Confirmed(());
    }
    if dispatch_provider_command(shared, &text, working_already).await {
        return DeliveryOutcome::Confirmed(());
    }
    if let Some(skill) = requested_skill {
        command_notice(
            shared,
            working_already,
            format!("/{skill} is not available for this directory. Nothing was sent."),
            NoticeTone::Warning,
        );
        return DeliveryOutcome::Confirmed(());
    }
    let transport = match shared.core.transport() {
        Ok(transport) => transport,
        Err(error) => return DeliveryOutcome::from_error(error),
    };
    let (thread_id, active_turn_id) = {
        let protocol = lock(&shared.protocol);
        (
            protocol.thread_id().map(ToOwned::to_owned),
            protocol.active_turn_id().map(ToOwned::to_owned),
        )
    };
    let Some(thread_id) = thread_id else {
        return DeliveryOutcome::DefinitelyUnsent(EngineError::definitely_unsent(
            EngineErrorKind::Lifecycle,
            "Codex thread id is not ready",
            true,
        ));
    };
    // A created thread owes the collaboration source label to its first turn;
    // resume already delivered the same instruction with `thread.resume`. It
    // leads the input as its own marked part so the user's text stays intact.
    let owed = lock(&shared.pending_host_context).take();
    let mut input = Vec::with_capacity(2);
    if let Some(host_context) = &owed {
        input.push(TextInput::Text {
            text: host_context.clone(),
        });
    }
    input.push(TextInput::Text { text });
    let steered = match active_turn_id {
        Some(expected_turn_id) => {
            steer_turn(shared, &transport, thread_id, expected_turn_id, &input).await
        }
        None => None,
    };
    let outcome = match steered {
        Some(outcome) => outcome,
        None => start_new_turn(shared, input).await,
    };
    // A turn that definitely never reached Codex still owes the label. An
    // unknown outcome keeps it spent: that turn is never replayed.
    if matches!(
        outcome,
        DeliveryOutcome::DefinitelyRejected(_) | DeliveryOutcome::DefinitelyUnsent(_)
    ) {
        let mut slot = lock(&shared.pending_host_context);
        if slot.is_none() {
            *slot = owed;
        }
    }
    outcome
}

/// Steers the running turn. `None` means there was nothing to steer into, or
/// Codex refused the steer in a way that leaves starting a new turn safe.
async fn steer_turn(
    shared: &Arc<SessionShared>,
    transport: &Arc<interface_engine::supervised::SupervisedTransport<CodexTransportEvent>>,
    thread_id: String,
    expected_turn_id: String,
    input: &[TextInput],
) -> Option<DeliveryOutcome<()>> {
    let steer = transport
        .request(
            CodexMethod::TURN_STEER,
            &TurnSteerParams {
                thread_id,
                expected_turn_id,
                input: Cow::Borrowed(input),
            },
        )
        .await;
    match &steer {
        DeliveryOutcome::Confirmed(response) => {
            if let Some(turn_id) = turn_id_from_response(response) {
                lock(&shared.protocol).note_turn_started(turn_id);
            }
            Some(DeliveryOutcome::Confirmed(()))
        }
        DeliveryOutcome::DefinitelyRejected(error) if steer_not_steerable(error) => Some(
            DeliveryOutcome::DefinitelyUnsent(EngineError::definitely_unsent(
                EngineErrorKind::Lifecycle,
                "Codex is running a turn that cannot take mid-turn input — a /review or a /compact (the protocol's activeTurnNotSteerable). Nothing was sent. Stop the turn, or wait for it to finish, then send this again.",
                false,
            )),
        ),
        DeliveryOutcome::DefinitelyRejected(error) if steer_fallback_safe(error) => None,
        DeliveryOutcome::DefinitelyRejected(_)
        | DeliveryOutcome::DefinitelyUnsent(_)
        | DeliveryOutcome::OutcomeUnknown(_) => Some(steer.map(|_| ())),
    }
}

pub(super) fn note_turn_notification(shared: &SessionShared, method: &str, params: Option<&Value>) {
    let params = params.and_then(Value::as_object);
    if method == "turn/started" {
        if let Some(id) = params
            .and_then(|params| params.get("turn"))
            .and_then(Value::as_object)
            .and_then(|turn| turn.get("id"))
            .and_then(Value::as_str)
            .or_else(|| {
                params
                    .and_then(|params| params.get("turnId"))
                    .and_then(Value::as_str)
            })
        {
            lock(&shared.protocol).note_turn_started(id);
        }
    } else if method == "turn/completed" {
        if let Some(id) = params
            .and_then(|params| params.get("turn"))
            .and_then(Value::as_object)
            .and_then(|turn| turn.get("id"))
            .and_then(Value::as_str)
        {
            lock(&shared.protocol).note_turn_completed(id);
        }
    }
}

/// A non-empty prompt within the prompt bound that one frame can carry.
fn sendable_prompt(text: &str, max_frame_bytes: usize) -> bool {
    !text.is_empty() && text.len() <= MAX_PROMPT_BYTES && text_fits_frame(text, max_frame_bytes)
}

pub(super) fn steer_fallback_safe(error: &EngineError) -> bool {
    let message = error.message.to_ascii_lowercase();
    message.contains("expectedturnid")
        || message.contains("expected turn")
        || message.contains("no active turn")
        || message.contains("turn not found")
        || message.contains("not active")
}

pub(super) fn steer_not_steerable(error: &EngineError) -> bool {
    error
        .message
        .to_ascii_lowercase()
        .contains("activeturnnotsteerable")
}
