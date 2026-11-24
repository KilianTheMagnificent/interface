//! Provider starts, sends, stops, steers and resumes started from the keyboard.
//!
//! The key handler never awaits one of these inline. It queues a
//! [`ProviderEffect`], and the terminal loop runs it as a task of its own, so
//! drawing, other sessions' output and Ctrl+C/SIGTERM stay live while the
//! provider answers. One effect runs at a time. Keys and pastes that arrive
//! meanwhile wait, and are replayed in order once its result is applied —
//! the order the terminal held them in while the handler awaited inline. A
//! second Enter during a send therefore finds the composer already taken and
//! issues nothing, and effects keep their order.
//!
//! Only Ctrl+C skips the line: it quits at once. An effect still in flight
//! then is never aborted; the loop drops its handle, and shutdown's runtime
//! fence settles it. Approval answers and setting changes still await inline.
use std::{collections::VecDeque, future::Future, pin::Pin};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::{
    AgentDelivery, AgentDeliveryKind, AgentErrorCode, AgentProtocolError, AgentSessionState,
    StartFailure, TuiApp, UnknownDelivery, protocol,
};

pub(super) type EffectFuture = Pin<Box<dyn Future<Output = EffectOutcome> + Send>>;

/// One queued provider operation and the status text shown while it runs.
struct ProviderEffect {
    label: String,
    run: EffectFuture,
}

/// What a finished provider operation hands back to the UI.
pub(super) enum EffectOutcome {
    /// Enter on the active session: a send, or a queue while it works.
    Send {
        key: String,
        text: String,
        result: Result<AgentDelivery, AgentProtocolError>,
    },
    /// Enter with no active session: a new session started with the prompt,
    /// or `r` on such a start whose outcome is unknown.
    Start {
        text: String,
        result: Result<AgentSessionState, StartFailure>,
    },
    /// `r` on a prompt whose delivery is unknown.
    Resend {
        pending: UnknownDelivery,
        result: Result<AgentDelivery, AgentProtocolError>,
    },
    /// Esc, or `/stop` when `command` is set.
    Stop {
        command: bool,
        result: Result<AgentSessionState, AgentProtocolError>,
    },
    /// `/resume` or the session picker. On success `select` becomes the
    /// active, remembered session, and `shown_id` (else the resumed session's
    /// id) names it in the notice.
    Resume {
        select: Option<String>,
        shown_id: Option<String>,
        result: Result<AgentSessionState, AgentProtocolError>,
    },
    /// Ctrl+T.
    Steer {
        result: Result<(AgentDelivery, Option<String>), AgentProtocolError>,
    },
}

/// Terminal input that waits behind a pending effect.
pub(super) enum HeldInput {
    Key(KeyEvent),
    Paste(String),
}

#[derive(Default)]
pub(super) struct Effects {
    queued: VecDeque<ProviderEffect>,
    /// The label of the effect the terminal loop is running.
    running: Option<String>,
    held: VecDeque<HeldInput>,
}

/// Ctrl+C quits even while an effect is pending.
pub(super) fn is_quit_key(key: &KeyEvent) -> bool {
    key.kind != KeyEventKind::Release
        && matches!(key.code, KeyCode::Char('c' | 'C'))
        && key.modifiers.contains(KeyModifiers::CONTROL)
}

impl TuiApp {
    /// Hands a provider operation to the terminal loop instead of awaiting it.
    pub(super) fn queue_effect(
        &mut self,
        label: impl Into<String>,
        run: impl Future<Output = EffectOutcome> + Send + 'static,
    ) {
        self.effects.queued.push_back(ProviderEffect {
            label: label.into(),
            run: Box::pin(run),
        });
    }

    pub(super) fn effect_pending(&self) -> bool {
        self.effects.running.is_some() || !self.effects.queued.is_empty()
    }

    pub(super) fn effect_label(&self) -> Option<&str> {
        self.effects.running.as_deref().or_else(|| {
            self.effects
                .queued
                .front()
                .map(|effect| effect.label.as_str())
        })
    }

    /// The next effect to run, once none is running.
    pub(super) fn next_effect(&mut self) -> Option<EffectFuture> {
        if self.effects.running.is_some() {
            return None;
        }
        let effect = self.effects.queued.pop_front()?;
        self.effects.running = Some(effect.label);
        Some(effect.run)
    }

    /// Routes one input event: Ctrl+C quits at once; while an effect is
    /// pending everything else waits its turn.
    pub(super) async fn terminal_input(&mut self, input: HeldInput) {
        if let HeldInput::Key(key) = &input
            && is_quit_key(key)
        {
            self.quit = true;
            return;
        }
        if self.effect_pending() {
            self.effects.held.push_back(input);
            return;
        }
        self.dispatch(input).await;
    }

    async fn dispatch(&mut self, input: HeldInput) {
        match input {
            HeldInput::Key(key) => self.key(key).await,
            HeldInput::Paste(text) => self.paste(&text),
        }
    }

    /// Applies the running effect's result, then replays held input in order
    /// until it quits or queues another effect. Queued-send failures are
    /// drained after the result and after each replayed event, as the frame
    /// that used to follow each of them did.
    pub(super) async fn finish_effect(&mut self, outcome: EffectOutcome) {
        self.effects.running = None;
        self.apply_effect(outcome).await;
        while !self.effect_pending() {
            self.apply_queued_send_failures();
            if self.quit {
                break;
            }
            let Some(input) = self.effects.held.pop_front() else {
                break;
            };
            self.dispatch(input).await;
        }
    }

    /// The UI half of each operation, unchanged from when it ran inline.
    async fn apply_effect(&mut self, outcome: EffectOutcome) {
        match outcome {
            EffectOutcome::Send { key, text, result } => self.apply_send(key, text, result),
            EffectOutcome::Start { text, result } => self.apply_start(text, result),
            EffectOutcome::Resend { pending, result } => match result {
                Ok(delivery) if delivery.kind == AgentDeliveryKind::AdapterAccepted => {
                    self.delivery_unknown = None;
                    self.notice = Some("Prompt resent.".into());
                }
                Ok(delivery) if delivery.kind == AgentDeliveryKind::OutcomeUnknown => {
                    if let Some(current) = self.delivery_unknown.as_mut() {
                        current.message = delivery.message.unwrap_or(pending.message);
                    }
                }
                Ok(delivery) => {
                    self.delivery_unknown = None;
                    self.input.replace(pending.text);
                    self.notice = delivery.message;
                }
                Err(error) => {
                    self.delivery_unknown = None;
                    self.input.replace(pending.text);
                    self.notice = Some(error.message);
                }
            },
            EffectOutcome::Stop { command, result } => match result {
                Ok(_) if command => self.notice = Some("Turn interrupted.".into()),
                Ok(_) => {}
                Err(error) => self.notice = Some(error.message),
            },
            EffectOutcome::Resume {
                select,
                shown_id,
                result,
            } => {
                self.notice = match result {
                    Ok(session) => {
                        if let Some(key) = select {
                            self.controller.set_active(Some(key));
                            let launch = self.controller.launch();
                            self.config.remember_session(
                                launch.engine,
                                launch.model,
                                launch.effort,
                            );
                            self.save_config().await;
                        }
                        Some(format!(
                            "Resumed {}.",
                            shown_id.unwrap_or(session.session_id)
                        ))
                    }
                    Err(error) => Some(error.message),
                };
            }
            EffectOutcome::Steer { result } => match result {
                Ok((delivery, warning)) => {
                    self.notice = warning.or_else(|| {
                        (delivery.kind != AgentDeliveryKind::AdapterAccepted)
                            .then_some(delivery.message)
                            .flatten()
                    });
                }
                Err(error) => self.notice = Some(error.message),
            },
        }
    }

    /// Enter on the active session. A refusal from before the provider was
    /// called gives the prompt back; an unknown outcome shows the banner.
    fn apply_send(
        &mut self,
        key: String,
        text: String,
        result: Result<AgentDelivery, AgentProtocolError>,
    ) {
        let result = match result {
            Ok(delivery) if delivery.kind == AgentDeliveryKind::AdapterAccepted => {
                if delivery.queued {
                    self.notice = Some("Prompt queued for the active session.".into());
                }
                Ok(())
            }
            Ok(delivery) if delivery.kind == AgentDeliveryKind::OutcomeUnknown => {
                let message = delivery
                    .message
                    .unwrap_or_else(|| "The provider may have received this prompt.".into());
                self.delivery_unknown = Some(UnknownDelivery {
                    key: Some(key),
                    text,
                    message,
                });
                Ok(())
            }
            Ok(delivery) => {
                self.input.replace(text);
                Err(protocol(
                    AgentErrorCode::Failed,
                    delivery
                        .message
                        .unwrap_or_else(|| "Provider operation failed.".into()),
                ))
            }
            Err(error) => {
                // Refused before the provider was called.
                if matches!(
                    error.code,
                    AgentErrorCode::NotFound | AgentErrorCode::NotLive | AgentErrorCode::Blocked
                ) {
                    self.restore_prompt(text);
                }
                Err(error)
            }
        };
        if let Err(error) = result {
            self.notice = Some(error.message);
        }
    }

    /// A start from Enter, or the resend of a start whose outcome is
    /// unknown: a banner without a session key is that resend.
    fn apply_start(&mut self, text: String, result: Result<AgentSessionState, StartFailure>) {
        let resend = self
            .delivery_unknown
            .as_ref()
            .is_some_and(|pending| pending.key.is_none());
        match result {
            Ok(_) => {
                if resend {
                    self.delivery_unknown = None;
                    self.notice = Some("Prompt resent.".into());
                }
            }
            // Never reached the provider: the prompt goes back.
            Err(StartFailure {
                error,
                unknown: false,
            }) => {
                self.delivery_unknown = None;
                self.notice = Some(error.message);
                self.restore_prompt(text);
            }
            Err(StartFailure {
                error,
                unknown: true,
            }) => {
                self.delivery_unknown = Some(UnknownDelivery {
                    key: None,
                    text,
                    message: error.message,
                });
            }
        }
    }
}
