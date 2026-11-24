//! Team tools (`iface_team`, T3 section 2) as each engine adapter wires them:
//! the host's factory makes one binding per session, the adapter checks its
//! descriptor, names the provider process it spawned, hands the provider
//! the server, confirms the session's exact key once the provider has named
//! it, and disposes of it when the session ends. A binding that cannot be made or confirmed leaves the session
//! without team tools (with a notice); it never stops the session, since a
//! session without them can do less, not more.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use interface_contracts::engine::{
    CollaborationMcpSessionBinding, CollaborationMcpSessionRequest,
    CollaborationMcpStdioDescriptor, EngineId, TEAM_TOOLS_SERVER, TeamToolsFactory, session_key,
};

/// The notice a session gets when its team tools could not be set up.
pub const TEAM_TOOLS_UNAVAILABLE: &str =
    "Interface team tools are unavailable for this session; it started without them.";

/// What asking the factory came to.
pub enum TeamSetup {
    /// No team tools for this session: the owner turned them off, or the host
    /// does not offer them to it. Nothing to say.
    None,
    /// The binding could not be made or was invalid: the session starts
    /// without team tools and says so.
    Unavailable,
    Ready(TeamBinding),
}

/// One session's `iface_team` binding.
pub struct TeamBinding {
    binding: Arc<dyn CollaborationMcpSessionBinding>,
    descriptor: CollaborationMcpStdioDescriptor,
    disposed: AtomicBool,
}

impl TeamBinding {
    /// The stdio server the provider is given.
    #[must_use]
    pub const fn descriptor(&self) -> &CollaborationMcpStdioDescriptor {
        &self.descriptor
    }

    /// Names the provider process right after it is spawned, before the
    /// provider is given the server: the host answers only that process and
    /// the processes it started. `None` (it exited before its pid was read)
    /// names nothing, and the binding then answers nothing.
    pub async fn provider_started(&self, process_id: Option<u32>) {
        if let Some(process_id) = process_id {
            self.binding.provider_process_started(process_id).await;
        }
    }

    /// Binds the server to `engine#provider_id`, the session's key. Answers
    /// whether it took; a refusal disposes of the binding.
    pub async fn confirm(&self, engine: &EngineId, provider_id: &str) -> bool {
        let confirmed = self
            .binding
            .confirm_source_session_key(session_key(engine, provider_id))
            .await
            .is_ok();
        if !confirmed {
            self.dispose().await;
        }
        confirmed
    }

    /// Revokes the binding once; later calls do nothing.
    pub async fn dispose(&self) {
        if !self.disposed.swap(true, Ordering::AcqRel) {
            let _ = self.binding.dispose().await;
        }
    }
}

/// Asks `factory` for the session's binding and checks its descriptor (a
/// valid stdio server named [`TEAM_TOOLS_SERVER`]).
pub async fn create_team_binding(
    factory: &Arc<dyn TeamToolsFactory>,
    engine: &EngineId,
    provisional_session_id: &str,
    resumed: bool,
) -> TeamSetup {
    let binding = match factory
        .create(CollaborationMcpSessionRequest {
            engine: engine.clone(),
            provisional_session_id: provisional_session_id.to_owned(),
            resumed,
        })
        .await
    {
        Ok(Some(binding)) => binding,
        Ok(None) => return TeamSetup::None,
        Err(_) => return TeamSetup::Unavailable,
    };
    let descriptor = binding.descriptor();
    if descriptor.validate().is_err() || descriptor.name != TEAM_TOOLS_SERVER {
        let _ = binding.dispose().await;
        return TeamSetup::Unavailable;
    }
    TeamSetup::Ready(TeamBinding {
        binding,
        descriptor,
        disposed: AtomicBool::new(false),
    })
}
