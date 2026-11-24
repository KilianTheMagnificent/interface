use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;
use interface_contracts::{
    engine::{EngineId, EngineUsage, ProviderCommandScope, SessionSummary, session_key},
    ipc::CHANNELS,
};
use interface_runtime::{
    ImportProgress, ImportProgressSink, RuntimeBridgeCommand, RuntimeBridgeResponse,
    RuntimeDeliveryDisposition, RuntimeDeliveryReceipt, RuntimeError, RuntimeSendLiveResponse,
    SessionRuntime,
};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::mpsc;

use crate::{
    collaboration::CollaborationSessionHooks,
    dispatcher::{
        AutomaticWorktreeRelease, DEFINITELY_REJECTED_CODE, DEFINITELY_UNSENT_CODE, DispatchError,
        HostDispatcher, INVALID_PAYLOAD_CODE, OUTCOME_UNKNOWN_CODE, SessionAccessMemory,
        encode_legacy_result, runtime_dispatch_error,
    },
    payload,
    protocol::PushFrame,
    registry_access,
    registry_push::{RegistryPushCoordinator, SharedRegistryPushes, registry_push_frame},
    session_access::mark_permission_modes,
};

pub const ADOPT_ALL_OPERATION: &str = "iface:adoptAll";
pub const SEND_LIVE_OPERATION: &str = "iface:sendLive";
pub const IMPORT_PROGRESS_OPERATION: &str = "iface:importProgress";

/// The frozen session-management surface not already owned by the live-session
/// runtime dispatcher: thirteen public IPC operations and two legacy extras.
pub const SESSION_MANAGEMENT_OPERATIONS: [&str; 15] = [
    CHANNELS.list_session_snapshot,
    CHANNELS.list_archived_sessions,
    CHANNELS.list_provider_commands,
    CHANNELS.forget_session,
    CHANNELS.archive,
    CHANNELS.rename_session,
    CHANNELS.delete_session,
    CHANNELS.fork_session,
    CHANNELS.adopt_session,
    CHANNELS.set_engine_archived,
    CHANNELS.read_usage,
    CHANNELS.engine_ops,
    CHANNELS.search_sessions,
    ADOPT_ALL_OPERATION,
    SEND_LIVE_OPERATION,
];

/// Narrow injection seam for the frozen host adapter. Production delegates to
/// [`SessionRuntime`]; tests can prove request, result, certainty, and push
/// behavior without starting provider processes.
#[async_trait]
pub trait SessionManagementRuntimePort: Send + Sync + 'static {
    async fn handle_management_command(
        &self,
        command: RuntimeBridgeCommand,
        progress: Option<Arc<dyn ImportProgressSink>>,
    ) -> RuntimeBridgeResponse;

    /// Returns the current visible registry projection without refreshing a provider.
    ///
    /// # Errors
    ///
    /// Returns a durable registry access failure.
    fn visible_sessions(&self) -> Result<Vec<SessionSummary>, RuntimeError>;
}

/// Work the host does around a rename the owner asked for (`renameSession`).
/// The thread host holds a thread's title gate across the rename and, once
/// it happened, records the title as the owner's, so no generated title
/// lands on top of it.
#[async_trait]
pub trait RenameHooks: Send + Sync + 'static {
    /// Runs before the runtime renames `session_key`.
    async fn before_rename(&self, session_key: &str) -> Box<dyn RenameTicket>;
}

/// A rename [`RenameHooks::before_rename`] prepared for. Settled once the
/// rename's outcome is known.
pub trait RenameTicket: Send {
    /// `renamed` is false only when the rename definitely did not happen.
    fn settle(self: Box<Self>, renamed: bool);
}

/// Work the host does after the owner removed a session from Interface
/// (`deleteSession` or `forgetSession`, confirmed). The thread host releases
/// the session's thread worktree: removing a session, like archiving it, is
/// the owner saying they are done with it.
pub trait RemovalHooks: Send + Sync + 'static {
    fn session_removed(&self, session_key: &str);

    /// [`Self::session_removed`], knowing whether the session was deleted
    /// (its provider transcript too) or forgotten.
    fn session_removed_as(&self, session_key: &str, deleted: bool) {
        let _ = deleted;
        self.session_removed(session_key);
    }
}

/// Optional host-owned usage source for a provider that is not a chat runtime
/// adapter. Its failures are represented by an unavailable usage row so one
/// provider cannot make the whole usage read fail.
#[async_trait]
pub trait SupplementalUsageReader: Send + Sync + 'static {
    fn engine(&self) -> interface_contracts::engine::EngineId;
    async fn read_usage(&self) -> EngineUsage;
}

#[async_trait]
impl SessionManagementRuntimePort for SessionRuntime {
    async fn handle_management_command(
        &self,
        command: RuntimeBridgeCommand,
        progress: Option<Arc<dyn ImportProgressSink>>,
    ) -> RuntimeBridgeResponse {
        match command {
            RuntimeBridgeCommand::AdoptAll { adopt } => match self.adopt_all(adopt, progress).await
            {
                Ok(report) => RuntimeBridgeResponse::ImportReport(report),
                Err(error) => RuntimeBridgeResponse::Error(error),
            },
            command => self.handle_bridge_command(command).await,
        }
    }

    fn visible_sessions(&self) -> Result<Vec<SessionSummary>, RuntimeError> {
        registry_access::sorted_visible(self)
    }
}

/// Exact Electron-compatible adapter over the transport-neutral runtime
/// management boundary.
pub struct SessionManagementHostDispatcher<R> {
    runtime: Arc<R>,
    pushes: Option<mpsc::Sender<PushFrame>>,
    /// The host's shared registry coalescer; without it the frame is built
    /// and sent here.
    registry_pushes: Option<RegistryPushCoordinator>,
    collaboration: Option<Arc<dyn CollaborationSessionHooks>>,
    worktree_release: Option<Arc<dyn AutomaticWorktreeRelease>>,
    supplemental_usage: Vec<Arc<dyn SupplementalUsageReader>>,
    rename_hooks: Option<Arc<dyn RenameHooks>>,
    removal_hooks: Option<Arc<dyn RemovalHooks>>,
    /// Set by the first `listSessionSnapshot`, which pushes the cached
    /// registry before it waits on the provider listings.
    cached_registry_painted: AtomicBool,
    /// Protocol 68: the permission modes the host keeps, put on the
    /// listing's sessions.
    session_access: Option<Arc<dyn SessionAccessMemory>>,
}

impl<R> SessionManagementHostDispatcher<R> {
    #[must_use]
    pub const fn new(runtime: Arc<R>) -> Self {
        Self {
            runtime,
            pushes: None,
            registry_pushes: None,
            collaboration: None,
            worktree_release: None,
            supplemental_usage: Vec::new(),
            rename_hooks: None,
            removal_hooks: None,
            cached_registry_painted: AtomicBool::new(false),
            session_access: None,
        }
    }

    #[must_use]
    pub const fn with_pushes(runtime: Arc<R>, pushes: mpsc::Sender<PushFrame>) -> Self {
        Self {
            runtime,
            pushes: Some(pushes),
            registry_pushes: None,
            collaboration: None,
            worktree_release: None,
            supplemental_usage: Vec::new(),
            rename_hooks: None,
            removal_hooks: None,
            cached_registry_painted: AtomicBool::new(false),
            session_access: None,
        }
    }

    /// Protocol 68: the listing and the frames sent from here carry each
    /// session's permission mode as `memory` keeps it.
    #[must_use]
    pub fn with_session_access(mut self, memory: Arc<dyn SessionAccessMemory>) -> Self {
        self.session_access = Some(memory);
        self
    }

    /// Sends post-mutation registry frames through the host's shared
    /// coalescer, so the next scheduled round knows the registry was just sent.
    #[must_use]
    pub fn with_shared_registry_pushes(mut self, shared: SharedRegistryPushes) -> Self {
        self.registry_pushes = Some(shared.0);
        self
    }

    #[must_use]
    pub fn with_collaboration_hooks(
        mut self,
        collaboration: Arc<dyn CollaborationSessionHooks>,
    ) -> Self {
        self.collaboration = Some(collaboration);
        self
    }

    #[must_use]
    pub fn with_worktree_release(
        mut self,
        worktree_release: Arc<dyn AutomaticWorktreeRelease>,
    ) -> Self {
        self.worktree_release = Some(worktree_release);
        self
    }

    #[must_use]
    pub fn with_supplemental_usage(
        mut self,
        supplemental_usage: Vec<Arc<dyn SupplementalUsageReader>>,
    ) -> Self {
        self.supplemental_usage = supplemental_usage;
        self
    }

    #[must_use]
    pub fn with_removal_hooks(mut self, removal_hooks: Arc<dyn RemovalHooks>) -> Self {
        self.removal_hooks = Some(removal_hooks);
        self
    }

    #[must_use]
    pub fn with_rename_hooks(mut self, rename_hooks: Arc<dyn RenameHooks>) -> Self {
        self.rename_hooks = Some(rename_hooks);
        self
    }
}

impl<R> SessionManagementHostDispatcher<R>
where
    R: SessionManagementRuntimePort,
{
    fn import_progress_sink(&self) -> Option<Arc<dyn ImportProgressSink>> {
        let pushes = self.pushes.clone()?;
        Some(Arc::new(move |progress: ImportProgress| {
            let Ok(payload) = serde_json::to_value(progress) else {
                tracing::error!("failed to encode import progress");
                return;
            };
            let Ok(frame) = PushFrame::new(IMPORT_PROGRESS_OPERATION, payload) else {
                tracing::error!("import progress channel is not registered as a push");
                return;
            };
            if let Err(error) = pushes.try_send(frame) {
                tracing::warn!(%error, "could not enqueue transient import progress");
            }
        }))
    }

    /// The rename hooks' ticket for an owner's `renameSession`.
    async fn before_rename(&self, command: &RuntimeBridgeCommand) -> Option<Box<dyn RenameTicket>> {
        let (Some(hooks), RuntimeBridgeCommand::RenameSession { session_key, .. }) =
            (&self.rename_hooks, command)
        else {
            return None;
        };
        Some(hooks.before_rename(session_key).await)
    }

    async fn emit_registry_snapshot(&self) -> Result<(), DispatchError> {
        // A flush resolves once the frame is queued, so the response still
        // follows it, as the frozen bridge requires.
        if let Some(registry_pushes) = &self.registry_pushes {
            return if registry_pushes.flush().await {
                Ok(())
            } else {
                Err(registry_push_error())
            };
        }
        let Some(pushes) = &self.pushes else {
            return Ok(());
        };
        let mut sessions = self.runtime.visible_sessions().map_err(|error| {
            tracing::error!(kind = ?error.kind, "could not read the post-mutation registry");
            registry_push_error()
        })?;
        mark_permission_modes(&mut sessions, self.session_access.as_ref());
        let frame = registry_push_frame(&sessions).map_err(|error| {
            tracing::error!(code = %error.code, "could not construct the registry push");
            registry_push_error()
        })?;
        pushes.send(frame).await.map_err(|_| registry_push_error())
    }
}

#[async_trait]
impl<R> HostDispatcher for SessionManagementHostDispatcher<R>
where
    R: SessionManagementRuntimePort,
{
    async fn dispatch(&self, operation: &str, payload: Value) -> Result<Value, DispatchError> {
        if !SESSION_MANAGEMENT_OPERATIONS.contains(&operation) {
            return Err(DispatchError::new(
                "operation-not-owned",
                "That operation is not owned by the Rust session-management service.",
                false,
                true,
            ));
        }
        let command = decode_command(operation, payload)?;
        if let Some(collaboration) = &self.collaboration {
            match management_guard(&command) {
                ManagementGuard::None => {}
                ManagementGuard::Session {
                    session_key,
                    action,
                } => {
                    collaboration
                        .guard_session_mutation(session_key, action, false)
                        .await?;
                }
                ManagementGuard::Bulk { action } => {
                    collaboration.guard_bulk_mutation(action).await?;
                }
            }
        }
        // A cold first listing reads every provider store before it answers.
        // Paint the registry already in memory first (the same frozen push),
        // so the sidebar leaves its skeleton; the renderer prunes saved
        // layouts only from the listing response, never from this cache.
        if operation == CHANNELS.list_session_snapshot
            && !self.cached_registry_painted.swap(true, Ordering::AcqRel)
            && let Err(error) = self.emit_registry_snapshot().await
        {
            tracing::warn!(code = %error.code, "could not paint the cached registry");
        }
        let changed_session = management_session_key(&command).map(ToOwned::to_owned);
        let rename = self.before_rename(&command).await;
        let bulk_mutation = matches!(command, RuntimeBridgeCommand::AdoptAll { .. });
        let progress = bulk_mutation.then(|| self.import_progress_sink()).flatten();
        let supplemental_readers = matches!(command, RuntimeBridgeCommand::ReadUsage)
            .then(|| self.supplemental_usage.clone())
            .unwrap_or_default();
        let (response, supplemental) = if supplemental_readers.is_empty() {
            (
                self.runtime
                    .handle_management_command(command, progress)
                    .await,
                Vec::new(),
            )
        } else {
            let reads = supplemental_readers
                .iter()
                .map(|reader| async {
                    let engine = reader.engine();
                    let usage = reader.read_usage().await;
                    (engine, usage)
                })
                .collect::<Vec<_>>();
            let (response, supplemental) = tokio::join!(
                self.runtime.handle_management_command(command, progress),
                futures::future::join_all(reads),
            );
            (response, supplemental)
        };
        let mut response = if operation == CHANNELS.read_usage {
            supplement_usage_response(response, supplemental)
        } else {
            response
        };
        if let RuntimeBridgeResponse::SessionSnapshot(snapshot) = &mut response {
            mark_permission_modes(&mut snapshot.sessions, self.session_access.as_ref());
        }
        let result = legacy_result(operation, response);
        if let Some(rename) = rename {
            // An unknown outcome may have renamed it: the owner's title wins.
            rename.settle(result.as_ref().map_or_else(|e| !e.replay_safe, |_| true));
        }
        let result = result?;
        if mutates_registry(operation) {
            if let Some(collaboration) = &self.collaboration {
                if bulk_mutation {
                    for session in self
                        .runtime
                        .visible_sessions()
                        .map_err(runtime_dispatch_error)?
                    {
                        collaboration.observe_registry_change(&session_key(
                            &session.engine,
                            &session.session_id,
                        ))?;
                    }
                } else if let Some(session_key) = changed_session.as_deref() {
                    collaboration.observe_registry_change(session_key)?;
                }
            }
            self.emit_registry_snapshot().await?;
        }
        if let Some(session_key) = changed_session {
            self.after_removal(operation, session_key);
        }
        Ok(result)
    }
}

impl<R> SessionManagementHostDispatcher<R>
where
    R: SessionManagementRuntimePort,
{
    /// After a confirmed delete or forget: the removal hooks, and for a
    /// forget the managed worktree's release.
    fn after_removal(&self, operation: &str, session_key: String) {
        if operation != CHANNELS.forget_session && operation != CHANNELS.delete_session {
            return;
        }
        if let Some(removal_hooks) = &self.removal_hooks {
            removal_hooks.session_removed_as(&session_key, operation == CHANNELS.delete_session);
        }
        if operation == CHANNELS.forget_session
            && let Some(worktree_release) = &self.worktree_release
        {
            worktree_release.schedule_session_release(session_key);
        }
    }
}

fn supplement_usage_response(
    response: RuntimeBridgeResponse,
    supplemental: Vec<(EngineId, EngineUsage)>,
) -> RuntimeBridgeResponse {
    let RuntimeBridgeResponse::Usage(mut usage) = response else {
        return response;
    };
    let had_supplemental = !supplemental.is_empty();
    for (expected_engine, supplemental) in supplemental {
        if !usage
            .iter()
            .any(|current| current.engine == expected_engine)
        {
            if supplemental.engine == expected_engine {
                usage.push(supplemental);
            } else {
                tracing::error!(
                    expected = %expected_engine,
                    actual = %supplemental.engine,
                    "supplemental usage reader returned a different engine"
                );
            }
        }
    }
    if had_supplemental {
        usage.sort_by(|left, right| left.engine.as_str().cmp(right.engine.as_str()));
    }
    RuntimeBridgeResponse::Usage(usage)
}

enum ManagementGuard<'a> {
    None,
    Session {
        session_key: &'a str,
        action: &'static str,
    },
    Bulk {
        action: &'static str,
    },
}

fn management_guard(command: &RuntimeBridgeCommand) -> ManagementGuard<'_> {
    match command {
        RuntimeBridgeCommand::ForgetSession { session_key } => ManagementGuard::Session {
            session_key,
            action: "forget",
        },
        RuntimeBridgeCommand::ArchiveSession { session_key, .. } => ManagementGuard::Session {
            session_key,
            action: "archive",
        },
        RuntimeBridgeCommand::RenameSession { session_key, .. } => ManagementGuard::Session {
            session_key,
            action: "rename",
        },
        RuntimeBridgeCommand::DeleteSession { session_key } => ManagementGuard::Session {
            session_key,
            action: "delete",
        },
        RuntimeBridgeCommand::ForkSession { session_key } => ManagementGuard::Session {
            session_key,
            action: "fork",
        },
        RuntimeBridgeCommand::AdoptSession { session_key, .. } => ManagementGuard::Session {
            session_key,
            action: "import change",
        },
        RuntimeBridgeCommand::SetEngineArchived { session_key, .. } => ManagementGuard::Session {
            session_key,
            action: "provider archive",
        },
        RuntimeBridgeCommand::SendLive { session_key, .. } => ManagementGuard::Session {
            session_key,
            action: "live send",
        },
        RuntimeBridgeCommand::AdoptAll { .. } => ManagementGuard::Bulk {
            action: "import change",
        },
        _ => ManagementGuard::None,
    }
}

fn management_session_key(command: &RuntimeBridgeCommand) -> Option<&str> {
    match management_guard(command) {
        ManagementGuard::Session { session_key, .. } => Some(session_key),
        ManagementGuard::None | ManagementGuard::Bulk { .. } => None,
    }
}

fn legacy_result(operation: &str, response: RuntimeBridgeResponse) -> Result<Value, DispatchError> {
    if let RuntimeBridgeResponse::Error(error) = response {
        return Err(runtime_dispatch_error(error));
    }
    match (operation, response) {
        (operation, RuntimeBridgeResponse::SessionSnapshot(snapshot))
            if operation == CHANNELS.list_session_snapshot =>
        {
            encode_legacy_result(snapshot)
        }
        (operation, RuntimeBridgeResponse::ArchivedSessions(sessions))
            if operation == CHANNELS.list_archived_sessions =>
        {
            encode_legacy_result(sessions)
        }
        (operation, RuntimeBridgeResponse::ProviderCommands(commands))
            if operation == CHANNELS.list_provider_commands =>
        {
            encode_legacy_result(commands)
        }
        (operation, RuntimeBridgeResponse::Acknowledged) if unit_operation(operation) => {
            Ok(Value::Null)
        }
        (operation, RuntimeBridgeResponse::ForkedSession { session_key })
            if operation == CHANNELS.fork_session =>
        {
            Ok(serde_json::json!({ "key": session_key }))
        }
        (operation, RuntimeBridgeResponse::Usage(usage)) if operation == CHANNELS.read_usage => {
            encode_legacy_result(usage)
        }
        (operation, RuntimeBridgeResponse::EngineOps(ops)) if operation == CHANNELS.engine_ops => {
            encode_legacy_result(ops)
        }
        (operation, RuntimeBridgeResponse::SessionSearch(search))
            if operation == CHANNELS.search_sessions =>
        {
            encode_legacy_result(search)
        }
        (ADOPT_ALL_OPERATION, RuntimeBridgeResponse::ImportReport(report)) => {
            encode_legacy_result(report)
        }
        (
            SEND_LIVE_OPERATION,
            RuntimeBridgeResponse::SendLive(RuntimeSendLiveResponse::NotLive),
        ) => Ok(Value::Bool(false)),
        (
            SEND_LIVE_OPERATION,
            RuntimeBridgeResponse::SendLive(RuntimeSendLiveResponse::Delivery(receipt)),
        ) => delivery_boolean(receipt),
        _ => Err(response_mismatch(operation)),
    }
}

fn decode_command(operation: &str, payload: Value) -> Result<RuntimeBridgeCommand, DispatchError> {
    if operation == CHANNELS.list_session_snapshot {
        decode_empty(&payload)?;
        return Ok(RuntimeBridgeCommand::ListSessionSnapshot);
    }
    if operation == CHANNELS.list_archived_sessions {
        decode_empty(&payload)?;
        return Ok(RuntimeBridgeCommand::ListArchivedSessions);
    }
    if operation == CHANNELS.list_provider_commands {
        return decode_payload::<ProviderCommandScope>(payload)
            .map(|scope| RuntimeBridgeCommand::ListProviderCommands { scope });
    }
    if operation == CHANNELS.forget_session {
        return decode_payload::<SessionPayload>(payload).map(|payload| {
            RuntimeBridgeCommand::ForgetSession {
                session_key: payload.session_key,
            }
        });
    }
    if operation == CHANNELS.archive {
        return decode_payload::<ArchivePayload>(payload).map(|payload| {
            RuntimeBridgeCommand::ArchiveSession {
                session_key: payload.session_key,
                archived: payload.archived,
            }
        });
    }
    if operation == CHANNELS.rename_session {
        return decode_payload::<RenamePayload>(payload).map(|payload| {
            RuntimeBridgeCommand::RenameSession {
                session_key: payload.session_key,
                title: payload.title,
            }
        });
    }
    if operation == CHANNELS.delete_session {
        return decode_payload::<SessionPayload>(payload).map(|payload| {
            RuntimeBridgeCommand::DeleteSession {
                session_key: payload.session_key,
            }
        });
    }
    if operation == CHANNELS.fork_session {
        return decode_payload::<SessionPayload>(payload).map(|payload| {
            RuntimeBridgeCommand::ForkSession {
                session_key: payload.session_key,
            }
        });
    }
    if operation == CHANNELS.adopt_session {
        return decode_payload::<AdoptPayload>(payload).map(|payload| {
            RuntimeBridgeCommand::AdoptSession {
                session_key: payload.session_key,
                adopt: payload.adopt,
            }
        });
    }
    if operation == CHANNELS.set_engine_archived {
        return decode_payload::<ArchivePayload>(payload).map(|payload| {
            RuntimeBridgeCommand::SetEngineArchived {
                session_key: payload.session_key,
                archived: payload.archived,
            }
        });
    }
    if operation == CHANNELS.read_usage {
        decode_empty(&payload)?;
        return Ok(RuntimeBridgeCommand::ReadUsage);
    }
    if operation == CHANNELS.engine_ops {
        decode_empty(&payload)?;
        return Ok(RuntimeBridgeCommand::EngineOps);
    }
    if operation == CHANNELS.search_sessions {
        return decode_payload::<SearchPayload>(payload).map(|payload| {
            RuntimeBridgeCommand::SearchSessions {
                query: payload.query,
            }
        });
    }
    if operation == ADOPT_ALL_OPERATION {
        return decode_payload::<AdoptAllPayload>(payload).map(|payload| {
            RuntimeBridgeCommand::AdoptAll {
                adopt: payload.adopt,
            }
        });
    }
    if operation == SEND_LIVE_OPERATION {
        return decode_payload::<SendPayload>(payload).map(|payload| {
            RuntimeBridgeCommand::SendLive {
                session_key: payload.session_key,
                text: payload.text,
            }
        });
    }
    unreachable!("operation ownership checked before decoding")
}

fn decode_empty(payload: &Value) -> Result<(), DispatchError> {
    payload::require_empty(payload, invalid_payload)
}

fn decode_payload<T>(payload: Value) -> Result<T, DispatchError>
where
    T: serde::de::DeserializeOwned,
{
    payload::decode_object(payload, |_| invalid_payload())
}

fn invalid_payload() -> DispatchError {
    DispatchError::new(
        INVALID_PAYLOAD_CODE,
        "The request payload does not match the migrated operation schema.",
        false,
        true,
    )
}

fn delivery_boolean(receipt: RuntimeDeliveryReceipt) -> Result<Value, DispatchError> {
    if receipt.disposition == RuntimeDeliveryDisposition::Confirmed {
        return Ok(Value::Bool(true));
    }
    let replay_safe = receipt.replay_is_safe();
    let (message, retryable) = receipt.error.map_or_else(
        || {
            (
                "The provider effect did not complete with confirmed delivery.".to_owned(),
                false,
            )
        },
        |error| (error.message, error.retryable),
    );
    let code = match receipt.disposition {
        RuntimeDeliveryDisposition::Confirmed => unreachable!("handled above"),
        RuntimeDeliveryDisposition::DefinitelyUnsent => DEFINITELY_UNSENT_CODE,
        RuntimeDeliveryDisposition::DefinitelyRejected => DEFINITELY_REJECTED_CODE,
        RuntimeDeliveryDisposition::OutcomeUnknown => OUTCOME_UNKNOWN_CODE,
    };
    Err(DispatchError::new(code, message, retryable, replay_safe))
}

fn response_mismatch(operation: &str) -> DispatchError {
    tracing::error!(
        operation,
        "runtime returned a response for a different operation"
    );
    DispatchError::new(
        "runtime-response-mismatch",
        "The Rust runtime returned an incompatible response.",
        false,
        false,
    )
}

fn registry_push_error() -> DispatchError {
    DispatchError::new(
        OUTCOME_UNKNOWN_CODE,
        "The session changed, but its registry update could not reach the desktop.",
        false,
        false,
    )
}

fn unit_operation(operation: &str) -> bool {
    [
        CHANNELS.forget_session,
        CHANNELS.archive,
        CHANNELS.rename_session,
        CHANNELS.delete_session,
        CHANNELS.adopt_session,
        CHANNELS.set_engine_archived,
    ]
    .contains(&operation)
}

fn mutates_registry(operation: &str) -> bool {
    unit_operation(operation)
        || operation == CHANNELS.fork_session
        || operation == ADOPT_ALL_OPERATION
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SessionPayload {
    session_key: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ArchivePayload {
    session_key: String,
    archived: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RenamePayload {
    session_key: String,
    title: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AdoptPayload {
    session_key: String,
    adopt: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SearchPayload {
    query: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AdoptAllPayload {
    adopt: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SendPayload {
    session_key: String,
    text: String,
}
