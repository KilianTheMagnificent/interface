//! Test fakes the engine crates' adapter tests share. Built only for this
//! crate's tests and the `test-support` feature, which only dev-dependencies
//! enable.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use interface_common::sync::lock;
use interface_contracts::engine::{
    CollaborationBindingError, CollaborationMcpSessionBinding, CollaborationMcpSessionFactory,
    CollaborationMcpSessionRequest, CollaborationMcpStdioDescriptor, SessionKey,
};

/// Everything the fake collaboration broker was asked to do.
#[derive(Default)]
pub struct CollaborationTestState {
    pub requests: Mutex<Vec<CollaborationMcpSessionRequest>>,
    pub confirmed: Mutex<Vec<SessionKey>>,
    /// Each provider process a team tools binding was told of.
    pub providers: Mutex<Vec<u32>>,
    pub disposed: AtomicUsize,
}

/// A collaboration factory whose bindings describe `/usr/bin/nc -U <socket>`
/// and record each request, confirmation and disposal in `state`.
pub struct CollaborationTestFactory {
    pub state: Arc<CollaborationTestState>,
    pub socket: &'static str,
}

impl CollaborationTestFactory {
    #[must_use]
    pub fn shared(
        state: &Arc<CollaborationTestState>,
        socket: &'static str,
    ) -> Arc<dyn CollaborationMcpSessionFactory> {
        Arc::new(Self {
            state: Arc::clone(state),
            socket,
        })
    }
}

struct CollaborationTestBinding {
    state: Arc<CollaborationTestState>,
    socket: &'static str,
}

#[async_trait]
impl CollaborationMcpSessionFactory for CollaborationTestFactory {
    async fn create(
        &self,
        request: CollaborationMcpSessionRequest,
    ) -> Result<Arc<dyn CollaborationMcpSessionBinding>, CollaborationBindingError> {
        lock(&self.state.requests).push(request);
        Ok(Arc::new(CollaborationTestBinding {
            state: Arc::clone(&self.state),
            socket: self.socket,
        }))
    }
}

#[async_trait]
impl CollaborationMcpSessionBinding for CollaborationTestBinding {
    fn descriptor(&self) -> CollaborationMcpStdioDescriptor {
        CollaborationMcpStdioDescriptor {
            name: "iface_collaboration".into(),
            command: "/usr/bin/nc".into(),
            args: vec!["-U".into(), self.socket.into()],
            env: Vec::new(),
        }
    }

    async fn confirm_source_session_key(
        &self,
        source_session_key: SessionKey,
    ) -> Result<(), CollaborationBindingError> {
        lock(&self.state.confirmed).push(source_session_key);
        Ok(())
    }

    async fn dispose(&self) -> Result<(), CollaborationBindingError> {
        self.state.disposed.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
}

/// A team tools factory (`iface_team`) whose bindings describe
/// `/usr/bin/nc -U <socket>`, record each request, confirmation and disposal
/// in `state`, and are offered only while `offer` is set (the owner's
/// setting, the host's started-thread ledger).
pub struct TeamToolsTestFactory {
    pub state: Arc<CollaborationTestState>,
    pub socket: &'static str,
    pub offer: bool,
}

impl std::fmt::Debug for TeamToolsTestFactory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TeamToolsTestFactory")
            .field("offer", &self.offer)
            .finish_non_exhaustive()
    }
}

impl TeamToolsTestFactory {
    #[must_use]
    pub fn shared(
        state: &Arc<CollaborationTestState>,
        socket: &'static str,
        offer: bool,
    ) -> Arc<dyn interface_contracts::engine::TeamToolsFactory> {
        Arc::new(Self {
            state: Arc::clone(state),
            socket,
            offer,
        })
    }
}

struct TeamToolsTestBinding {
    state: Arc<CollaborationTestState>,
    socket: &'static str,
}

#[async_trait]
impl interface_contracts::engine::TeamToolsFactory for TeamToolsTestFactory {
    async fn create(
        &self,
        request: CollaborationMcpSessionRequest,
    ) -> Result<Option<Arc<dyn CollaborationMcpSessionBinding>>, CollaborationBindingError> {
        lock(&self.state.requests).push(request);
        if !self.offer {
            return Ok(None);
        }
        Ok(Some(Arc::new(TeamToolsTestBinding {
            state: Arc::clone(&self.state),
            socket: self.socket,
        })))
    }
}

#[async_trait]
impl CollaborationMcpSessionBinding for TeamToolsTestBinding {
    fn descriptor(&self) -> CollaborationMcpStdioDescriptor {
        CollaborationMcpStdioDescriptor {
            name: interface_contracts::engine::TEAM_TOOLS_SERVER.into(),
            command: "/usr/bin/nc".into(),
            args: vec!["-U".into(), self.socket.into()],
            env: Vec::new(),
        }
    }

    async fn confirm_source_session_key(
        &self,
        source_session_key: SessionKey,
    ) -> Result<(), CollaborationBindingError> {
        lock(&self.state.confirmed).push(source_session_key);
        Ok(())
    }

    async fn provider_process_started(&self, process_id: u32) {
        lock(&self.state.providers).push(process_id);
    }

    async fn dispose(&self) -> Result<(), CollaborationBindingError> {
        self.state.disposed.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
}
