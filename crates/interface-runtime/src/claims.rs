//! One writer per provider session (review C1).
//!
//! A provider session can be driven from two places: the structured runtime
//! (a thread, a chat, a teammate) and a Code pane running the provider's CLI.
//! Both appending to one transcript would interleave two conversations, so
//! the runtime never opens, resumes or sends to a session while a running pane
//! is bound to it, nor while one is on its way to a pane (Open in terminal).
//! Moving a session between the two ends the side it leaves first: Open in
//! terminal closes the structured session before the pane starts, and Continue
//! as thread ends the pane before Threads takes it. Every refusal here is
//! definitely-unsent: nothing reached the provider.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use interface_common::sync::lock;
use interface_contracts::ipc::parse_key;

use crate::{RuntimeError, RuntimeErrorKind};

/// The provider sessions running Code panes are bound to.
pub trait TerminalSessionBindings: Send + Sync + 'static {
    /// Provider session ids (the part of a session key after `#`).
    fn bound_provider_sessions(&self) -> Vec<String>;
}

/// Who besides the structured runtime holds a provider session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionClaim {
    /// A running Code pane drives it.
    Terminal,
    /// It is on its way to a Code pane that has not started yet.
    HandingOff,
}

impl SessionClaim {
    /// Why the runtime will not drive the session, in the owner's words.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::Terminal => {
                "This session is open in a Code pane, so Interface will not drive it from here too. \
                 Continue there, or use Continue as thread on that pane."
            }
            Self::HandingOff => {
                "This session is opening in a Code pane. Continue there, or close that pane first."
            }
        }
    }
}

/// How long a hand-off holds a session for a pane that has not bound yet.
pub const HANDOFF_HOLD: Duration = Duration::from_secs(30);

/// The runtime's view of who else drives a provider session.
pub struct SessionClaims {
    terminals: Arc<dyn TerminalSessionBindings>,
    handoffs: Mutex<HashMap<String, Instant>>,
    hold: Duration,
}

impl SessionClaims {
    #[must_use]
    pub fn new(terminals: Arc<dyn TerminalSessionBindings>) -> Self {
        Self::with_hold(terminals, HANDOFF_HOLD)
    }

    /// As [`Self::new`], with a hand-off hold other than [`HANDOFF_HOLD`].
    #[must_use]
    pub fn with_hold(terminals: Arc<dyn TerminalSessionBindings>, hold: Duration) -> Self {
        Self {
            terminals,
            handoffs: Mutex::new(HashMap::new()),
            hold,
        }
    }

    /// Who else holds `session_key`, if anyone. A running pane wins over a
    /// hand-off; an expired hand-off is forgotten.
    #[must_use]
    pub fn claim(&self, session_key: &str) -> Option<SessionClaim> {
        let Ok((_, provider_session_id)) = parse_key(session_key) else {
            return None;
        };
        if self
            .terminals
            .bound_provider_sessions()
            .contains(&provider_session_id)
        {
            return Some(SessionClaim::Terminal);
        }
        let mut handoffs = lock(&self.handoffs);
        let hold = self.hold;
        handoffs.retain(|_, since| since.elapsed() < hold);
        handoffs
            .contains_key(session_key)
            .then_some(SessionClaim::HandingOff)
    }

    /// Holds `session_key` for a Code pane that is about to resume it.
    pub fn hand_to_terminal(&self, session_key: &str) {
        lock(&self.handoffs).insert(session_key.to_owned(), Instant::now());
    }

    /// Drops a hand-off: the pane bound, failed to start, or was closed.
    pub fn release_handoff(&self, session_key: &str) {
        lock(&self.handoffs).remove(session_key);
    }

    /// The definitely-unsent refusal for driving `session_key` from the
    /// structured runtime, when something else holds it.
    #[must_use]
    pub fn refusal(&self, session_key: &str) -> Option<RuntimeError> {
        self.claim(session_key).map(|claim| {
            RuntimeError::definitely_unsent(RuntimeErrorKind::Lifecycle, claim.message(), false)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use interface_engine::EffectCertainty;

    use super::*;

    #[derive(Default)]
    struct Panes(Mutex<Vec<String>>);

    impl TerminalSessionBindings for Panes {
        fn bound_provider_sessions(&self) -> Vec<String> {
            lock(&self.0).clone()
        }
    }

    #[test]
    fn a_running_pane_or_a_hand_off_holds_a_session_and_nothing_else_does() {
        let panes = Arc::new(Panes::default());
        let claims = SessionClaims::new(panes.clone());
        assert_eq!(claims.claim("claude#one"), None);
        assert!(claims.refusal("claude#one").is_none());

        lock(&panes.0).push("one".into());
        assert_eq!(claims.claim("claude#one"), Some(SessionClaim::Terminal));
        assert_eq!(claims.claim("codex#two"), None);
        let refusal = claims.refusal("claude#one").unwrap();
        assert_eq!(refusal.certainty, EffectCertainty::DefinitelyUnsent);
        assert!(!refusal.retryable);
        assert!(refusal.message.contains("open in a Code pane"));

        claims.hand_to_terminal("codex#two");
        assert_eq!(claims.claim("codex#two"), Some(SessionClaim::HandingOff));
        claims.release_handoff("codex#two");
        assert_eq!(claims.claim("codex#two"), None);
        assert_eq!(claims.claim("not a key"), None);
    }

    #[test]
    fn a_hand_off_lapses_after_its_hold() {
        let claims = SessionClaims::with_hold(Arc::new(Panes::default()), Duration::ZERO);
        claims.hand_to_terminal("claude#one");
        assert_eq!(claims.claim("claude#one"), None);
    }
}
