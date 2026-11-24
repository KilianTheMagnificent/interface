//! Read-only and control connections: the kept listing child, the spare
//! history lanes, and the isolated connections provider mutations use.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use interface_engine::{
    EngineError, EngineErrorKind,
    jsonrpc::{FrameError, RouteEvent, RpcRemoteError},
    supervised::InboundReceiver,
};
use serde_json::{Value, json};
use tokio::sync::Mutex as AsyncMutex;

use crate::{
    protocol::CodexMethod,
    transport::{CodexConnection, CodexTransportEvent},
};

use super::{CodexAdapter, CodexCollaborationRuntime, open::initialized_session_connection};

/// One spare read-only history lane. `returns` counts the times a connection
/// was put back, always under the slot lock, so an idle reaper scheduled for
/// one put-back can tell that the lane has been used since.
#[derive(Default)]
pub(super) struct SpareLane {
    pub(super) slot: AsyncMutex<Option<ControlConnection>>,
    returns: AtomicU64,
}

/// A control-plane app-server connection that shuts its child down when it is
/// dropped, including when the caller's future is cancelled mid-flight. The
/// guard cannot live on `CodexConnection` itself: `open_session` moves
/// `.inbound` out of the connection it owns.
pub(super) struct ControlConnection(CodexConnection);

impl Drop for ControlConnection {
    fn drop(&mut self) {
        self.0.transport.shutdown();
    }
}

impl std::ops::Deref for ControlConnection {
    type Target = CodexConnection;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for ControlConnection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// A connection for one read-only operation: the kept listing child, held in
/// its locked slot and put back healthy, or a fresh child that ends with it.
pub(super) struct ReadLane<'a> {
    pub(super) connection: ControlConnection,
    slot: Option<tokio::sync::MutexGuard<'a, Option<ControlConnection>>>,
}

impl ReadLane<'_> {
    /// Returns a still-open kept child to its slot for the next read.
    pub(super) fn finish(self) {
        if let Some(mut slot) = self.slot
            && !self.connection.transport.is_closed()
        {
            *slot = Some(self.connection);
        }
    }
}

impl CodexAdapter {
    /// Control operations never boot the collaboration broker. The notice that
    /// explains a broker startup failure is only read on the session path, so
    /// latching `process_enabled`/`startup_unavailable` from a background
    /// refresh would disable collaboration for the adapter's lifetime with
    /// nothing to tell the user why.
    pub(super) async fn initialized_connection(&self) -> Result<ControlConnection, EngineError> {
        initialized_session_connection(
            &self.config.executable_path.get(),
            &self.config.client_version,
            &self.config.transport,
            &CodexCollaborationRuntime::default(),
            None,
            &self.config.environment,
        )
        .await
        .map(|(connection, _)| ControlConnection(connection))
    }

    /// Answers every server request on a control connection with -32601.
    /// On a `read_only` connection an oversize frame also fails the whole
    /// connection: the frame was dropped, so a read it answered would
    /// otherwise wait out the request timeout. In-flight reads then settle as
    /// outcome-unknown and nothing is replayed. Malformed JSON stays ignored,
    /// and a mutation's connection is never cut.
    pub(super) fn drain_control_requests(
        connection: &mut CodexConnection,
        operation: &'static str,
        read_only: bool,
    ) {
        let transport = Arc::clone(&connection.transport);
        let mut inbound = std::mem::replace(&mut connection.inbound, InboundReceiver::closed());
        tokio::spawn(async move {
            while let Some(event) = inbound.recv().await {
                match event {
                    CodexTransportEvent::Route(RouteEvent::ServerRequest(request)) => {
                        let _ = transport
                            .server_error(
                                &request.id,
                                &RpcRemoteError {
                                    code: -32_601,
                                    message: format!(
                                        "client request unavailable during Codex {operation}"
                                    ),
                                    data: None,
                                },
                            )
                            .await;
                    }
                    CodexTransportEvent::ProtocolFault(FrameError::TooLarge { .. })
                        if read_only =>
                    {
                        transport.shutdown();
                    }
                    _ => {}
                }
            }
        });
    }

    /// Checks a read-only lane's connection out of its slot. The caller holds
    /// the slot's lock for the whole operation, so the disposed check here
    /// cannot race `dispose()` putting a child back, and the caller alone
    /// decides whether the connection goes back.
    pub(super) async fn checkout_lane(
        &self,
        slot: &mut Option<ControlConnection>,
        operation: &'static str,
    ) -> Result<ControlConnection, EngineError> {
        if self.collaboration.adapter_disposed.load(Ordering::Acquire) {
            return Err(EngineError::definitely_unsent(
                EngineErrorKind::Lifecycle,
                "Codex adapter is disposed",
                false,
            ));
        }
        if let Some(connection) = slot
            .take()
            .filter(|connection| !connection.transport.is_closed())
        {
            return Ok(connection);
        }
        let mut connection = self.initialized_connection().await?;
        Self::drain_control_requests(&mut connection, operation, true);
        Ok(connection)
    }

    /// Read-only requests use the kept listing child when its lane is free,
    /// rather than starting and initializing another app-server each time. A
    /// listing can hold the lane for its whole budget, so a busy lane is never
    /// waited for: the read gets a fresh child, as it always did.
    pub(super) async fn read_lane(
        &self,
        operation: &'static str,
    ) -> Result<ReadLane<'_>, EngineError> {
        if let Ok(mut slot) = self.listing_connection.try_lock() {
            let connection = self.checkout_lane(&mut slot, operation).await?;
            return Ok(ReadLane {
                connection,
                slot: Some(slot),
            });
        }
        let mut connection = self.initialized_connection().await?;
        Self::drain_control_requests(&mut connection, operation, true);
        Ok(ReadLane {
            connection,
            slot: None,
        })
    }

    pub(super) async fn control_request(
        &self,
        method: &str,
        params: Option<Value>,
        operation: &'static str,
    ) -> Result<Value, EngineError> {
        let mut connection = self.initialized_connection().await?;
        Self::drain_control_requests(&mut connection, operation, false);
        let outcome = match params {
            Some(params) => connection.transport.request(method, &params).await,
            None => connection.transport.request_without_params(method).await,
        };
        outcome.into_result()
    }

    /// Stored-history refreshes share the supervised read-only listing child.
    /// Take it out of the slot while awaiting so cancellation/error drops it.
    /// Provider mutations continue using isolated, non-replayed connections.
    pub(super) async fn history_read_request(
        &self,
        session_id: String,
    ) -> Result<Value, EngineError> {
        if let Ok(mut slot) = self.listing_connection.try_lock() {
            return self.read_history_on(&mut slot, session_id).await;
        }
        for (index, lane) in self.history_connections.iter().enumerate() {
            if let Ok(mut slot) = lane.slot.try_lock() {
                let response = self.read_history_on(&mut slot, session_id).await;
                if slot.is_some() {
                    let returned = lane.returns.fetch_add(1, Ordering::AcqRel) + 1;
                    drop(slot);
                    self.reap_idle_spare(index, returned);
                }
                return response;
            }
        }
        let mut slot = self.listing_connection.lock().await;
        self.read_history_on(&mut slot, session_id).await
    }

    /// Drops a spare lane's child once it has sat unused for `spare_idle`. A
    /// lane put back again since is left to that put-back's reaper, and a lane
    /// in use now is left to its reader.
    fn reap_idle_spare(&self, index: usize, returned: u64) {
        let lanes = Arc::clone(&self.history_connections);
        let idle = self.spare_idle;
        tokio::spawn(async move {
            tokio::time::sleep(idle).await;
            let lane = &lanes[index];
            if let Ok(mut slot) = lane.slot.try_lock()
                && lane.returns.load(Ordering::Acquire) == returned
            {
                slot.take();
            }
        });
    }

    async fn read_history_on(
        &self,
        slot: &mut Option<ControlConnection>,
        session_id: String,
    ) -> Result<Value, EngineError> {
        let connection = self.checkout_lane(slot, "history read").await?;
        let response = connection
            .transport
            .request(
                CodexMethod::THREAD_READ,
                &json!({"threadId": session_id, "includeTurns": true}),
            )
            .await
            .into_result()?;
        if !connection.transport.is_closed() {
            *slot = Some(connection);
        }
        Ok(response)
    }
}
