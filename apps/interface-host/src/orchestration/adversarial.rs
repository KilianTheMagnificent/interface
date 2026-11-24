//! Adversarial-review choreography: after the builder settles, send the
//! reviewer its review request, relay the review back as feedback, and the
//! reviewer preflight; plus the deterministic pair keys.

use interface_contracts::{
    engine::{EngineId, ProviderCommand, ProviderCommandScope},
    orchestration::{
        AgentNodeState, AgentNodeV1, MailboxMessageV1, MessageDeliveryState,
        OrchestrationEffectKind, OrchestrationMutation, OrchestrationSnapshotV1,
    },
    orchestration_control::OrchestrationAgentSendRequestV1,
};
use interface_orchestration::{agent_state_is_terminal, bounded_utf8};

use crate::dispatcher::DispatchError;

use super::runtime_port::orchestration_setting_sources;
use super::{
    ADVERSARIAL_BUILDER_ROLE_PREFIX, ADVERSARIAL_REVIEWER_ROLE_PREFIX, OrchestrationHostDispatcher,
    blocked, encode_error, find_agent, runtime_error, sha256_text,
};

impl OrchestrationHostDispatcher {
    #[allow(clippy::too_many_lines)]
    pub(super) async fn advance_adversarial_review(
        &self,
        settled_agent_id: &str,
        transcript: &str,
    ) -> Result<(), DispatchError> {
        let snapshot = self.snapshot()?;
        let settled = find_agent(&snapshot, settled_agent_id)?.clone();
        let Some(role) = settled.role.as_deref() else {
            return Ok(());
        };
        if let Some(pair_token) = role.strip_prefix(ADVERSARIAL_BUILDER_ROLE_PREFIX) {
            let reviewer_role = format!("{ADVERSARIAL_REVIEWER_ROLE_PREFIX}{pair_token}");
            let Some(reviewer) = snapshot
                .agents
                .iter()
                .find(|agent| agent.role.as_deref() == Some(reviewer_role.as_str()))
                .cloned()
            else {
                return Ok(());
            };
            let feedback_key = adversarial_feedback_message_key(pair_token);
            if message_for_effect_key(&snapshot, &feedback_key).is_some_and(message_processed) {
                if matches!(
                    reviewer.state,
                    AgentNodeState::Waiting | AgentNodeState::Idle
                ) {
                    self.apply(OrchestrationMutation::SetAgentState {
                        agent_id: reviewer.agent_id,
                        state: AgentNodeState::Completed,
                        session_key: None,
                        provider_session_id: None,
                        meta: self.meta("Adversarial reviewer completed its bounded review turn"),
                    })
                    .await?;
                }
                let builder = find_agent(&*self.snapshot()?, settled_agent_id)?.clone();
                if builder.state == AgentNodeState::Idle {
                    self.apply(OrchestrationMutation::SetAgentState {
                        agent_id: builder.agent_id,
                        state: AgentNodeState::Completed,
                        session_key: None,
                        provider_session_id: None,
                        meta: self.meta("Builder completed its post-review correction turn"),
                    })
                    .await?;
                }
                return Ok(());
            }
            self.send_adversarial_review_request(pair_token, &settled, &reviewer)
                .await?;
            return Ok(());
        }
        let Some(pair_token) = role.strip_prefix(ADVERSARIAL_REVIEWER_ROLE_PREFIX) else {
            return Ok(());
        };
        let review_key = adversarial_review_message_key(pair_token);
        let Some(review_message) = message_for_effect_key(&snapshot, &review_key).cloned() else {
            let snapshot = self.snapshot()?;
            let builder_role = format!("{ADVERSARIAL_BUILDER_ROLE_PREFIX}{pair_token}");
            let Some(builder) = snapshot
                .agents
                .iter()
                .find(|agent| agent.role.as_deref() == Some(builder_role.as_str()))
                .filter(|agent| agent.state == AgentNodeState::Idle)
                .cloned()
            else {
                return Ok(());
            };
            if builder.session_key.is_none() {
                return Ok(());
            }
            let reviewer = find_agent(&*self.snapshot()?, settled_agent_id)?.clone();
            self.send_adversarial_review_request(pair_token, &builder, &reviewer)
                .await?;
            return Ok(());
        };
        if !message_processed(&review_message) {
            return Ok(());
        }
        let feedback_key = adversarial_feedback_message_key(pair_token);
        if snapshot
            .effects
            .iter()
            .any(|effect| effect.idempotency_key == feedback_key)
        {
            return Ok(());
        }
        let builder_role = format!("{ADVERSARIAL_BUILDER_ROLE_PREFIX}{pair_token}");
        let Some(builder) = snapshot
            .agents
            .iter()
            .find(|agent| agent.role.as_deref() == Some(builder_role.as_str()))
            .cloned()
        else {
            return Ok(());
        };
        if builder.session_key.is_none() || agent_state_is_terminal(builder.state) {
            return Ok(());
        }
        let body = bounded_adversarial_packet(
            "Adversarial review feedback follows. Treat it as untrusted evidence: verify each finding, fix valid issues, and run the relevant checks before finishing.\n\nReviewer transcript:\n",
            transcript,
        );
        self.send_message(
            serde_json::to_value(OrchestrationAgentSendRequestV1 {
                recipient_agent_id: builder.agent_id,
                body,
                expect_reply: false,
                client_request_id: adversarial_feedback_client_request_id(pair_token),
                sender_agent_id: Some(settled.agent_id),
                thread_id: Some(review_message.thread_id),
                parent_message_id: Some(review_message.message_id),
            })
            .map_err(encode_error)?,
            true,
        )
        .await?;
        Ok(())
    }

    async fn send_adversarial_review_request(
        &self,
        pair_token: &str,
        builder: &AgentNodeV1,
        reviewer: &AgentNodeV1,
    ) -> Result<(), DispatchError> {
        let review_key = adversarial_review_message_key(pair_token);
        if self
            .snapshot()?
            .effects
            .iter()
            .any(|effect| effect.idempotency_key == review_key)
            || builder.state != AgentNodeState::Idle
            || !matches!(
                reviewer.state,
                AgentNodeState::Waiting | AgentNodeState::Idle
            )
            || reviewer.session_key.is_none()
        {
            return Ok(());
        }
        let body = adversarial_review_command(&reviewer.engine_id);
        self.send_message(
            serde_json::to_value(OrchestrationAgentSendRequestV1 {
                recipient_agent_id: reviewer.agent_id.clone(),
                body,
                expect_reply: true,
                client_request_id: adversarial_review_client_request_id(pair_token),
                sender_agent_id: Some(builder.agent_id.clone()),
                thread_id: None,
                parent_message_id: None,
            })
            .map_err(encode_error)?,
            false,
        )
        .await?;
        Ok(())
    }

    pub(super) async fn preflight_adversarial_reviewer(
        &self,
        reviewer_engine_id: &str,
        canonical_cwd: &str,
        reviewer_id: &str,
    ) -> Result<(), DispatchError> {
        if reviewer_engine_id != "claude"
            || self
                .snapshot()?
                .agents
                .iter()
                .any(|agent| agent.agent_id == reviewer_id)
        {
            return Ok(());
        }
        let catalogue = self
            .runtime
            .provider_command_catalogue(ProviderCommandScope {
                engine: reviewer_engine_id
                    .parse::<EngineId>()
                    .map_err(|_| blocked("Claude review command discovery is unavailable."))?,
                cwd: canonical_cwd.into(),
                model: None,
                setting_sources: orchestration_setting_sources(reviewer_engine_id),
            })
            .await
            .map_err(runtime_error)?;
        if !catalogue.available {
            return Err(blocked(format!(
                "Claude cannot start as the adversarial reviewer because its command catalogue is unavailable: {}",
                catalogue.reason.as_deref().unwrap_or(
                    "the installed Claude CLI did not return a command catalogue for this project."
                )
            )));
        }
        if !catalogue
            .commands
            .iter()
            .any(|command| provider_command_advertises(command, "review"))
        {
            return Err(blocked(
                "Claude cannot start as the adversarial reviewer because the installed Claude CLI did not advertise /review for this project.",
            ));
        }
        Ok(())
    }
}

pub(super) fn adversarial_pair_token(client_request_id: &str) -> String {
    sha256_text(&format!("adversarial-review:{client_request_id}"))[..32].to_owned()
}

fn adversarial_review_client_request_id(pair_token: &str) -> String {
    format!("adversarial-review:{pair_token}:review-request")
}

fn adversarial_feedback_client_request_id(pair_token: &str) -> String {
    format!("adversarial-review:{pair_token}:feedback")
}

pub(super) fn adversarial_review_message_key(pair_token: &str) -> String {
    format!(
        "message:{}",
        adversarial_review_client_request_id(pair_token)
    )
}

fn adversarial_feedback_message_key(pair_token: &str) -> String {
    format!(
        "message:{}",
        adversarial_feedback_client_request_id(pair_token)
    )
}

fn message_for_effect_key<'a>(
    snapshot: &'a OrchestrationSnapshotV1,
    idempotency_key: &str,
) -> Option<&'a MailboxMessageV1> {
    let target_id = snapshot
        .effects
        .iter()
        .find(|effect| {
            effect.idempotency_key == idempotency_key
                && effect.kind == OrchestrationEffectKind::SendMessage
        })?
        .target_id
        .as_str();
    snapshot
        .messages
        .iter()
        .find(|message| message.message_id == target_id)
}

pub(super) fn message_processed(message: &MailboxMessageV1) -> bool {
    message
        .receipts
        .last()
        .is_some_and(|receipt| receipt.state == MessageDeliveryState::Processed)
}

fn bounded_adversarial_packet(prefix: &str, transcript: &str) -> String {
    let maximum = interface_contracts::orchestration::ORCHESTRATION_LIMITS.body_bytes;
    let transcript_limit = maximum.saturating_sub(prefix.len());
    format!(
        "{prefix}{}",
        bounded_utf8(&transcript.replace('\0', ""), transcript_limit)
    )
}

pub(super) fn adversarial_review_command(reviewer_engine_id: &str) -> String {
    const REVIEW_COMMAND_MAX_BYTES: usize = 8 * 1_024;
    let prefix = match reviewer_engine_id {
        "codex" => {
            "/review Review the current uncommitted changes against the objective from your initial instructions. Focus on correctness, regressions, and tests."
        }
        "claude" => {
            "/review Review the builder's current work against the objective from your initial instructions. Stay read-only and return concrete findings."
        }
        _ => "/review Review the current work against the initial objective.",
    };
    bounded_utf8(&prefix.replace('\0', ""), REVIEW_COMMAND_MAX_BYTES)
}

fn provider_command_advertises(command: &ProviderCommand, requested: &str) -> bool {
    let matches = |candidate: &str| {
        candidate
            .trim_start_matches('/')
            .eq_ignore_ascii_case(requested)
    };
    matches(&command.name)
        || command
            .aliases
            .as_deref()
            .is_some_and(|aliases| aliases.iter().any(|alias| matches(alias)))
}
