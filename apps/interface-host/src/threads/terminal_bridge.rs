//! The bridges between Threads and Code (review C1, protocol 42).
//!
//! A provider session has one writer at a time ([`SessionClaims`]). Moving it
//! ends the side it leaves first: Open in terminal closes the thread's
//! structured session and holds the session for the Code pane the renderer
//! opens next; Continue as thread waits until no running pane drives the
//! session (the renderer ends the pane first) before Threads takes it.

use std::time::Duration;

use interface_contracts::{
    engine::{SessionKind, SessionStatus},
    ipc::parse_key,
    terminal::TerminalLaunchProfileId,
    threads::{ThreadControlResponse, ThreadTerminalHandoff},
};
use interface_runtime::SessionClaim;
use interface_settings::threads::ThreadRefusal;

use super::{THREADS_UNKNOWN_SESSION_CODE, ThreadHost, refused};
use crate::dispatcher::DispatchError;

/// A pane drives the session, or one is opening for it.
pub const THREADS_IN_TERMINAL_CODE: &str = "threads-in-terminal";
/// Only Claude and Codex sessions resume in a Code pane.
pub const THREADS_TERMINAL_UNSUPPORTED_CODE: &str = "threads-terminal-unsupported";
/// A turn is running; a session moves only between turns.
pub const THREADS_SESSION_BUSY_CODE: &str = "threads-session-busy";
/// The structured session did not confirm its close; it may still be open.
pub const THREADS_CLOSE_UNCONFIRMED_CODE: &str = "threads-close-unconfirmed";

/// How long Continue as thread waits for an ended pane to exit.
const PANE_EXIT_WAIT: Duration = Duration::from_secs(5);
const PANE_EXIT_POLL: Duration = Duration::from_millis(100);

fn refusal(code: &'static str, message: &str) -> DispatchError {
    refused(ThreadRefusal::new(code, message))
}

impl ThreadHost {
    /// Open in terminal: the thread's structured session closes and the
    /// session is held for the Code pane the answer names.
    pub(super) async fn open_in_terminal(
        &self,
        key: String,
    ) -> Result<ThreadControlResponse, DispatchError> {
        let (engine, provider_session_id) = parse_key(&key).map_err(|_| {
            refusal(
                THREADS_UNKNOWN_SESSION_CODE,
                "That is not a thread Interface knows.",
            )
        })?;
        let profile_id = match engine.as_str() {
            "claude" => TerminalLaunchProfileId::Claude,
            "codex" => TerminalLaunchProfileId::Codex,
            _ => {
                return Err(refusal(
                    THREADS_TERMINAL_UNSUPPORTED_CODE,
                    "Only Claude and Codex threads open in a Code pane.",
                ));
            }
        };
        let session = self
            .inner
            .ports
            .sessions
            .session(&key)
            .map_err(|_| {
                refusal(
                    THREADS_UNKNOWN_SESSION_CODE,
                    "The session list could not be read. Try again.",
                )
            })?
            .filter(|session| session.kind != Some(SessionKind::Agent))
            .ok_or_else(|| {
                refusal(
                    THREADS_UNKNOWN_SESSION_CODE,
                    "That thread is no longer in Interface. Refresh the list and try again.",
                )
            })?;
        if matches!(
            session.status,
            SessionStatus::Working | SessionStatus::Waiting
        ) {
            return Err(refusal(
                THREADS_SESSION_BUSY_CODE,
                "A turn is running. Stop it or let it finish, then open the thread in a terminal.",
            ));
        }
        let claims = &self.inner.ports.claims;
        if let Some(claim) = claims.claim(&key) {
            return Err(refusal(THREADS_IN_TERMINAL_CODE, claim.message()));
        }
        // No send starts while the session changes hands.
        let _gate = self.hold_send_gate(&key)?;
        // End the structured side first; an unconfirmed close leaves the
        // session where it was, never with two writers.
        self.inner.ports.sessions.close(&key).await.map_err(|error| {
            DispatchError::new(
                THREADS_CLOSE_UNCONFIRMED_CODE,
                format!(
                    "The thread's session did not confirm it closed, so it was not opened in a terminal: {error}"
                ),
                false,
                false,
            )
        })?;
        claims.hand_to_terminal(&key);
        let mut response = self.view_response().await?;
        response.terminal = Some(ThreadTerminalHandoff {
            profile_id,
            provider_session_id,
            cwd: session.cwd,
        });
        Ok(response)
    }

    /// Continue as thread: once no running pane drives the session, Threads
    /// takes it: Active, at the top, visited.
    pub(super) async fn continue_from_terminal(
        &self,
        key: String,
    ) -> Result<ThreadControlResponse, DispatchError> {
        if parse_key(&key).is_err() {
            return Err(refusal(
                THREADS_UNKNOWN_SESSION_CODE,
                "That is not a session Interface knows.",
            ));
        }
        let claims = &self.inner.ports.claims;
        let waited = std::time::Instant::now();
        while claims.claim(&key) == Some(SessionClaim::Terminal) {
            if waited.elapsed() >= PANE_EXIT_WAIT {
                return Err(refusal(
                    THREADS_IN_TERMINAL_CODE,
                    "A Code pane is still running this session. End that pane, then continue it as a thread.",
                ));
            }
            tokio::time::sleep(PANE_EXIT_POLL).await;
        }
        // A pane that never bound, or that this pane replaced, holds nothing now.
        claims.release_handoff(&key);
        let target = key.clone();
        self.metadata(&key, move |document, now| {
            document.unsettle(&target, now)?;
            document.visit(&target, now)
        })
        .await
    }
}
