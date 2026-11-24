//! The fail-closed provider-effect protocol's host half: reserve and start an
//! agent effect, then settle it exactly once from its external outcome.

use interface_contracts::orchestration::{
    AgentNodeV1, EffectReservationV1, OrchestrationEffectKind, OrchestrationEffectState,
    OrchestrationEffectTargetKind, OrchestrationMutation,
};
use uuid::Uuid;

use crate::dispatcher::DispatchError;

use super::{ExternalOutcome, OrchestrationHostDispatcher, human_meta_at, sha256_text};

impl OrchestrationHostDispatcher {
    /// A new reservation in this runtime epoch, not yet attempted: the one
    /// shape every provider effect enters the store with.
    pub(super) fn reserved_effect(
        &self,
        kind: OrchestrationEffectKind,
        target_kind: OrchestrationEffectTargetKind,
        target_id: String,
        idempotency_key: String,
        request_hash: String,
        reserved_at: String,
    ) -> EffectReservationV1 {
        EffectReservationV1 {
            version: 1,
            effect_id: format!("effect_{}", Uuid::new_v4()),
            idempotency_key,
            kind,
            target_kind,
            target_id,
            state: OrchestrationEffectState::Reserved,
            attempts: 0,
            request_hash,
            runtime_epoch: self.runtime_epoch.clone(),
            reserved_at,
            started_at: None,
            settled_at: None,
            provider_effect_id: None,
            error_code: None,
            reconciliation_note: None,
        }
    }

    pub(super) async fn reserve_agent_effect(
        &self,
        agent: &AgentNodeV1,
        kind: OrchestrationEffectKind,
        stable: Option<(String, String)>,
    ) -> Result<EffectReservationV1, DispatchError> {
        let at = self.clock.now();
        let (idempotency_key, request_hash) = stable.unwrap_or_else(|| {
            (
                format!("{kind:?}:{}:{}", agent.agent_id, Uuid::new_v4()),
                sha256_text(&format!("{kind:?}:{}", agent.agent_id)),
            )
        });
        let effect = self.reserved_effect(
            kind,
            OrchestrationEffectTargetKind::Agent,
            agent.agent_id.clone(),
            idempotency_key,
            request_hash,
            at.clone(),
        );
        self.apply(OrchestrationMutation::ReserveEffect {
            effect: effect.clone(),
            meta: human_meta_at(at, format!("Reserved {kind:?}")),
        })
        .await?;
        self.apply(OrchestrationMutation::StartEffect {
            effect_id: effect.effect_id.clone(),
            meta: self.meta(format!("Crossed provider boundary for {kind:?}")),
        })
        .await?;
        Ok(effect)
    }

    pub(super) async fn settle_effect<T>(
        &self,
        effect_id: &str,
        outcome: &ExternalOutcome<T>,
        label: &str,
    ) -> Result<(), DispatchError> {
        let provider_effect_id = match outcome {
            ExternalOutcome::Succeeded {
                provider_effect_id, ..
            } => provider_effect_id.clone(),
            _ => None,
        };
        self.apply(OrchestrationMutation::SettleEffect {
            effect_id: effect_id.into(),
            state: outcome.effect_state(),
            provider_effect_id,
            error_code: outcome.error_code(),
            reconciliation_note: None,
            meta: self.meta(format!("Settled {label}")),
        })
        .await?;
        Ok(())
    }
}
