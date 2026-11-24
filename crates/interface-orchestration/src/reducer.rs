#![allow(clippy::missing_errors_doc)]

use interface_contracts::orchestration::{
    AgentNodeState, EffectReservationV1, MessageDeliveryState, ORCHESTRATION_LIMITS,
    ORCHESTRATOR_ROLE, OrchestrationCapabilityFacet, OrchestrationCapabilityState,
    OrchestrationEffectKind, OrchestrationEffectState, OrchestrationEffectTargetKind,
    OrchestrationEntityKind, OrchestrationEventV1, OrchestrationMutation,
    OrchestrationMutationMetaV1, OrchestrationSnapshotV1, ReconciledLeaseState, RoleClaimState,
    SettledEffectState, SettledEscrowState, WORKER_ROLE, WorktreeEscrowState, WorktreeEscrowV1,
    WorktreeLeaseState, WorktreeLeaseV1, upgrade_orchestration_snapshot,
};
use interface_storage::sha256_hex;
use std::collections::HashSet;

use crate::{OrchestrationError, OrchestrationErrorCode, Result};

fn fail<T>(code: OrchestrationErrorCode, message: impl Into<String>) -> Result<T> {
    Err(OrchestrationError::new(code, message))
}

fn at_or_after(at: &str, previous: &str, label: &str) -> Result<()> {
    if at < previous {
        fail(
            OrchestrationErrorCode::InvalidMutation,
            format!("{label} cannot move backwards"),
        )
    } else {
        Ok(())
    }
}

fn create_time(at: &str, created_at: &str, label: &str) -> Result<()> {
    if at == created_at {
        Ok(())
    } else {
        fail(
            OrchestrationErrorCode::InvalidMutation,
            format!("{label} must equal mutation time"),
        )
    }
}

fn capacity(length: usize, maximum: usize, label: &str) -> Result<()> {
    if length >= maximum {
        fail(
            OrchestrationErrorCode::LimitExceeded,
            format!("{label} limit of {maximum} is reached"),
        )
    } else {
        Ok(())
    }
}

fn index_of<T>(items: &[T], predicate: impl Fn(&T) -> bool, label: &str) -> Result<usize> {
    items.iter().position(predicate).ok_or_else(|| {
        OrchestrationError::new(
            OrchestrationErrorCode::NotFound,
            format!("{label} does not exist"),
        )
    })
}

fn session_unbound(
    snapshot: &OrchestrationSnapshotV1,
    session_key: &str,
    agent_id: Option<&str>,
) -> Result<()> {
    if let Some(owner) = snapshot.agents.iter().find(|agent| {
        agent.session_key.as_deref() == Some(session_key)
            && Some(agent.agent_id.as_str()) != agent_id
    }) {
        fail(
            OrchestrationErrorCode::AlreadyExists,
            format!("session {session_key} is already agent {}", owner.agent_id),
        )
    } else {
        Ok(())
    }
}

/// Absorbing agent states: `agent_transition` has no outgoing arcs from these.
#[must_use]
pub const fn agent_state_is_terminal(state: AgentNodeState) -> bool {
    matches!(
        state,
        AgentNodeState::Completed
            | AgentNodeState::Failed
            | AgentNodeState::Cancelled
            | AgentNodeState::Unknown
    )
}

fn agent_transition(from: AgentNodeState, to: AgentNodeState) -> bool {
    use AgentNodeState::{
        Cancelled, Completed, Failed, Idle, Planned, Ready, Starting, Unknown, Waiting, Working,
    };
    matches!(
        (from, to),
        (Planned, Starting | Cancelled)
            | (Starting, Ready | Failed | Cancelled | Unknown)
            | (
                Ready,
                Working | Waiting | Idle | Completed | Failed | Cancelled | Unknown
            )
            | (
                Working,
                Waiting | Idle | Completed | Failed | Cancelled | Unknown
            )
            | (
                Waiting,
                Working | Idle | Completed | Failed | Cancelled | Unknown
            )
            | (Idle, Working | Completed | Failed | Cancelled | Unknown)
    )
}

fn message_transition(from: MessageDeliveryState, to: MessageDeliveryState) -> bool {
    use MessageDeliveryState::{
        Failed, Processed, ProviderAccepted, Queued, Requested, Settled, Unknown,
    };
    matches!(
        (from, to),
        (Requested, Queued | Failed | Unknown)
            | (Queued, ProviderAccepted | Failed | Unknown)
            | (ProviderAccepted, Settled | Failed | Unknown)
            | (Settled, Processed | Failed | Unknown)
    )
}

fn invalid_transition(
    label: &str,
    from: impl std::fmt::Debug,
    to: impl std::fmt::Debug,
) -> OrchestrationError {
    OrchestrationError::new(
        OrchestrationErrorCode::InvalidTransition,
        format!("{label} cannot move from {from:?} to {to:?}"),
    )
}

pub(crate) fn sha256_text(value: &str) -> String {
    sha256_hex(value.as_bytes())
}

/// Admission checks shared by the escrow-intent and direct escrow mutations.
fn guard_escrow_admission(
    snapshot: &OrchestrationSnapshotV1,
    lease_index: usize,
    escrow: &WorktreeEscrowV1,
) -> Result<()> {
    if snapshot.worktree_leases[lease_index].coordinator.is_some() {
        return fail(
            OrchestrationErrorCode::InvalidTransition,
            "a coordinated lease cannot be escrowed",
        );
    }
    if snapshot.worktree_leases[lease_index].state != WorktreeLeaseState::Active
        || snapshot.worktree_leases[lease_index]
            .escrow
            .as_ref()
            .is_some_and(|item| item.state == WorktreeEscrowState::Prepared)
    {
        return fail(
            OrchestrationErrorCode::InvalidTransition,
            "only an active lease without prepared escrow can enter escrow",
        );
    }
    let lease = &snapshot.worktree_leases[lease_index];
    if escrow.state != WorktreeEscrowState::Prepared
        || escrow.source_agent_id != lease.owner_agent_id
        || escrow.source_owner_revision != lease.owner_revision
    {
        return fail(
            OrchestrationErrorCode::InvalidMutation,
            "escrow must bind the current owner and owner revision",
        );
    }
    let target_index = index_of(
        &snapshot.agents,
        |agent| agent.agent_id == escrow.target_agent_id,
        &format!("worktree target agent {}", escrow.target_agent_id),
    )?;
    let target_root = snapshot.agents[target_index]
        .scope
        .canonical_project_root
        .as_deref()
        .unwrap_or(&snapshot.agents[target_index].scope.canonical_cwd);
    if target_root != lease.repository_root
        || snapshot.agents[target_index]
            .scope
            .worktree_lease_id
            .is_some()
    {
        return fail(
            OrchestrationErrorCode::InvalidMutation,
            "worktree target must be unleased in the same project root",
        );
    }
    Ok(())
}

/// An intent's new effect may reuse neither an effect id nor an idempotency
/// key. The id is checked first; `idempotency_message` names the intent.
fn admit_new_effect(
    snapshot: &OrchestrationSnapshotV1,
    effect: &EffectReservationV1,
    idempotency_message: &str,
) -> Result<()> {
    if snapshot
        .effects
        .iter()
        .any(|item| item.effect_id == effect.effect_id)
    {
        return fail(
            OrchestrationErrorCode::AlreadyExists,
            format!("effect {} already exists", effect.effect_id),
        );
    }
    if snapshot
        .effects
        .iter()
        .any(|item| item.idempotency_key == effect.idempotency_key)
    {
        return fail(
            OrchestrationErrorCode::IdempotencyConflict,
            idempotency_message,
        );
    }
    Ok(())
}

/// An intent's effect must be an unattempted reservation of `kind` aimed at
/// exactly this target in the active runtime epoch.
fn fresh_intent_effect(
    snapshot: &OrchestrationSnapshotV1,
    effect: &EffectReservationV1,
    kind: OrchestrationEffectKind,
    target_kind: OrchestrationEffectTargetKind,
    target_id: &str,
    message: &str,
) -> Result<()> {
    if effect.kind != kind
        || effect.target_kind != target_kind
        || effect.target_id != target_id
        || effect.state != OrchestrationEffectState::Reserved
        || effect.attempts != 0
        || effect.runtime_epoch != snapshot.runtime_epoch
    {
        return fail(OrchestrationErrorCode::InvalidMutation, message);
    }
    Ok(())
}

/// Every ownership change or release advances the lease's owner revision.
fn bump_owner_revision(lease: &mut WorktreeLeaseV1) -> Result<()> {
    lease.owner_revision = lease.owner_revision.checked_add(1).ok_or_else(|| {
        OrchestrationError::new(
            OrchestrationErrorCode::LimitExceeded,
            "lease owner revision overflow",
        )
    })?;
    Ok(())
}

fn append_event(
    snapshot: &mut OrchestrationSnapshotV1,
    meta: &OrchestrationMutationMetaV1,
    kind: String,
    entity_kind: OrchestrationEntityKind,
    entity_id: String,
    event_ids: Option<&HashSet<String>>,
) -> Result<()> {
    at_or_after(&meta.at, &snapshot.updated_at, "event timestamp")?;
    let retained = || {
        snapshot
            .events
            .iter()
            .any(|event| event.event_id == meta.event_id)
    };
    let duplicate = event_ids.map_or_else(retained, |ids| {
        let indexed = ids.contains(&meta.event_id);
        debug_assert_eq!(indexed, retained(), "the event index left its snapshot");
        indexed
    });
    if duplicate {
        return fail(
            OrchestrationErrorCode::AlreadyExists,
            format!("event {} already exists", meta.event_id),
        );
    }
    if snapshot.event_floor_sequence.is_none() {
        snapshot.event_floor_sequence = Some(crate::orchestration_event_floor(snapshot));
    }
    snapshot.revision = snapshot.revision.checked_add(1).ok_or_else(|| {
        OrchestrationError::new(
            OrchestrationErrorCode::LimitExceeded,
            "snapshot revision overflow",
        )
    })?;
    snapshot.updated_at.clone_from(&meta.at);
    snapshot.events.push(OrchestrationEventV1 {
        version: 1,
        event_id: meta.event_id.clone(),
        sequence: snapshot.revision,
        kind,
        entity_kind,
        entity_id,
        actor: meta.actor.clone(),
        at: meta.at.clone(),
        summary: meta.summary.clone(),
    });
    let overflow = snapshot
        .events
        .len()
        .saturating_sub(ORCHESTRATION_LIMITS.events);
    if overflow > 0 {
        snapshot.event_floor_sequence = snapshot
            .events
            .drain(..overflow)
            .next_back()
            .map(|event| event.sequence);
    }
    Ok(())
}

fn live_lease(state: WorktreeLeaseState) -> bool {
    matches!(
        state,
        WorktreeLeaseState::Active | WorktreeLeaseState::Escrow | WorktreeLeaseState::Unknown
    )
}

fn clear_lease_references(snapshot: &mut OrchestrationSnapshotV1, lease_id: &str, at: &str) {
    for agent in &mut snapshot.agents {
        if agent.scope.worktree_lease_id.as_deref() == Some(lease_id) {
            agent.scope.worktree_lease_id = None;
            at.clone_into(&mut agent.updated_at);
        }
    }
}

/// Apply exactly one pure domain mutation without clocks, randomness, I/O, or provider effects.
pub fn reduce_orchestration(
    input: &OrchestrationSnapshotV1,
    mutation: OrchestrationMutation,
) -> Result<OrchestrationSnapshotV1> {
    reduce(input, mutation, None)
}

/// [`reduce_orchestration`] with the ids of `input`'s retained events
/// already indexed, so the duplicate-event check is a lookup instead of a
/// scan of the whole window. `event_ids` must be exactly those ids.
pub(crate) fn reduce_orchestration_indexed(
    input: &OrchestrationSnapshotV1,
    mutation: OrchestrationMutation,
    event_ids: &HashSet<String>,
) -> Result<OrchestrationSnapshotV1> {
    reduce(input, mutation, Some(event_ids))
}

#[allow(clippy::too_many_lines)]
fn reduce(
    input: &OrchestrationSnapshotV1,
    mutation: OrchestrationMutation,
    event_ids: Option<&HashSet<String>>,
) -> Result<OrchestrationSnapshotV1> {
    let mut snapshot = upgrade_orchestration_snapshot(input.clone());

    let (meta, kind, entity_kind, entity_id) = match mutation {
        OrchestrationMutation::RotateRuntimeEpoch {
            runtime_epoch,
            meta,
        } => {
            if runtime_epoch == snapshot.runtime_epoch {
                return fail(
                    OrchestrationErrorCode::InvalidMutation,
                    "runtime epoch must change",
                );
            }
            snapshot.runtime_epoch = runtime_epoch;
            let host_id = snapshot.host_id.clone();
            (
                meta,
                "host.runtime-epoch-rotated".to_owned(),
                OrchestrationEntityKind::Host,
                host_id,
            )
        }
        OrchestrationMutation::CreateAgentIntent {
            agent,
            effect,
            meta,
        } => {
            capacity(snapshot.agents.len(), ORCHESTRATION_LIMITS.agents, "agent")?;
            capacity(
                snapshot.effects.len(),
                ORCHESTRATION_LIMITS.effects,
                "effect",
            )?;
            if snapshot
                .agents
                .iter()
                .any(|item| item.agent_id == agent.agent_id)
            {
                return fail(
                    OrchestrationErrorCode::AlreadyExists,
                    format!("agent {} already exists", agent.agent_id),
                );
            }
            admit_new_effect(
                &snapshot,
                &effect,
                "agent intent idempotency key already exists",
            )?;
            if agent.runtime_epoch != snapshot.runtime_epoch
                || agent.state != AgentNodeState::Planned
            {
                return fail(
                    OrchestrationErrorCode::InvalidMutation,
                    "new agent intents require a planned agent in the active epoch",
                );
            }
            fresh_intent_effect(
                &snapshot,
                &effect,
                OrchestrationEffectKind::SpawnAgent,
                OrchestrationEffectTargetKind::Agent,
                &agent.agent_id,
                "agent intent requires a matching unattempted spawn effect",
            )?;
            create_time(&meta.at, &agent.created_at, "agent.createdAt")?;
            create_time(&meta.at, &agent.updated_at, "agent.updatedAt")?;
            create_time(&meta.at, &effect.reserved_at, "effect.reservedAt")?;
            let id = agent.agent_id.clone();
            snapshot.agents.push(agent);
            snapshot.effects.push(effect);
            (
                meta,
                "agent.intent-created".to_owned(),
                OrchestrationEntityKind::Agent,
                id,
            )
        }
        OrchestrationMutation::RegisterAgent { agent, meta } => {
            capacity(snapshot.agents.len(), ORCHESTRATION_LIMITS.agents, "agent")?;
            if snapshot
                .agents
                .iter()
                .any(|item| item.agent_id == agent.agent_id)
            {
                return fail(
                    OrchestrationErrorCode::AlreadyExists,
                    format!("agent {} already exists", agent.agent_id),
                );
            }
            if let Some(key) = agent.session_key.as_deref() {
                session_unbound(&snapshot, key, None)?;
            }
            if agent.runtime_epoch != snapshot.runtime_epoch {
                return fail(
                    OrchestrationErrorCode::InvalidMutation,
                    "new agent must use the active runtime epoch",
                );
            }
            create_time(&meta.at, &agent.created_at, "agent.createdAt")?;
            create_time(&meta.at, &agent.updated_at, "agent.updatedAt")?;
            let id = agent.agent_id.clone();
            snapshot.agents.push(agent);
            (
                meta,
                "agent.registered".to_owned(),
                OrchestrationEntityKind::Agent,
                id,
            )
        }
        OrchestrationMutation::SetAgentState {
            agent_id,
            state,
            session_key,
            provider_session_id,
            meta,
        } => {
            let index = index_of(
                &snapshot.agents,
                |item| item.agent_id == agent_id,
                &format!("agent {agent_id}"),
            )?;
            let current = snapshot.agents[index].state;
            if !agent_transition(current, state) {
                return Err(invalid_transition(
                    &format!("agent {agent_id}"),
                    current,
                    state,
                ));
            }
            at_or_after(
                &meta.at,
                &snapshot.agents[index].updated_at,
                "agent.updatedAt",
            )?;
            if let Some(key) = session_key.as_deref() {
                session_unbound(&snapshot, key, Some(&agent_id))?;
            }
            let agent = &mut snapshot.agents[index];
            agent.state = state;
            agent.updated_at.clone_from(&meta.at);
            if session_key.is_some() {
                agent.session_key = session_key;
            }
            if provider_session_id.is_some() {
                agent.provider_session_id = provider_session_id;
            }
            (
                meta,
                "agent.state-changed".to_owned(),
                OrchestrationEntityKind::Agent,
                agent_id,
            )
        }
        OrchestrationMutation::SetOrchestratorPolicy { policy, meta } => {
            let agent_index = index_of(
                &snapshot.agents,
                |item| item.agent_id == policy.agent_id,
                &format!("agent {}", policy.agent_id),
            )?;
            if snapshot.agents[agent_index].parent_agent_id.is_some() {
                return fail(
                    OrchestrationErrorCode::InvalidTransition,
                    "a worker with a parent cannot become an orchestrator",
                );
            }
            let policy_index = snapshot
                .orchestrator_policies
                .iter()
                .position(|item| item.agent_id == policy.agent_id);
            if policy_index.is_none() {
                capacity(
                    snapshot.orchestrator_policies.len(),
                    ORCHESTRATION_LIMITS.orchestrator_policies,
                    "orchestrator policy",
                )?;
            }
            at_or_after(&meta.at, &policy.activated_at, "policy.activatedAt")?;
            create_time(&meta.at, &policy.updated_at, "policy.updatedAt")?;
            at_or_after(
                &meta.at,
                &snapshot.agents[agent_index].updated_at,
                "agent.updatedAt",
            )?;
            let id = policy.agent_id.clone();
            if let Some(index) = policy_index {
                snapshot.orchestrator_policies[index] = policy;
            } else {
                snapshot.orchestrator_policies.push(policy);
            }
            snapshot.agents[agent_index].role = Some(ORCHESTRATOR_ROLE.to_owned());
            snapshot.agents[agent_index].updated_at.clone_from(&meta.at);
            (
                meta,
                "orchestrator.policy-set".to_owned(),
                OrchestrationEntityKind::Agent,
                id,
            )
        }
        OrchestrationMutation::ClearOrchestratorPolicy { agent_id, meta } => {
            let agent_index = index_of(
                &snapshot.agents,
                |item| item.agent_id == agent_id,
                &format!("agent {agent_id}"),
            )?;
            let policy_index = index_of(
                &snapshot.orchestrator_policies,
                |item| item.agent_id == agent_id,
                &format!("orchestrator policy {agent_id}"),
            )?;
            if snapshot
                .agents
                .iter()
                .any(|item| item.parent_agent_id.as_deref() == Some(&agent_id))
            {
                return fail(
                    OrchestrationErrorCode::InvalidTransition,
                    "an orchestrator with workers cannot be demoted",
                );
            }
            at_or_after(
                &meta.at,
                &snapshot.agents[agent_index].updated_at,
                "agent.updatedAt",
            )?;
            snapshot.orchestrator_policies.remove(policy_index);
            snapshot.agents[agent_index].role = None;
            snapshot.agents[agent_index].updated_at.clone_from(&meta.at);
            (
                meta,
                "orchestrator.policy-cleared".to_owned(),
                OrchestrationEntityKind::Agent,
                agent_id,
            )
        }
        OrchestrationMutation::BindAgentParent {
            agent_id,
            parent_agent_id,
            meta,
        } => {
            let agent_index = index_of(
                &snapshot.agents,
                |item| item.agent_id == agent_id,
                &format!("agent {agent_id}"),
            )?;
            at_or_after(
                &meta.at,
                &snapshot.agents[agent_index].updated_at,
                "agent.updatedAt",
            )?;
            let event_kind;
            if let Some(parent_id) = parent_agent_id {
                if parent_id == agent_id {
                    return fail(
                        OrchestrationErrorCode::InvalidMutation,
                        "an agent cannot be its own parent",
                    );
                }
                let parent_index = index_of(
                    &snapshot.agents,
                    |item| item.agent_id == parent_id,
                    &format!("agent {parent_id}"),
                )?;
                if !snapshot
                    .orchestrator_policies
                    .iter()
                    .any(|item| item.agent_id == parent_id)
                {
                    return fail(
                        OrchestrationErrorCode::InvalidTransition,
                        "only an orchestrator can take workers",
                    );
                }
                if snapshot
                    .orchestrator_policies
                    .iter()
                    .any(|item| item.agent_id == agent_id)
                {
                    return fail(
                        OrchestrationErrorCode::InvalidTransition,
                        "star topology: an orchestrator cannot be a worker",
                    );
                }
                if snapshot.agents[parent_index].depth != 0 {
                    return fail(
                        OrchestrationErrorCode::InvalidTransition,
                        "delegation is one level deep; a worker cannot take workers",
                    );
                }
                snapshot.agents[agent_index].parent_agent_id = Some(parent_id);
                snapshot.agents[agent_index].depth = 1;
                snapshot.agents[agent_index].role = Some(WORKER_ROLE.to_owned());
                event_kind = "agent.parent-bound";
            } else {
                if snapshot.agents[agent_index].parent_agent_id.is_none() {
                    return fail(
                        OrchestrationErrorCode::InvalidTransition,
                        format!("agent {agent_id} has no parent to unlink"),
                    );
                }
                snapshot.agents[agent_index].parent_agent_id = None;
                snapshot.agents[agent_index].role = None;
                snapshot.agents[agent_index].depth = 0;
                event_kind = "agent.parent-cleared";
            }
            snapshot.agents[agent_index].updated_at.clone_from(&meta.at);
            (
                meta,
                event_kind.to_owned(),
                OrchestrationEntityKind::Agent,
                agent_id,
            )
        }
        OrchestrationMutation::RegisterAgentFromSession { agent, meta } => {
            capacity(snapshot.agents.len(), ORCHESTRATION_LIMITS.agents, "agent")?;
            let Some(key) = agent.session_key.as_deref() else {
                return fail(
                    OrchestrationErrorCode::InvalidMutation,
                    "session registration requires a sessionKey",
                );
            };
            if agent.parent_agent_id.is_some() || agent.depth != 0 {
                return fail(
                    OrchestrationErrorCode::InvalidMutation,
                    "a registered session joins as a root agent",
                );
            }
            if snapshot
                .agents
                .iter()
                .any(|item| item.agent_id == agent.agent_id)
            {
                return fail(
                    OrchestrationErrorCode::AlreadyExists,
                    format!("agent {} already exists", agent.agent_id),
                );
            }
            session_unbound(&snapshot, key, None)?;
            if agent.runtime_epoch != snapshot.runtime_epoch {
                return fail(
                    OrchestrationErrorCode::InvalidMutation,
                    "new agent must use the active runtime epoch",
                );
            }
            create_time(&meta.at, &agent.created_at, "agent.createdAt")?;
            create_time(&meta.at, &agent.updated_at, "agent.updatedAt")?;
            let id = agent.agent_id.clone();
            snapshot.agents.push(agent);
            (
                meta,
                "agent.session-registered".to_owned(),
                OrchestrationEntityKind::Agent,
                id,
            )
        }
        OrchestrationMutation::CountOrchestratorUsage {
            agent_id,
            delegations,
            spawns,
            meta,
        } => {
            let index = index_of(
                &snapshot.orchestrator_policies,
                |item| item.agent_id == agent_id,
                &format!("orchestrator policy {agent_id}"),
            )?;
            let delegations = delegations.unwrap_or(0);
            let spawns = spawns.unwrap_or(0);
            if delegations == 0 && spawns == 0 {
                return fail(
                    OrchestrationErrorCode::InvalidMutation,
                    "usage accounting must count a delegation or a spawn",
                );
            }
            if snapshot.orchestrator_policies[index]
                .delegations_used
                .saturating_add(delegations)
                > snapshot.orchestrator_policies[index].max_delegations
            {
                return fail(
                    OrchestrationErrorCode::LimitExceeded,
                    format!(
                        "orchestrator delegation limit of {} is reached",
                        snapshot.orchestrator_policies[index].max_delegations
                    ),
                );
            }
            at_or_after(
                &meta.at,
                &snapshot.orchestrator_policies[index].updated_at,
                "policy.updatedAt",
            )?;
            let policy = &mut snapshot.orchestrator_policies[index];
            policy.delegations_used += delegations;
            policy.spawns_used = policy.spawns_used.checked_add(spawns).ok_or_else(|| {
                OrchestrationError::new(
                    OrchestrationErrorCode::LimitExceeded,
                    "orchestrator spawn counter overflow",
                )
            })?;
            policy.updated_at.clone_from(&meta.at);
            (
                meta,
                "orchestrator.usage-counted".to_owned(),
                OrchestrationEntityKind::Agent,
                agent_id,
            )
        }
        OrchestrationMutation::EnqueueMessageIntent {
            message,
            effect,
            meta,
        } => {
            capacity(
                snapshot.messages.len(),
                ORCHESTRATION_LIMITS.messages,
                "message",
            )?;
            capacity(
                snapshot.effects.len(),
                ORCHESTRATION_LIMITS.effects,
                "effect",
            )?;
            if snapshot
                .messages
                .iter()
                .any(|item| item.message_id == message.message_id)
            {
                return fail(
                    OrchestrationErrorCode::AlreadyExists,
                    format!("message {} already exists", message.message_id),
                );
            }
            admit_new_effect(
                &snapshot,
                &effect,
                "message intent idempotency key already exists",
            )?;
            if message.receipts.len() != 1
                || message.receipts[0].state != MessageDeliveryState::Requested
                || message
                    .body
                    .as_deref()
                    .is_none_or(|body| sha256_text(body) != message.body_digest)
            {
                return fail(
                    OrchestrationErrorCode::InvalidMutation,
                    "message intent requires one requested receipt and matching body",
                );
            }
            fresh_intent_effect(
                &snapshot,
                &effect,
                OrchestrationEffectKind::SendMessage,
                OrchestrationEffectTargetKind::Message,
                &message.message_id,
                "message intent requires a matching unattempted send effect",
            )?;
            create_time(&meta.at, &message.created_at, "message.createdAt")?;
            create_time(&meta.at, &message.updated_at, "message.updatedAt")?;
            create_time(&meta.at, &effect.reserved_at, "effect.reservedAt")?;
            let id = message.message_id.clone();
            snapshot.messages.push(message);
            snapshot.effects.push(effect);
            (
                meta,
                "message.intent-created".to_owned(),
                OrchestrationEntityKind::Message,
                id,
            )
        }
        OrchestrationMutation::EnqueueMessage { message, meta } => {
            capacity(
                snapshot.messages.len(),
                ORCHESTRATION_LIMITS.messages,
                "message",
            )?;
            if snapshot
                .messages
                .iter()
                .any(|item| item.message_id == message.message_id)
            {
                return fail(
                    OrchestrationErrorCode::AlreadyExists,
                    format!("message {} already exists", message.message_id),
                );
            }
            if message.receipts.len() != 1
                || message.receipts[0].state != MessageDeliveryState::Requested
            {
                return fail(
                    OrchestrationErrorCode::InvalidMutation,
                    "new messages require exactly one requested receipt",
                );
            }
            if message
                .body
                .as_deref()
                .is_none_or(|body| sha256_text(body) != message.body_digest)
            {
                return fail(
                    OrchestrationErrorCode::InvalidMutation,
                    "new message body must be present and match bodyDigest",
                );
            }
            create_time(&meta.at, &message.created_at, "message.createdAt")?;
            create_time(&meta.at, &message.updated_at, "message.updatedAt")?;
            let id = message.message_id.clone();
            snapshot.messages.push(message);
            (
                meta,
                "message.requested".to_owned(),
                OrchestrationEntityKind::Message,
                id,
            )
        }
        OrchestrationMutation::AppendMessageReceipt {
            message_id,
            receipt,
            meta,
        } => {
            let index = index_of(
                &snapshot.messages,
                |item| item.message_id == message_id,
                &format!("message {message_id}"),
            )?;
            let current = snapshot.messages[index]
                .receipts
                .last()
                .map(|item| item.state)
                .ok_or_else(|| {
                    OrchestrationError::new(
                        OrchestrationErrorCode::InvalidState,
                        "message has no receipt",
                    )
                })?;
            if !message_transition(current, receipt.state) {
                return Err(invalid_transition(
                    &format!("message {message_id}"),
                    current,
                    receipt.state,
                ));
            }
            if snapshot.messages[index]
                .receipts
                .iter()
                .any(|item| item.receipt_id == receipt.receipt_id)
            {
                return fail(
                    OrchestrationErrorCode::AlreadyExists,
                    format!("receipt {} already exists", receipt.receipt_id),
                );
            }
            capacity(
                snapshot.messages[index].receipts.len(),
                ORCHESTRATION_LIMITS.receipts_per_message,
                "message receipt",
            )?;
            at_or_after(
                &receipt.at,
                &snapshot.messages[index].updated_at,
                "message receipt timestamp",
            )?;
            if receipt.at != meta.at {
                return fail(
                    OrchestrationErrorCode::InvalidMutation,
                    "receipt time must equal event time",
                );
            }
            let state_name = receipt.state.event_suffix();
            snapshot.messages[index].updated_at.clone_from(&receipt.at);
            snapshot.messages[index].receipts.push(receipt);
            (
                meta,
                format!("message.{state_name}"),
                OrchestrationEntityKind::Message,
                message_id,
            )
        }
        OrchestrationMutation::ClaimRole { claim, meta } => {
            capacity(
                snapshot.role_claims.len(),
                ORCHESTRATION_LIMITS.role_claims,
                "role claim",
            )?;
            if snapshot
                .role_claims
                .iter()
                .any(|item| item.claim_id == claim.claim_id)
            {
                return fail(
                    OrchestrationErrorCode::AlreadyExists,
                    format!("role claim {} already exists", claim.claim_id),
                );
            }
            if claim.state != RoleClaimState::Active {
                return fail(
                    OrchestrationErrorCode::InvalidMutation,
                    "new role claims must be active",
                );
            }
            create_time(&meta.at, &claim.claimed_at, "claim.claimedAt")?;
            create_time(&meta.at, &claim.updated_at, "claim.updatedAt")?;
            let id = claim.claim_id.clone();
            snapshot.role_claims.push(claim);
            (
                meta,
                "role.claimed".to_owned(),
                OrchestrationEntityKind::RoleClaim,
                id,
            )
        }
        OrchestrationMutation::RelinquishRole { claim_id, meta } => {
            let index = index_of(
                &snapshot.role_claims,
                |item| item.claim_id == claim_id,
                &format!("role claim {claim_id}"),
            )?;
            if snapshot.role_claims[index].state != RoleClaimState::Active {
                return fail(
                    OrchestrationErrorCode::InvalidTransition,
                    "role claim is not active",
                );
            }
            at_or_after(
                &meta.at,
                &snapshot.role_claims[index].updated_at,
                "claim.updatedAt",
            )?;
            snapshot.role_claims[index].state = RoleClaimState::Relinquished;
            snapshot.role_claims[index].updated_at.clone_from(&meta.at);
            snapshot.role_claims[index].relinquished_at = Some(meta.at.clone());
            (
                meta,
                "role.relinquished".to_owned(),
                OrchestrationEntityKind::RoleClaim,
                claim_id,
            )
        }
        OrchestrationMutation::ReserveEffect { effect, meta } => {
            capacity(
                snapshot.effects.len(),
                ORCHESTRATION_LIMITS.effects,
                "effect",
            )?;
            if let Some(replay) = snapshot
                .effects
                .iter()
                .find(|item| item.idempotency_key == effect.idempotency_key)
            {
                if replay.effect_id == effect.effect_id
                    && replay.request_hash == effect.request_hash
                {
                    return fail(
                        OrchestrationErrorCode::AlreadyExists,
                        format!("effect {} is an idempotent replay", replay.effect_id),
                    );
                }
                return fail(
                    OrchestrationErrorCode::IdempotencyConflict,
                    format!("idempotency key belongs to effect {}", replay.effect_id),
                );
            }
            if snapshot
                .effects
                .iter()
                .any(|item| item.effect_id == effect.effect_id)
            {
                return fail(
                    OrchestrationErrorCode::AlreadyExists,
                    format!("effect {} already exists", effect.effect_id),
                );
            }
            if effect.state != OrchestrationEffectState::Reserved
                || effect.attempts != 0
                || effect.runtime_epoch != snapshot.runtime_epoch
            {
                return fail(
                    OrchestrationErrorCode::InvalidMutation,
                    "new effects must be unattempted reservations in the active epoch",
                );
            }
            create_time(&meta.at, &effect.reserved_at, "effect.reservedAt")?;
            let id = effect.effect_id.clone();
            snapshot.effects.push(effect);
            (
                meta,
                "effect.reserved".to_owned(),
                OrchestrationEntityKind::Effect,
                id,
            )
        }
        OrchestrationMutation::StartEffect { effect_id, meta } => {
            let index = index_of(
                &snapshot.effects,
                |item| item.effect_id == effect_id,
                &format!("effect {effect_id}"),
            )?;
            if snapshot.effects[index].state != OrchestrationEffectState::Reserved
                || snapshot.effects[index].attempts != 0
            {
                return fail(
                    OrchestrationErrorCode::InvalidTransition,
                    "only an unattempted reservation can start",
                );
            }
            if snapshot.effects[index].runtime_epoch != snapshot.runtime_epoch {
                return fail(
                    OrchestrationErrorCode::InvalidTransition,
                    "an effect reserved by an older runtime cannot start",
                );
            }
            at_or_after(
                &meta.at,
                &snapshot.effects[index].reserved_at,
                "effect.startedAt",
            )?;
            snapshot.effects[index].state = OrchestrationEffectState::Started;
            snapshot.effects[index].attempts = 1;
            snapshot.effects[index].started_at = Some(meta.at.clone());
            (
                meta,
                "effect.started".to_owned(),
                OrchestrationEntityKind::Effect,
                effect_id,
            )
        }
        OrchestrationMutation::SettleEffect {
            effect_id,
            state,
            provider_effect_id,
            error_code,
            reconciliation_note,
            meta,
        } => {
            let index = index_of(
                &snapshot.effects,
                |item| item.effect_id == effect_id,
                &format!("effect {effect_id}"),
            )?;
            let effect = &snapshot.effects[index];
            if effect.state == OrchestrationEffectState::Reserved {
                if state != SettledEffectState::Failed || effect.attempts != 0 {
                    return fail(
                        OrchestrationErrorCode::InvalidTransition,
                        "unattempted effects may only fail proven-unsent",
                    );
                }
            } else if effect.state != OrchestrationEffectState::Started || effect.attempts != 1 {
                return fail(
                    OrchestrationErrorCode::InvalidTransition,
                    "only reserved or started effects can settle",
                );
            }
            let previous = effect
                .started_at
                .as_deref()
                .unwrap_or(&effect.reserved_at)
                .to_owned();
            at_or_after(&meta.at, &previous, "effect.settledAt")?;
            let (settled, suffix) = match state {
                SettledEffectState::Succeeded => (OrchestrationEffectState::Succeeded, "succeeded"),
                SettledEffectState::Failed => (OrchestrationEffectState::Failed, "failed"),
                SettledEffectState::Unknown => (OrchestrationEffectState::Unknown, "unknown"),
            };
            let effect = &mut snapshot.effects[index];
            effect.state = settled;
            effect.settled_at = Some(meta.at.clone());
            if provider_effect_id.is_some() {
                effect.provider_effect_id = provider_effect_id;
            }
            if error_code.is_some() {
                effect.error_code = error_code;
            }
            if reconciliation_note.is_some() {
                effect.reconciliation_note = reconciliation_note;
            }
            (
                meta,
                format!("effect.{suffix}"),
                OrchestrationEntityKind::Effect,
                effect_id,
            )
        }
        OrchestrationMutation::SettleAgentLaunch {
            agent_id,
            effect_id,
            session_key,
            provider_session_id,
            provider_effect_id,
            lease,
            meta,
        } => {
            let agent_index = index_of(
                &snapshot.agents,
                |item| item.agent_id == agent_id,
                &format!("agent {agent_id}"),
            )?;
            let effect_index = index_of(
                &snapshot.effects,
                |item| item.effect_id == effect_id,
                &format!("effect {effect_id}"),
            )?;
            let effect = &snapshot.effects[effect_index];
            if effect.kind != OrchestrationEffectKind::SpawnAgent
                || effect.target_kind != OrchestrationEffectTargetKind::Agent
                || effect.target_id != agent_id
                || effect.state != OrchestrationEffectState::Started
                || effect.attempts != 1
                || effect.runtime_epoch != snapshot.runtime_epoch
                || snapshot.agents[agent_index].runtime_epoch != snapshot.runtime_epoch
            {
                return fail(
                    OrchestrationErrorCode::InvalidTransition,
                    "agent launch settlement requires its exact once-started spawn effect",
                );
            }
            let current = snapshot.agents[agent_index].state;
            if !agent_transition(current, AgentNodeState::Ready) {
                return Err(invalid_transition(
                    &format!("agent {agent_id}"),
                    current,
                    AgentNodeState::Ready,
                ));
            }
            let started_at = effect.started_at.clone().ok_or_else(|| {
                OrchestrationError::new(
                    OrchestrationErrorCode::InvalidState,
                    "started effect lacks startedAt",
                )
            })?;
            at_or_after(&meta.at, &started_at, "effect.settledAt")?;
            at_or_after(
                &meta.at,
                &snapshot.agents[agent_index].updated_at,
                "agent.updatedAt",
            )?;
            session_unbound(&snapshot, &session_key, Some(&agent_id))?;
            let access = snapshot.agents[agent_index].scope.access_mode.as_str();
            if matches!(access, "isolated" | "coordinated") && lease.is_none() {
                return fail(
                    OrchestrationErrorCode::InvalidMutation,
                    "isolated or coordinated agent launch settlement requires a managed lease",
                );
            }
            if let Some(lease) = lease {
                capacity(
                    snapshot.worktree_leases.len(),
                    ORCHESTRATION_LIMITS.worktree_leases,
                    "worktree lease",
                )?;
                if snapshot
                    .worktree_leases
                    .iter()
                    .any(|item| item.lease_id == lease.lease_id)
                {
                    return fail(
                        OrchestrationErrorCode::AlreadyExists,
                        format!("worktree lease {} already exists", lease.lease_id),
                    );
                }
                if snapshot.worktree_leases.iter().any(|item| {
                    item.canonical_path == lease.canonical_path && live_lease(item.state)
                }) {
                    return fail(
                        OrchestrationErrorCode::AlreadyExists,
                        "worktree path already has a live or ambiguous lease",
                    );
                }
                if lease.state != WorktreeLeaseState::Active
                    || lease.owner_revision != 0
                    || lease.owner_agent_id != agent_id
                {
                    return fail(
                        OrchestrationErrorCode::InvalidMutation,
                        "launch lease must be active at revision zero and owned by the launched agent",
                    );
                }
                let owner_root = snapshot.agents[agent_index]
                    .scope
                    .canonical_project_root
                    .as_deref()
                    .unwrap_or(&snapshot.agents[agent_index].scope.canonical_cwd);
                if owner_root != lease.repository_root
                    || snapshot.agents[agent_index]
                        .scope
                        .worktree_lease_id
                        .is_some()
                {
                    return fail(
                        OrchestrationErrorCode::InvalidMutation,
                        "launch lease must remain inside the approved unleased agent project root",
                    );
                }
                if !snapshot.agents[agent_index]
                    .capabilities
                    .iter()
                    .any(|capability| {
                        capability.facet == OrchestrationCapabilityFacet::Worktree
                            && capability.state == OrchestrationCapabilityState::Available
                    })
                {
                    return fail(
                        OrchestrationErrorCode::InvalidMutation,
                        "launch lease owner lacks managed worktree capability",
                    );
                }
                create_time(&meta.at, &lease.acquired_at, "lease.acquiredAt")?;
                create_time(&meta.at, &lease.updated_at, "lease.updatedAt")?;
                let lease_id = lease.lease_id.clone();
                let cwd = lease
                    .canonical_cwd
                    .clone()
                    .unwrap_or_else(|| lease.canonical_path.clone());
                let root = lease.repository_root.clone();
                snapshot.worktree_leases.push(lease);
                let agent = &mut snapshot.agents[agent_index];
                agent.scope.canonical_cwd = cwd;
                agent.scope.canonical_project_root = Some(root);
                agent.scope.worktree_lease_id = Some(lease_id);
            }
            let effect = &mut snapshot.effects[effect_index];
            effect.state = OrchestrationEffectState::Succeeded;
            effect.settled_at = Some(meta.at.clone());
            if provider_effect_id.is_some() {
                effect.provider_effect_id = provider_effect_id;
            }
            let agent = &mut snapshot.agents[agent_index];
            agent.state = AgentNodeState::Ready;
            agent.session_key = Some(session_key);
            agent.provider_session_id = Some(provider_session_id);
            agent.updated_at.clone_from(&meta.at);
            (
                meta,
                "agent.launch-settled".to_owned(),
                OrchestrationEntityKind::Agent,
                agent_id,
            )
        }
        OrchestrationMutation::AcquireWorktreeLease { lease, meta } => {
            capacity(
                snapshot.worktree_leases.len(),
                ORCHESTRATION_LIMITS.worktree_leases,
                "worktree lease",
            )?;
            if snapshot
                .worktree_leases
                .iter()
                .any(|item| item.lease_id == lease.lease_id)
            {
                return fail(
                    OrchestrationErrorCode::AlreadyExists,
                    format!("worktree lease {} already exists", lease.lease_id),
                );
            }
            if snapshot
                .worktree_leases
                .iter()
                .any(|item| item.canonical_path == lease.canonical_path && live_lease(item.state))
            {
                return fail(
                    OrchestrationErrorCode::AlreadyExists,
                    "worktree path already has a live or ambiguous lease",
                );
            }
            if lease.state != WorktreeLeaseState::Active || lease.owner_revision != 0 {
                return fail(
                    OrchestrationErrorCode::InvalidMutation,
                    "new worktree leases must be active at owner revision zero",
                );
            }
            let owner_index = index_of(
                &snapshot.agents,
                |agent| agent.agent_id == lease.owner_agent_id,
                &format!("worktree owner agent {}", lease.owner_agent_id),
            )?;
            let owner_root = snapshot.agents[owner_index]
                .scope
                .canonical_project_root
                .as_deref()
                .unwrap_or(&snapshot.agents[owner_index].scope.canonical_cwd);
            if owner_root != lease.repository_root {
                return fail(
                    OrchestrationErrorCode::InvalidMutation,
                    "worktree repositoryRoot must match the owner's project root",
                );
            }
            if snapshot.agents[owner_index]
                .scope
                .worktree_lease_id
                .is_some()
            {
                return fail(
                    OrchestrationErrorCode::InvalidTransition,
                    "worktree owner already has a managed lease",
                );
            }
            create_time(&meta.at, &lease.acquired_at, "lease.acquiredAt")?;
            create_time(&meta.at, &lease.updated_at, "lease.updatedAt")?;
            let lease_id = lease.lease_id.clone();
            let cwd = lease
                .canonical_cwd
                .clone()
                .unwrap_or_else(|| lease.canonical_path.clone());
            let root = lease.repository_root.clone();
            snapshot.worktree_leases.push(lease);
            snapshot.agents[owner_index].scope.canonical_cwd = cwd;
            snapshot.agents[owner_index].scope.canonical_project_root = Some(root);
            snapshot.agents[owner_index].scope.worktree_lease_id = Some(lease_id.clone());
            snapshot.agents[owner_index].updated_at.clone_from(&meta.at);
            (
                meta,
                "worktree.acquired".to_owned(),
                OrchestrationEntityKind::WorktreeLease,
                lease_id,
            )
        }
        OrchestrationMutation::BeginWorktreeEscrowIntent {
            lease_id,
            escrow,
            effect,
            meta,
        } => {
            let lease_index = index_of(
                &snapshot.worktree_leases,
                |item| item.lease_id == lease_id,
                &format!("worktree lease {lease_id}"),
            )?;
            capacity(
                snapshot.effects.len(),
                ORCHESTRATION_LIMITS.effects,
                "effect",
            )?;
            guard_escrow_admission(&snapshot, lease_index, &escrow)?;
            admit_new_effect(
                &snapshot,
                &effect,
                "worktree escrow idempotency key already exists",
            )?;
            fresh_intent_effect(
                &snapshot,
                &effect,
                OrchestrationEffectKind::TransferWorktree,
                OrchestrationEffectTargetKind::WorktreeLease,
                &lease_id,
                "worktree escrow requires a matching unattempted transfer effect",
            )?;
            create_time(&meta.at, &escrow.prepared_at, "escrow.preparedAt")?;
            create_time(&meta.at, &effect.reserved_at, "effect.reservedAt")?;
            snapshot.worktree_leases[lease_index].state = WorktreeLeaseState::Escrow;
            snapshot.worktree_leases[lease_index].escrow = Some(escrow);
            snapshot.worktree_leases[lease_index]
                .updated_at
                .clone_from(&meta.at);
            snapshot.effects.push(effect);
            (
                meta,
                "worktree.escrow-intent-prepared".to_owned(),
                OrchestrationEntityKind::WorktreeLease,
                lease_id,
            )
        }
        OrchestrationMutation::BeginWorktreeEscrow {
            lease_id,
            escrow,
            meta,
        } => {
            let lease_index = index_of(
                &snapshot.worktree_leases,
                |item| item.lease_id == lease_id,
                &format!("worktree lease {lease_id}"),
            )?;
            guard_escrow_admission(&snapshot, lease_index, &escrow)?;
            create_time(&meta.at, &escrow.prepared_at, "escrow.preparedAt")?;
            snapshot.worktree_leases[lease_index].state = WorktreeLeaseState::Escrow;
            snapshot.worktree_leases[lease_index].escrow = Some(escrow);
            snapshot.worktree_leases[lease_index]
                .updated_at
                .clone_from(&meta.at);
            (
                meta,
                "worktree.escrow-prepared".to_owned(),
                OrchestrationEntityKind::WorktreeLease,
                lease_id,
            )
        }
        OrchestrationMutation::SettleWorktreeEscrow {
            lease_id,
            state,
            meta,
        } => {
            let lease_index = index_of(
                &snapshot.worktree_leases,
                |item| item.lease_id == lease_id,
                &format!("worktree lease {lease_id}"),
            )?;
            let escrow = snapshot.worktree_leases[lease_index]
                .escrow
                .clone()
                .filter(|item| item.state == WorktreeEscrowState::Prepared);
            let Some(escrow) = escrow else {
                return fail(
                    OrchestrationErrorCode::InvalidTransition,
                    "worktree lease has no prepared escrow",
                );
            };
            if snapshot.worktree_leases[lease_index].state != WorktreeLeaseState::Escrow {
                return fail(
                    OrchestrationErrorCode::InvalidTransition,
                    "worktree lease has no prepared escrow",
                );
            }
            at_or_after(&meta.at, &escrow.prepared_at, "escrow.settledAt")?;
            let (escrow_state, lease_state, suffix) = match state {
                SettledEscrowState::Completed => (
                    WorktreeEscrowState::Completed,
                    WorktreeLeaseState::Active,
                    "completed",
                ),
                SettledEscrowState::Failed => (
                    WorktreeEscrowState::Failed,
                    WorktreeLeaseState::Failed,
                    "failed",
                ),
                SettledEscrowState::Unknown => (
                    WorktreeEscrowState::Unknown,
                    WorktreeLeaseState::Unknown,
                    "unknown",
                ),
            };
            let stored_escrow = snapshot.worktree_leases[lease_index]
                .escrow
                .as_mut()
                .ok_or_else(|| {
                    OrchestrationError::new(
                        OrchestrationErrorCode::InvalidState,
                        "prepared escrow disappeared",
                    )
                })?;
            stored_escrow.state = escrow_state;
            stored_escrow.settled_at = Some(meta.at.clone());
            if state == SettledEscrowState::Completed {
                let source_index = index_of(
                    &snapshot.agents,
                    |agent| agent.agent_id == escrow.source_agent_id,
                    &format!("worktree source agent {}", escrow.source_agent_id),
                )?;
                let target_index = index_of(
                    &snapshot.agents,
                    |agent| agent.agent_id == escrow.target_agent_id,
                    &format!("worktree target agent {}", escrow.target_agent_id),
                )?;
                if snapshot.agents[source_index]
                    .scope
                    .worktree_lease_id
                    .as_deref()
                    == Some(&lease_id)
                {
                    snapshot.agents[source_index].scope.worktree_lease_id = None;
                    snapshot.agents[source_index]
                        .updated_at
                        .clone_from(&meta.at);
                }
                let cwd = snapshot.worktree_leases[lease_index]
                    .canonical_cwd
                    .clone()
                    .unwrap_or_else(|| {
                        snapshot.worktree_leases[lease_index].canonical_path.clone()
                    });
                let root = snapshot.worktree_leases[lease_index]
                    .repository_root
                    .clone();
                snapshot.agents[target_index].scope.canonical_cwd = cwd;
                snapshot.agents[target_index].scope.canonical_project_root = Some(root);
                snapshot.agents[target_index].scope.worktree_lease_id = Some(lease_id.clone());
                snapshot.agents[target_index]
                    .updated_at
                    .clone_from(&meta.at);
                snapshot.worktree_leases[lease_index].owner_agent_id = escrow.target_agent_id;
                bump_owner_revision(&mut snapshot.worktree_leases[lease_index])?;
            }
            snapshot.worktree_leases[lease_index].state = lease_state;
            snapshot.worktree_leases[lease_index]
                .updated_at
                .clone_from(&meta.at);
            (
                meta,
                format!("worktree.escrow-{suffix}"),
                OrchestrationEntityKind::WorktreeLease,
                lease_id,
            )
        }
        OrchestrationMutation::ReconcileWorktreeLease {
            lease_id,
            expected_owner_revision,
            state,
            meta,
        } => {
            let lease_index = index_of(
                &snapshot.worktree_leases,
                |item| item.lease_id == lease_id,
                &format!("worktree lease {lease_id}"),
            )?;
            if snapshot.worktree_leases[lease_index].state != WorktreeLeaseState::Active
                || snapshot.worktree_leases[lease_index].owner_revision != expected_owner_revision
            {
                return fail(
                    OrchestrationErrorCode::InvalidTransition,
                    "worktree lease owner revision changed before reconciliation",
                );
            }
            at_or_after(
                &meta.at,
                &snapshot.worktree_leases[lease_index].updated_at,
                "lease reconciliation time",
            )?;
            let (lease_state, suffix) = match state {
                ReconciledLeaseState::Released => (WorktreeLeaseState::Released, "released"),
                ReconciledLeaseState::Unknown => (WorktreeLeaseState::Unknown, "unknown"),
            };
            snapshot.worktree_leases[lease_index].state = lease_state;
            bump_owner_revision(&mut snapshot.worktree_leases[lease_index])?;
            snapshot.worktree_leases[lease_index]
                .updated_at
                .clone_from(&meta.at);
            if state == ReconciledLeaseState::Released {
                snapshot.worktree_leases[lease_index].released_at = Some(meta.at.clone());
                clear_lease_references(&mut snapshot, &lease_id, &meta.at);
            }
            (
                meta,
                format!("worktree.reconciled-{suffix}"),
                OrchestrationEntityKind::WorktreeLease,
                lease_id,
            )
        }
        OrchestrationMutation::SetWorktreeCoordinator {
            lease_id,
            coordinator,
            meta,
        } => {
            let lease_index = index_of(
                &snapshot.worktree_leases,
                |item| item.lease_id == lease_id,
                &format!("worktree lease {lease_id}"),
            )?;
            let Some(current) = snapshot.worktree_leases[lease_index].coordinator.as_ref() else {
                return fail(
                    OrchestrationErrorCode::InvalidTransition,
                    "only an active coordinated lease carries a coordinator",
                );
            };
            if snapshot.worktree_leases[lease_index].state != WorktreeLeaseState::Active {
                return fail(
                    OrchestrationErrorCode::InvalidTransition,
                    "only an active coordinated lease carries a coordinator",
                );
            }
            if coordinator.kind != current.kind
                || coordinator.task_id != current.task_id
                || coordinator.owner != current.owner
                || coordinator.epoch < current.epoch
                || coordinator.fence != current.fence
                || coordinator.write_scope != current.write_scope
                || coordinator.resources != current.resources
            {
                return fail(
                    OrchestrationErrorCode::InvalidMutation,
                    "a coordinator update keeps its task, owner, scope, fence and epoch order",
                );
            }
            at_or_after(
                &meta.at,
                &snapshot.worktree_leases[lease_index].updated_at,
                "lease.updatedAt",
            )?;
            snapshot.worktree_leases[lease_index].coordinator = Some(coordinator);
            snapshot.worktree_leases[lease_index]
                .updated_at
                .clone_from(&meta.at);
            (
                meta,
                "worktree.coordinator-updated".to_owned(),
                OrchestrationEntityKind::WorktreeLease,
                lease_id,
            )
        }
        OrchestrationMutation::ReleaseWorktreeLease { lease_id, meta } => {
            let lease_index = index_of(
                &snapshot.worktree_leases,
                |item| item.lease_id == lease_id,
                &format!("worktree lease {lease_id}"),
            )?;
            if snapshot.worktree_leases[lease_index].state != WorktreeLeaseState::Active {
                return fail(
                    OrchestrationErrorCode::InvalidTransition,
                    "only an active lease can release",
                );
            }
            at_or_after(
                &meta.at,
                &snapshot.worktree_leases[lease_index].updated_at,
                "lease.releasedAt",
            )?;
            snapshot.worktree_leases[lease_index].state = WorktreeLeaseState::Released;
            bump_owner_revision(&mut snapshot.worktree_leases[lease_index])?;
            snapshot.worktree_leases[lease_index]
                .updated_at
                .clone_from(&meta.at);
            snapshot.worktree_leases[lease_index].released_at = Some(meta.at.clone());
            clear_lease_references(&mut snapshot, &lease_id, &meta.at);
            (
                meta,
                "worktree.released".to_owned(),
                OrchestrationEntityKind::WorktreeLease,
                lease_id,
            )
        }
    };

    append_event(
        &mut snapshot,
        &meta,
        kind,
        entity_kind,
        entity_id,
        event_ids,
    )?;
    snapshot.validate().map_err(|error| {
        OrchestrationError::new(OrchestrationErrorCode::InvalidMutation, error.to_string())
    })?;
    Ok(snapshot)
}
