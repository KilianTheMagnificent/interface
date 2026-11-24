//! Mid-session settings: model, effort, approval policy and Fast mode.

use std::sync::Arc;

use interface_common::sync::lock;
use interface_contracts::engine::{EngineEvent, EngineEventKind, FastModeState};
use interface_engine::{DeliveryOutcome, EngineError, EngineErrorKind};

use super::{
    PERMISSION_MODES,
    commands::spawn_command_refresh,
    models::fast_tier_for,
    session::{ServiceTierOverride, SessionShared},
};

const AGENT_MODE_REFUSAL: &str =
    "This agent's approval policy comes from its settings; change it there";

pub(super) enum ConfigChange {
    Model(Option<String>),
    Effort(Option<String>),
    Permission(String),
}

pub(super) fn set_config(shared: &Arc<SessionShared>, change: ConfigChange) -> DeliveryOutcome<()> {
    if shared.core.is_closed() {
        return shared.core.closed_outcome("change configuration");
    }
    if let Err(error) = lock(&shared.core.lifecycle).require_sendable() {
        return DeliveryOutcome::DefinitelyUnsent(error.into());
    }
    let (events, refresh_commands) = {
        let mut config = lock(&shared.config);
        match change {
            ConfigChange::Model(model) => {
                if model.as_deref().is_some_and(str::is_empty) {
                    return invalid_config("model cannot be empty");
                }
                config.model.clone_from(&model);
                let mut events = Vec::with_capacity(2);
                if matches!(config.service_tier, ServiceTierOverride::Exact(_)) {
                    let next_tier = lock(&shared.fast_tiers).tier_for(model.as_deref());
                    if let Some(next_tier) = next_tier {
                        config.service_tier = ServiceTierOverride::Exact(next_tier);
                    } else {
                        config.service_tier = ServiceTierOverride::Standard;
                        events.push(EngineEventKind::FastmodeChanged {
                            state: FastModeState::Off,
                            reason: None,
                        });
                    }
                }
                events.push(EngineEventKind::ConfigChanged {
                    model,
                    effort: Some(config.effort.clone()),
                });
                {
                    let mut commands = lock(&shared.commands);
                    commands.commands.clear();
                    commands.skills.clear();
                }
                events.push(EngineEventKind::CommandsInvalidated);
                (events, true)
            }
            ConfigChange::Effort(effort) => {
                if effort.as_deref().is_some_and(str::is_empty) {
                    return invalid_config("effort cannot be empty");
                }
                config.effort.clone_from(&effort);
                (
                    vec![EngineEventKind::ConfigChanged {
                        model: config.model.clone(),
                        effort: Some(effort),
                    }],
                    false,
                )
            }
            ConfigChange::Permission(mode) => {
                if shared.approval_gate {
                    return DeliveryOutcome::DefinitelyUnsent(EngineError::definitely_unsent(
                        EngineErrorKind::Unsupported,
                        "This agent session runs behind Interface's approval gate, so its Codex approval policy cannot change",
                        false,
                    ));
                }
                if shared.agent_mode.is_some() {
                    return DeliveryOutcome::DefinitelyUnsent(EngineError::definitely_unsent(
                        EngineErrorKind::Unsupported,
                        AGENT_MODE_REFUSAL,
                        false,
                    ));
                }
                if !PERMISSION_MODES.contains(&mode.as_str()) {
                    return invalid_config("unsupported Codex permission mode");
                }
                if lock(&shared.protocol).active_turn_id().is_some() {
                    return DeliveryOutcome::DefinitelyUnsent(EngineError::definitely_unsent(
                        EngineErrorKind::Lifecycle,
                        format!(
                            "Codex cannot change the approval policy of a turn that is already running; stop it, then set {mode:?} again"
                        ),
                        false,
                    ));
                }
                config.approval_override = Some(mode.clone());
                config.approval_display = Some(mode.clone());
                (vec![EngineEventKind::AccessChanged { mode }], false)
            }
        }
    };
    for event in events {
        let _ = shared.core.emit(EngineEvent { at: None, event });
    }
    if refresh_commands {
        spawn_command_refresh(Arc::clone(shared), true);
    }
    DeliveryOutcome::Confirmed(())
}

pub(super) async fn set_fast_mode(
    shared: &Arc<SessionShared>,
    enabled: bool,
) -> DeliveryOutcome<()> {
    let _guard = shared.effect_lock.lock().await;
    if shared.core.is_closed() {
        return shared.core.closed_outcome("change Fast mode");
    }
    if let Err(error) = lock(&shared.core.lifecycle).require_sendable() {
        return DeliveryOutcome::DefinitelyUnsent(error.into());
    }
    let transport = match shared.core.transport() {
        Ok(transport) => transport,
        Err(error) => return DeliveryOutcome::from_error(error),
    };
    let current_model = {
        let config = lock(&shared.config);
        config
            .model
            .clone()
            .or_else(|| config.resolved_model.clone())
    };
    let tier = match fast_tier_for(&transport, &shared.fast_tiers, current_model.as_deref()).await {
        Ok(Some(tier)) => tier,
        Ok(None) => {
            return DeliveryOutcome::DefinitelyUnsent(EngineError::definitely_unsent(
                EngineErrorKind::Unsupported,
                format!(
                    "Codex model {} does not advertise a Fast service tier.",
                    current_model.as_deref().unwrap_or("(provider default)")
                ),
                false,
            ));
        }
        Err(error) => return DeliveryOutcome::from_error(error),
    };
    lock(&shared.config).service_tier = if enabled {
        ServiceTierOverride::Exact(tier)
    } else {
        ServiceTierOverride::Standard
    };
    let _ = shared.core.emit(EngineEvent {
        at: None,
        event: EngineEventKind::FastmodeChanged {
            state: if enabled {
                FastModeState::On
            } else {
                FastModeState::Off
            },
            reason: None,
        },
    });
    DeliveryOutcome::Confirmed(())
}

fn invalid_config(message: &str) -> DeliveryOutcome<()> {
    DeliveryOutcome::DefinitelyUnsent(EngineError::definitely_unsent(
        EngineErrorKind::InvalidInput,
        message,
        false,
    ))
}
