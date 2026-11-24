//! Protocol 50: an agent CLI that exits cleanly by itself gets a plain shell
//! in its pane, decided here the moment the exit is delivered, whatever the
//! window shows. The rule is `interface_terminal::shell_handoff`; the
//! renderer only moves the tab to the shell, whose summary `replaces` the
//! agent. A refused create leaves the agent's tab (and its Start again).

use std::sync::atomic::Ordering;

use super::{
    Arc, DispatchError, TerminalGeneration, TerminalHostDispatcher, TerminalId, TerminalState,
    terminal_closed_error, terminal_state_error,
};

impl TerminalHostDispatcher {
    /// Starts the one worker that hands cleanly exited agents a shell. Call
    /// it once, inside a Tokio runtime, before terminals start.
    ///
    /// # Errors
    ///
    /// Refuses a second worker or a dispatcher already closing.
    pub fn start_shell_handoff(self: &Arc<Self>) -> Result<(), DispatchError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(terminal_closed_error());
        }
        let mut slot = self
            .shell_handoff
            .lock()
            .map_err(|_| terminal_state_error())?;
        let (sender, mut exits) = tokio::sync::mpsc::unbounded_channel();
        if slot.is_some() || self.lifecycle_sink.shell_handoffs.set(sender).is_err() {
            return Err(DispatchError::new(
                "terminal-handoff-owner-conflict",
                "The terminal shell handoff already has an owner.",
                false,
                true,
            ));
        }
        let owner = Arc::downgrade(self);
        *slot = Some(tokio::spawn(async move {
            while let Some((id, generation)) = exits.recv().await {
                let Some(owner) = owner.upgrade() else {
                    break;
                };
                owner.hand_off_to_shell(id, generation).await;
            }
        }));
        Ok(())
    }

    /// Hands `id`'s exited `generation` a shell, then dismisses the agent.
    /// Holds the lifecycle dispatch gate throughout, as a renderer create
    /// and dismiss would each hold it, so no close, restart or Start again
    /// lands between the check, the create and the dismissal.
    async fn hand_off_to_shell(&self, id: TerminalId, generation: TerminalGeneration) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        let _lifecycle_dispatch = self.lifecycle_dispatch_gate.lock().await;
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        let Some(request) = self
            .service
            .shell_handoff_due(id, generation, TerminalId::random())
        else {
            return;
        };
        if let Err(error) = self.create_terminal_request(request, Some(id)).await {
            tracing::warn!(
                code = %error.code,
                %error.message,
                "an agent that ended by itself could not be handed a shell; its tab stays"
            );
            return;
        }
        let still_exited = self.service.snapshot().terminals.iter().any(|terminal| {
            terminal.id == id
                && terminal.generation == generation
                && terminal.state == TerminalState::Exited
        });
        if still_exited
            && let Err(error) = self.dismiss_terminal(serde_json::json!({ "id": id })).await
        {
            // The renderer's hidden-pane sweep ends it once its tab has moved.
            tracing::warn!(code = %error.code, "the agent a shell replaced was not dismissed");
        }
    }
}
