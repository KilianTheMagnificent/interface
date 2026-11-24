//! Cross-model collaboration commands (/consult, /handoff, /advisor,
//! /collab, /peer), the input guard for sessions a collaboration owns, and
//! their pure argument parsers.
use interface_contracts::ipc::{
    CollaborationApprovalAction, CollaborationEngine, CollaborationPeerAction,
    CollaborationSubmitRequest,
};

use super::{
    AgentController, CollaborationMode, CollaborationState, CollaborationStatusDto, TranscriptCell,
    TuiApp, for_display, parsed_app_command,
};

impl TuiApp {
    pub(super) async fn collaboration_input_blocked(&mut self, text: &str) -> bool {
        let Some(key) = self.controller.active_key() else {
            return false;
        };
        let Some(control) = self.controller.collaboration.clone() else {
            return false;
        };
        let rows = match control.list().await {
            Ok(rows) => rows,
            Err(error) => {
                self.notice = Some(error);
                return true;
            }
        };
        let target = rows
            .iter()
            .any(|row| row.target_session_key.as_deref() == Some(key.as_str()));
        let source = rows.iter().any(|row| {
            row.source_session_key == key
                && matches!(
                    row.receipt.state,
                    CollaborationState::ParkingSource
                        | CollaborationState::OwnershipTransfer
                        | CollaborationState::ActivatingTarget
                        | CollaborationState::Unknown
                )
        });
        if target {
            let safe = parsed_app_command(text).is_some_and(|(name, _)| {
                matches!(
                    name.as_str(),
                    "sessions"
                        | "new"
                        | "commands"
                        | "details"
                        | "window"
                        | "steers"
                        | "stop"
                        | "notifications"
                        | "diagnostics"
                        | "collab"
                        | "help"
                        | "quit"
                        | "exit"
                )
            });
            if safe {
                return false;
            }
            self.notice = Some("Input is disabled while iface owns this peer session. Its safety profile, approvals, and prompt are host-controlled.".into());
            return true;
        }
        if source {
            self.notice = Some(
                "The source is frozen until the target is ready and the source is safely parked."
                    .into(),
            );
            return true;
        }
        false
    }

    #[allow(clippy::too_many_lines)]
    pub(super) async fn run_collaboration(
        &mut self,
        mode: CollaborationMode,
        argument: &str,
        advisor: bool,
    ) {
        let config = self.config.get();
        let Some(control) = self.controller.collaboration.clone() else {
            self.notice = Some(if config.collaboration {
                "Durable collaboration is unavailable in this iface runtime. Your source session was not changed."
                    .into()
            } else {
                "Collaboration is disabled in your iface config (collaboration: false). Your source session was not changed."
                    .into()
            });
            return;
        };
        let Some(source_key) = self.controller.active_key() else {
            self.notice = Some(
                "Select a live source session before collaborating with another model.".into(),
            );
            return;
        };
        let Ok(source) = self.controller.agent_session(&source_key) else {
            self.notice = Some(
                "Select a live source session before collaborating with another model.".into(),
            );
            return;
        };
        let Some(source_engine) = collaboration_engine(&source.engine) else {
            self.notice = Some(
                "Cross-model collaboration currently connects Claude, GPT/Codex, and Grok sessions."
                    .into(),
            );
            return;
        };
        let parsed = if advisor {
            parse_advisor_argument(
                argument,
                source_engine,
                &advisor_objective(&self.controller.transcript_snapshot()),
            )
        } else {
            parse_collaboration_request(argument, source_engine, mode)
        };
        let parsed = match parsed {
            Ok(parsed) => parsed,
            Err(message) => {
                self.notice = Some(message);
                return;
            }
        };
        let target_id = collaboration_engine_id(parsed.target);
        if self
            .controller
            .probe(target_id)
            .is_none_or(|probe| probe.info.is_none())
        {
            self.notice = Some(format!(
                "{} is not healthy in this iface runtime.",
                collaboration_engine_label(parsed.target)
            ));
            return;
        }
        if !source.live {
            self.notice =
                Some("Resume the source session before starting a consultation or handoff.".into());
            return;
        }
        let submitted = control
            .submit(CollaborationSubmitRequest {
                source_session_key: source_key,
                mode,
                target_engine: parsed.target,
                objective: parsed.objective,
                client_request_id: format!(
                    "{}-{}",
                    if advisor { "advisor" } else { "human" },
                    uuid::Uuid::new_v4()
                ),
            })
            .await;
        let mut status = match submitted {
            Ok(status) => status,
            Err(error) => {
                self.notice = Some(error);
                return;
            }
        };
        if status.receipt.state == CollaborationState::AwaitingApproval {
            let Some(preview) = status.approval.clone() else {
                self.notice = Some(
                    "The durable request exists, but its approval preview changed. Use /collab before authorizing it."
                        .into(),
                );
                return;
            };
            status = match control
                .approve(CollaborationApprovalAction {
                    collaboration_id: preview.collaboration_id,
                    approval_id: preview.approval_id,
                    expected_revision: preview.expected_revision,
                })
                .await
            {
                Ok(status) => status,
                Err(error) => {
                    self.notice = Some(error);
                    return;
                }
            };
        }
        self.show_collaboration_receipt(&status, true);
    }

    pub(super) async fn advisor_command(&mut self, argument: &str) {
        self.run_collaboration(CollaborationMode::Consult, argument, true)
            .await;
    }

    #[allow(clippy::too_many_lines)]
    pub(super) async fn collaboration_command(&mut self, argument: &str) {
        let config = self.config.get();
        let Some(control) = self.controller.collaboration.clone() else {
            self.notice = Some(if config.collaboration {
                "This runtime did not start the host-owned collaboration coordinator.".into()
            } else {
                "Collaboration is disabled in your iface config (collaboration: false).".into()
            });
            return;
        };
        let rows = match control.list().await {
            Ok(rows) => rows,
            Err(error) => {
                self.notice = Some(error);
                return;
            }
        };
        let mut words = argument.split_whitespace();
        let verb = words.next().unwrap_or("status").to_ascii_lowercase();
        let selector = words.next().unwrap_or_default();
        if argument.is_empty() || matches!(verb.as_str(), "status" | "list") {
            self.show_collaboration_status(&rows);
            return;
        }
        let approvals = rows
            .iter()
            .filter(|row| row.approval.is_some())
            .collect::<Vec<_>>();
        match verb.as_str() {
            "show" => {
                let Some(chosen) = self
                    .pick_collaboration(&approvals, selector, |row| &row.receipt.collaboration_id)
                else {
                    return;
                };
                let Some(preview) = chosen.approval.as_ref() else {
                    return;
                };
                self.show_details(
                    format!(
                        "{} approval · {} → {}",
                        collaboration_mode_text(preview.mode),
                        collaboration_engine_id(preview.source_engine),
                        collaboration_engine_id(preview.target_engine)
                    ),
                    vec![
                        for_display(&preview.objective),
                        format!(
                            "{} · {} bytes · expires {}",
                            for_display(&preview.canonical_project_root),
                            preview.packet_bytes,
                            for_display(&preview.expires_at)
                        ),
                        for_display(&preview.packet_preview),
                        format!(
                            "/collab approve {} · /collab deny {}",
                            preview.collaboration_id, preview.collaboration_id
                        ),
                    ],
                );
            }
            "approve" | "deny" => {
                let Some(chosen) = self
                    .pick_collaboration(&approvals, selector, |row| &row.receipt.collaboration_id)
                else {
                    return;
                };
                let Some(preview) = chosen.approval.as_ref() else {
                    return;
                };
                let action = CollaborationApprovalAction {
                    collaboration_id: preview.collaboration_id.clone(),
                    approval_id: preview.approval_id.clone(),
                    expected_revision: preview.expected_revision,
                };
                let result = if verb == "approve" {
                    control.approve(action).await
                } else {
                    control.deny(action).await
                };
                match result {
                    Ok(status) => self.show_collaboration_receipt(&status, false),
                    Err(error) => self.notice = Some(error),
                }
            }
            "cancel" => {
                let active = rows.iter().collect::<Vec<_>>();
                let Some(chosen) =
                    self.pick_collaboration(&active, selector, |row| &row.receipt.collaboration_id)
                else {
                    return;
                };
                match control
                    .cancel(chosen.receipt.collaboration_id.clone())
                    .await
                {
                    Ok(status) => self.show_collaboration_receipt(&status, false),
                    Err(error) => self.notice = Some(error),
                }
            }
            _ => {
                self.notice = Some("Use /collab, /collab show [number|id], /collab approve|deny [number|id], or /collab cancel <number|id>.".into());
            }
        }
    }

    fn show_collaboration_status(&mut self, rows: &[CollaborationStatusDto]) {
        let approvals = rows
            .iter()
            .filter_map(|row| row.approval.as_ref())
            .collect::<Vec<_>>();
        let peers = rows
            .iter()
            .filter_map(|row| row.peer.as_ref())
            .collect::<Vec<_>>();
        let reviewable = peers
            .iter()
            .filter(|peer| {
                peer.state == interface_contracts::ipc::CollaborationPeerState::AwaitingReview
            })
            .collect::<Vec<_>>();
        let deliveries = peers
            .iter()
            .filter(|peer| {
                peer.state == interface_contracts::ipc::CollaborationPeerState::DeliveryPending
            })
            .collect::<Vec<_>>();
        let mut lines = vec![format!("Pending approvals: {}", approvals.len())];
        lines.extend(approvals.iter().enumerate().map(|(index, row)| {
            format!(
                "  {}. {} {} → {} · {} · {}",
                index + 1,
                collaboration_mode_text(row.mode),
                collaboration_engine_id(row.source_engine),
                collaboration_engine_id(row.target_engine),
                for_display(&row.objective),
                row.collaboration_id
            )
        }));
        lines.push(format!("Active durable operations: {}", rows.len()));
        lines.extend(rows.iter().enumerate().map(|(index, row)| {
            format!(
                "  {}. {} {} → {} · {} · r{} · {}",
                index + 1,
                collaboration_mode_text(row.receipt.mode),
                collaboration_engine_id(row.receipt.source_engine),
                collaboration_engine_id(row.receipt.target_engine),
                collaboration_state_text(row.receipt.state),
                row.receipt.revision,
                row.receipt.collaboration_id
            )
        }));
        lines.extend(collaboration_peer_lines(
            format!("Peer results awaiting review: {}", reviewable.len()),
            reviewable.into_iter().map(|peer| &**peer),
        ));
        lines.extend(collaboration_peer_lines(
            format!(
                "Accepted peer deliveries waiting for source rebind: {}",
                deliveries.len()
            ),
            deliveries.into_iter().map(|peer| &**peer),
        ));
        lines.push(
            "/collab show · /collab approve|deny · /collab cancel · /peer accept|discard".into(),
        );
        self.show_details("Durable collaboration", lines);
    }

    fn show_collaboration_receipt(&mut self, status: &CollaborationStatusDto, human: bool) {
        let receipt = &status.receipt;
        self.notice = Some(match receipt.state {
            CollaborationState::Approved => format!(
                "{} durable request {}. The target will remain visible and all provider effects are journaled.",
                if human {
                    "Your command authorized"
                } else {
                    "Host approval authorized"
                },
                receipt.collaboration_id
            ),
            CollaborationState::AwaitingApproval => format!(
                "The source guard changed or this request came from a model. Review /collab show before approving {}.",
                receipt.collaboration_id
            ),
            CollaborationState::Denied | CollaborationState::Cancelled => format!(
                "No new provider effect will start for {}.",
                receipt.collaboration_id
            ),
            CollaborationState::Failed | CollaborationState::Unknown => format!(
                "{} · {}",
                receipt
                    .error
                    .as_ref()
                    .map_or("BROKER_UNAVAILABLE", |error| error.code.as_str()),
                receipt.collaboration_id
            ),
            _ => format!(
                "Durable request {} is at revision {}.",
                receipt.collaboration_id, receipt.revision
            ),
        });
    }

    pub(super) async fn peer_command(&mut self, argument: &str) {
        let Some(source_key) = self.controller.active_key() else {
            self.notice = Some("Select the source session that owns the peer result.".into());
            return;
        };
        let words = argument.split_whitespace().collect::<Vec<_>>();
        if words.is_empty()
            || words.len() > 2
            || !matches!(words[0].to_ascii_lowercase().as_str(), "accept" | "discard")
            || words
                .get(1)
                .is_some_and(|number| number.parse::<usize>().is_err())
        {
            self.notice = Some("Use /peer accept [number] or /peer discard [number].".into());
            return;
        }
        let Some(control) = self.controller.collaboration.clone() else {
            self.notice = Some(
                "That queue entry is not a held peer result. Open /queue to inspect the current order."
                    .into(),
            );
            return;
        };
        let rows = match control.list().await {
            Ok(rows) => rows,
            Err(error) => {
                self.notice = Some(error);
                return;
            }
        };
        let peers = rows
            .iter()
            .filter(|row| {
                row.source_session_key == source_key
                    && row.peer.as_ref().is_some_and(|peer| {
                        peer.state
                            == interface_contracts::ipc::CollaborationPeerState::AwaitingReview
                    })
            })
            .collect::<Vec<_>>();
        let selected = words.get(1).map_or_else(
            || peers.first().copied(),
            |number| {
                number
                    .parse::<usize>()
                    .ok()
                    .and_then(|number| number.checked_sub(1))
                    .and_then(|index| peers.get(index).copied())
            },
        );
        let Some(selected) = selected else {
            self.notice = Some(
                "That queue entry is not a held peer result. Open /queue to inspect the current order."
                    .into(),
            );
            return;
        };
        let Some(peer) = selected.peer.as_ref() else {
            return;
        };
        let action = CollaborationPeerAction {
            collaboration_id: peer.collaboration_id.clone(),
            result_id: peer.result_id.clone(),
            result_digest: peer.result_digest.clone(),
            expected_revision: peer.expected_revision,
        };
        let accepted = words[0].eq_ignore_ascii_case("accept");
        let result = if accepted {
            control.accept_peer(action).await
        } else {
            control.discard_peer(action).await
        };
        match result {
            Ok(status) => {
                self.show_collaboration_receipt(&status, false);
                if accepted {
                    self.notice = Some(
                        "Peer result accepted. It will enter the source only through the durable delivery effect."
                            .into(),
                    );
                } else {
                    self.notice = Some("Peer result discarded.".into());
                }
            }
            Err(error) => self.notice = Some(error),
        }
    }

    /// Resolves one durable collaboration row, recording the refusal as the
    /// notice when the selector does not name exactly one.
    fn pick_collaboration<'a, T>(
        &mut self,
        rows: &[&'a T],
        selector: &str,
        id_for: impl Fn(&T) -> &str,
    ) -> Option<&'a T> {
        match select_collaboration_row(rows, selector, id_for) {
            Ok(row) => Some(row),
            Err(message) => {
                self.notice = Some(message);
                None
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ParsedCollaborationRequest {
    pub(super) target: CollaborationEngine,
    pub(super) objective: String,
}

fn collaboration_engine(value: &str) -> Option<CollaborationEngine> {
    match value {
        "claude" => Some(CollaborationEngine::Claude),
        "codex" => Some(CollaborationEngine::Codex),
        "acp:grok" => Some(CollaborationEngine::AcpGrok),
        _ => None,
    }
}

const fn collaboration_engine_id(value: CollaborationEngine) -> &'static str {
    match value {
        CollaborationEngine::Claude => "claude",
        CollaborationEngine::Codex => "codex",
        CollaborationEngine::AcpGrok => "acp:grok",
    }
}

const fn collaboration_engine_label(value: CollaborationEngine) -> &'static str {
    match value {
        CollaborationEngine::Claude => "Claude",
        CollaborationEngine::Codex => "GPT/Codex",
        CollaborationEngine::AcpGrok => "Grok",
    }
}

const fn collaboration_mode_text(value: CollaborationMode) -> &'static str {
    match value {
        CollaborationMode::Consult => "consult",
        CollaborationMode::Handoff => "handoff",
    }
}

const fn collaboration_state_text(value: CollaborationState) -> &'static str {
    match value {
        CollaborationState::Requested => "requested",
        CollaborationState::AwaitingApproval => "awaiting_approval",
        CollaborationState::Approved => "approved",
        CollaborationState::Preflight => "preflight",
        CollaborationState::PreparingTarget => "preparing_target",
        CollaborationState::RunningTarget => "running_target",
        CollaborationState::ResultReady => "result_ready",
        CollaborationState::AwaitingPeerReview => "awaiting_peer_review",
        CollaborationState::DeliveryPending => "delivery_pending",
        CollaborationState::ParkingSource => "parking_source",
        CollaborationState::OwnershipTransfer => "ownership_transfer",
        CollaborationState::ActivatingTarget => "activating_target",
        CollaborationState::Completed => "completed",
        CollaborationState::Denied => "denied",
        CollaborationState::Cancelled => "cancelled",
        CollaborationState::Failed => "failed",
        CollaborationState::Unknown => "unknown",
    }
}

fn target_alias(value: &str) -> Option<CollaborationEngine> {
    match value.to_ascii_lowercase().as_str() {
        "claude" | "anthropic" => Some(CollaborationEngine::Claude),
        "codex" | "gpt" | "openai" => Some(CollaborationEngine::Codex),
        "grok" | "xai" | "x.ai" => Some(CollaborationEngine::AcpGrok),
        _ => None,
    }
}

pub(super) fn parse_collaboration_request(
    argument: &str,
    source: CollaborationEngine,
    mode: CollaborationMode,
) -> Result<ParsedCollaborationRequest, String> {
    let trimmed = argument.trim();
    let (target_text, supplied) = trimmed
        .split_once(char::is_whitespace)
        .map_or((trimmed, ""), |(head, tail)| (head, tail.trim()));
    let objective = if supplied.is_empty() && mode == CollaborationMode::Handoff {
        "Continue the project from its current state and complete the remaining work."
    } else {
        supplied
    };
    let Some(target) = target_alias(target_text) else {
        return Err("Choose Claude, GPT/Codex, or Grok as the collaboration target.".into());
    };
    if target == source {
        return Err("Choose a different model so this is an independent cross-model pass.".into());
    }
    if objective.is_empty() {
        return Err("Add the question or handoff objective after the target model.".into());
    }
    if objective.encode_utf16().count() > 4_000 {
        return Err("The collaboration objective is limited to 4,000 characters.".into());
    }
    Ok(ParsedCollaborationRequest {
        target,
        objective: objective.to_owned(),
    })
}

const fn default_advisor_target(source: CollaborationEngine) -> CollaborationEngine {
    if matches!(source, CollaborationEngine::Claude) {
        CollaborationEngine::Codex
    } else {
        CollaborationEngine::Claude
    }
}

fn advisor_objective(cells: &[TranscriptCell]) -> String {
    cells
        .iter()
        .rev()
        .find_map(|cell| match cell {
            TranscriptCell::Message {
                role: "user",
                markdown,
            } if !markdown.trim().is_empty() => Some(format!(
                "Review the latest turn: {}",
                take_utf16_units(markdown, 240)
            )),
            _ => None,
        })
        .unwrap_or_else(|| "Review the latest settled turn for material risk.".into())
}

pub(super) fn parse_advisor_argument(
    argument: &str,
    source: CollaborationEngine,
    fallback_objective: &str,
) -> Result<ParsedCollaborationRequest, String> {
    let target = collaboration_engine_label(default_advisor_target(source)).to_ascii_lowercase();
    let target = if target == "gpt/codex" {
        "codex".to_owned()
    } else {
        target
    };
    let trimmed = argument.trim();
    if trimmed.is_empty() {
        return parse_collaboration_request(
            &format!("{target} {fallback_objective}"),
            source,
            CollaborationMode::Consult,
        );
    }
    let (first, rest) = trimmed
        .split_once(char::is_whitespace)
        .map_or((trimmed, ""), |(head, tail)| (head, tail.trim()));
    let as_target = if rest.is_empty() {
        format!("{first} {fallback_objective}")
    } else {
        trimmed.to_owned()
    };
    parse_collaboration_request(&as_target, source, CollaborationMode::Consult).or_else(|_| {
        parse_collaboration_request(
            &format!("{target} {trimmed}"),
            source,
            CollaborationMode::Consult,
        )
    })
}

fn take_utf16_units(value: &str, limit: usize) -> String {
    let mut units = 0;
    value
        .chars()
        .take_while(|character| {
            let next = units + character.len_utf16();
            if next > limit {
                false
            } else {
                units = next;
                true
            }
        })
        .collect()
}

pub(super) fn select_collaboration_row<'a, T>(
    rows: &[&'a T],
    selector: &str,
    id_for: impl Fn(&T) -> &str,
) -> Result<&'a T, String> {
    if rows.is_empty() {
        return Err("No matching durable request is pending.".into());
    }
    if selector.is_empty() {
        return if rows.len() == 1 {
            Ok(rows[0])
        } else {
            Err("More than one request is pending; supply its number or ID.".into())
        };
    }
    if selector.chars().all(|character| character.is_ascii_digit()) {
        return selector
            .parse::<usize>()
            .ok()
            .and_then(|number| number.checked_sub(1))
            .and_then(|index| rows.get(index).copied())
            .ok_or_else(|| "That collaboration number is not in the current list.".into());
    }
    let matches = rows
        .iter()
        .copied()
        .filter(|row| {
            let identifier = id_for(row);
            identifier == selector || identifier.starts_with(selector)
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [row] => Ok(*row),
        [] => Err("No collaboration ID matches that selector.".into()),
        _ => Err("That collaboration ID prefix is ambiguous.".into()),
    }
}

fn collaboration_peer_lines<'a>(
    header: String,
    rows: impl IntoIterator<Item = &'a interface_contracts::ipc::CollaborationPeerDto>,
) -> Vec<String> {
    let rows = rows.into_iter().collect::<Vec<_>>();
    let mut lines = vec![header];
    lines.extend(rows.iter().enumerate().map(|(index, row)| {
        format!(
            "  {}. {} → {} · {} bytes · {}",
            index + 1,
            collaboration_engine_id(row.target_engine),
            collaboration_engine_id(row.source_engine),
            row.result_bytes,
            row.collaboration_id
        )
    }));
    lines
}
