//! Frozen 0.1.169 owner/client transport for shared orchestration state.
//!
//! A process connects to `host.sock` before it considers opening either
//! authoritative store. Only the process holding `host.lock` may construct the
//! stores and expose this one-request-per-connection control socket.

use std::{
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use interface_contracts::{
    ipc::CHANNELS,
    orchestration_control::{
        ORCHESTRATION_CONTROL_MAX_LINE_BYTES, ORCHESTRATION_CONTROL_VERSION,
        OrchestrationAgentPageV1, OrchestrationControlErrorBody, OrchestrationControlErrorCode,
        OrchestrationControlMethod, OrchestrationControlRequest, OrchestrationControlResponse,
        OrchestrationControlResponsePayload,
    },
};
use interface_orchestration::{
    AgentPageInput, bounded_utf8, is_orchestration_worker, project_orchestration_agent_for_control,
    project_orchestration_agents_for_control,
};
use interface_runtime::SessionRuntime;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::{Mutex as AsyncMutex, mpsc, watch},
    task::JoinHandle,
    time::timeout,
};
use uuid::Uuid;

use crate::{
    collaboration::DesktopCollaborationProjection,
    dispatcher::{DispatchError, HostDispatcher, SharedRegistryPushes},
    instance_lock::{CliInstanceLock, acquire_cli_instance_lock},
    orchestration::{
        OrchestrationHostDispatcher, OrchestrationRuntimePort, UnavailableCoordination,
    },
    protocol::PushFrame,
    worktree::WorktreeService,
};

const OWNER_DIRECTORY_MODE: u32 = 0o700;
const OWNER_FILE_MODE: u32 = 0o600;

const CONNECT_ATTEMPTS: usize = 20;
const CONNECT_RETRY: Duration = Duration::from_millis(25);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const PING_TIMEOUT: Duration = Duration::from_secs(2);
const ACCEPT_RETRY_BACKOFF: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrchestrationHostPaths {
    pub root: PathBuf,
    pub owner_uid: u32,
    pub lock: PathBuf,
    pub socket: PathBuf,
    pub state: PathBuf,
}

#[must_use]
pub fn resolve_paths(root: &Path) -> OrchestrationHostPaths {
    let owner_uid = std::fs::symlink_metadata(root).map_or(0, |metadata| metadata.uid());
    OrchestrationHostPaths {
        root: root.to_owned(),
        owner_uid,
        lock: root.join("host.lock"),
        socket: root.join("host.sock"),
        state: root.join("state"),
    }
}

#[derive(Debug)]
pub enum PreparedOrchestrationHost {
    Client {
        paths: OrchestrationHostPaths,
        runtime_epoch: String,
    },
    Owner {
        paths: OrchestrationHostPaths,
        runtime_epoch: String,
        provider_effects_v1: bool,
        lock: CliInstanceLock,
    },
}

impl PreparedOrchestrationHost {
    #[must_use]
    pub const fn owns_stores(&self) -> bool {
        matches!(self, Self::Owner { .. })
    }

    #[must_use]
    pub fn runtime_epoch(&self) -> &str {
        match self {
            Self::Client { runtime_epoch, .. } | Self::Owner { runtime_epoch, .. } => runtime_epoch,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HostPing {
    runtime_epoch: String,
    #[serde(default)]
    capabilities: Option<HostCapabilities>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HostCapabilities {
    version: u8,
    provider_effects_v1: bool,
}

/// Connects to a compatible live owner or acquires the sole writer lease.
/// No authoritative store is opened before this returns `Owner`.
///
/// # Errors
///
/// Refuses unsafe roots, unavailable compatible owners, and any writer-lease
/// state whose ownership cannot be proven.
pub async fn prepare_interactive(root: &Path) -> Result<PreparedOrchestrationHost, String> {
    prepare(root, true).await
}

/// Connects to any compatible owner or acquires a provider-free one-shot
/// writer lease. A later interactive process will wait for this short owner to
/// finish instead of attaching to a host that cannot execute provider effects.
///
/// # Errors
///
/// Refuses unsafe roots, unavailable owners, and uncertain writer ownership.
pub async fn prepare_one_shot(root: &Path) -> Result<PreparedOrchestrationHost, String> {
    prepare(root, false).await
}

async fn prepare(
    root: &Path,
    require_provider_effects: bool,
) -> Result<PreparedOrchestrationHost, String> {
    ensure_owner_root(root)?;
    let paths = resolve_paths(root);
    if let Some(ping) = connect_existing(&paths).await
        && (!require_provider_effects || provider_effects(&ping))
    {
        return Ok(PreparedOrchestrationHost::Client {
            paths,
            runtime_epoch: ping.runtime_epoch,
        });
    }
    let mut last_error = None;
    for attempt in 0..=CONNECT_ATTEMPTS {
        match acquire_cli_instance_lock(&paths.lock) {
            Ok(lock) => {
                return Ok(PreparedOrchestrationHost::Owner {
                    paths,
                    runtime_epoch: Uuid::new_v4().to_string(),
                    provider_effects_v1: require_provider_effects,
                    lock,
                });
            }
            Err(error) => last_error = Some(error),
        }
        if attempt == CONNECT_ATTEMPTS {
            break;
        }
        tokio::time::sleep(CONNECT_RETRY).await;
        if let Some(ping) = connect_existing(&paths).await
            && (!require_provider_effects || provider_effects(&ping))
        {
            return Ok(PreparedOrchestrationHost::Client {
                paths,
                runtime_epoch: ping.runtime_epoch,
            });
        }
    }
    Err(last_error.unwrap_or_else(|| {
        if require_provider_effects {
            "Another provider-free orchestration host is still finishing; try again shortly.".into()
        } else {
            "Another orchestration host owns durable state but is not accepting requests yet."
                .into()
        }
    }))
}

fn provider_effects(ping: &HostPing) -> bool {
    ping.capabilities
        .as_ref()
        .is_some_and(|capabilities| capabilities.version == 1 && capabilities.provider_effects_v1)
}

fn ensure_owner_root(root: &Path) -> Result<(), String> {
    if !root.is_absolute() {
        return Err("Orchestration host root must be absolute.".into());
    }
    std::fs::create_dir_all(root).map_err(|error| error.to_string())?;
    let metadata = std::fs::symlink_metadata(root).map_err(|error| error.to_string())?;
    let parent_uid = root
        .parent()
        .and_then(|parent| std::fs::symlink_metadata(parent).ok())
        .map(|parent| parent.uid())
        .ok_or_else(|| "Orchestration host root parent is unavailable.".to_owned())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || metadata.uid() != parent_uid {
        return Err("Orchestration host root must be an owner-controlled directory.".into());
    }
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(OWNER_DIRECTORY_MODE))
        .map_err(|error| error.to_string())?;
    let metadata = std::fs::symlink_metadata(root).map_err(|error| error.to_string())?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != parent_uid
        || metadata.mode() & 0o777 != OWNER_DIRECTORY_MODE
    {
        return Err("Orchestration host root did not become owner-private.".into());
    }
    Ok(())
}

async fn connect_existing(paths: &OrchestrationHostPaths) -> Option<HostPing> {
    invoke_socket::<_, HostPing>(
        &paths.socket,
        OrchestrationControlMethod::HostPing,
        &json!({}),
        PING_TIMEOUT,
    )
    .await
    .ok()
}

/// What answers `thread.schedule.trigger` on `host.sock` (T3 section 7):
/// the desktop host's Threads.
#[async_trait]
pub trait ScheduleTrigger: Send + Sync {
    /// Runs the schedule named or numbered `target` now; answers what
    /// became of it.
    async fn trigger(&self, target: &str) -> Result<Value, String>;
}

/// `iface trigger`: asks the Interface that owns `host.sock` under `root` to
/// run a scheduled prompt now. Never becomes an owner itself.
///
/// # Errors
/// No running Interface, one that serves no schedules, or its refusal.
pub async fn trigger_thread_schedule(root: &Path, target: &str) -> Result<Value, String> {
    let paths = resolve_paths(root);
    if connect_existing(&paths).await.is_none() {
        return Err(
            "Interface is not running. Scheduled prompts run only while Interface is open.".into(),
        );
    }
    invoke_socket(
        &paths.socket,
        OrchestrationControlMethod::ThreadScheduleTrigger,
        &json!({"target": target}),
        REQUEST_TIMEOUT,
    )
    .await
}

#[derive(Clone)]
pub struct OrchestrationHostControl {
    inner: Arc<ControlInner>,
}

enum ControlInner {
    Unavailable {
        reason: String,
        lock: Mutex<Option<CliInstanceLock>>,
    },
    Client {
        paths: OrchestrationHostPaths,
        closed: AtomicBool,
    },
    Owner {
        local: Arc<LocalHost>,
        server: AsyncMutex<Option<RunningServer>>,
        lock: Mutex<Option<CliInstanceLock>>,
        closed: AtomicBool,
    },
}

impl OrchestrationHostControl {
    /// Frozen safe-degradation owner used when shared orchestration could not
    /// open. Ordinary provider and desktop operations remain available while
    /// orchestration/Project Plans calls receive one explicit refusal.
    #[must_use]
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(ControlInner::Unavailable {
                reason: safe_message(&reason.into()),
                lock: Mutex::new(None),
            }),
        }
    }

    fn unavailable_holding_owner(reason: impl Into<String>, lock: CliInstanceLock) -> Self {
        Self {
            inner: Arc::new(ControlInner::Unavailable {
                reason: safe_message(&reason.into()),
                // The runtime may still own the already-open shared stores.
                // Retain the exact election lease so degradation cannot admit
                // a second writer during this process.
                lock: Mutex::new(Some(lock)),
            }),
        }
    }

    /// Completes an already prepared client connection without constructing a
    /// local runtime or opening any authoritative store.
    ///
    /// # Errors
    ///
    /// Refuses an owner preparation because that path requires a fully
    /// constructed runtime and its exact writer-owned stores.
    pub fn connect_client(prepared: PreparedOrchestrationHost) -> Result<Self, String> {
        let PreparedOrchestrationHost::Client { paths, .. } = prepared else {
            return Err("An orchestration owner requires its local runtime stores.".into());
        };
        Ok(Self {
            inner: Arc::new(ControlInner::Client {
                paths,
                closed: AtomicBool::new(false),
            }),
        })
    }

    /// Completes a prepared owner/client connection after runtime construction.
    ///
    /// # Errors
    ///
    /// Returns an error if owner stores are absent, recovery fails, or the
    /// owner-private socket cannot be bound and verified.
    pub async fn start(
        prepared: PreparedOrchestrationHost,
        runtime: &SessionRuntime,
    ) -> Result<Self, String> {
        let runtime_port: Arc<dyn OrchestrationRuntimePort> = Arc::new(runtime.clone());
        Self::start_with_runtime_port(
            prepared,
            runtime,
            runtime_port,
            None,
            None,
            None,
            None,
            false,
        )
        .await
    }

    /// Completes the owner graph with the process-live transcript projection
    /// used by the frozen collaboration/orchestration bridge.
    ///
    /// # Errors
    ///
    /// Returns an error if owner stores are absent, recovery fails, or the
    /// owner-private socket cannot be bound and verified.
    /// `registry_pushes` is the host's shared whole-registry coalescer; the
    /// orchestration owner schedules onto it and never closes it.
    pub async fn start_with_services_and_projection(
        prepared: PreparedOrchestrationHost,
        runtime: &SessionRuntime,
        projection: Arc<DesktopCollaborationProjection>,
        worktrees: Option<Arc<dyn WorktreeService>>,
        pushes: Option<mpsc::Sender<PushFrame>>,
        registry_pushes: Option<SharedRegistryPushes>,
    ) -> Result<Self, String> {
        let runtime_port: Arc<dyn OrchestrationRuntimePort> = Arc::new(runtime.clone());
        Self::start_with_runtime_port(
            prepared,
            runtime,
            runtime_port,
            Some(projection),
            worktrees,
            pushes,
            registry_pushes,
            true,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn start_with_runtime_port(
        prepared: PreparedOrchestrationHost,
        runtime: &SessionRuntime,
        runtime_port: Arc<dyn OrchestrationRuntimePort>,
        projection: Option<Arc<DesktopCollaborationProjection>>,
        worktrees: Option<Arc<dyn WorktreeService>>,
        pushes: Option<mpsc::Sender<PushFrame>>,
        registry_pushes: Option<SharedRegistryPushes>,
        degrade_owner_failures: bool,
    ) -> Result<Self, String> {
        match prepared {
            client @ PreparedOrchestrationHost::Client { .. } => Self::connect_client(client),
            PreparedOrchestrationHost::Owner {
                paths,
                runtime_epoch,
                provider_effects_v1,
                lock,
            } => {
                let Some(store) = runtime.services().orchestration() else {
                    let reason = "The orchestration owner was not opened.";
                    return if degrade_owner_failures {
                        Ok(Self::unavailable_holding_owner(reason, lock))
                    } else {
                        Err(reason.into())
                    };
                };
                let mut dispatcher = OrchestrationHostDispatcher::new(
                    Arc::clone(&store),
                    runtime_port,
                    Arc::new(UnavailableCoordination),
                    runtime_epoch.clone(),
                );
                if let Some(worktrees) = worktrees {
                    dispatcher = dispatcher.with_worktrees(worktrees);
                }
                if let Some(projection) = projection {
                    dispatcher = dispatcher.with_projection(projection);
                }
                if let Some(pushes) = pushes {
                    dispatcher = dispatcher.with_pushes(pushes);
                }
                if let Some(registry_pushes) = registry_pushes {
                    dispatcher = dispatcher.with_shared_registry_pushes(registry_pushes);
                }
                let dispatcher = Arc::new(dispatcher);
                if let Err(error) = dispatcher.recover().await {
                    return if degrade_owner_failures {
                        Ok(Self::unavailable_holding_owner(error.message, lock))
                    } else {
                        Err(error.message)
                    };
                }
                let local = Arc::new(LocalHost {
                    dispatcher,
                    store,
                    runtime_epoch,
                    provider_effects_v1,
                    change_sequence: AtomicU64::new(0),
                    schedule_trigger: std::sync::OnceLock::new(),
                });
                let server = match RunningServer::start(
                    &paths.socket,
                    paths.owner_uid,
                    Arc::clone(&local),
                ) {
                    Ok(server) => server,
                    Err(error) => {
                        return if degrade_owner_failures {
                            Ok(Self::unavailable_holding_owner(error, lock))
                        } else {
                            Err(error)
                        };
                    }
                };
                Ok(Self {
                    inner: Arc::new(ControlInner::Owner {
                        local,
                        server: AsyncMutex::new(Some(server)),
                        lock: Mutex::new(Some(lock)),
                        closed: AtomicBool::new(false),
                    }),
                })
            }
        }
    }

    /// Serves `thread.schedule.trigger` from `trigger` when this process
    /// owns `host.sock`. Attaching twice keeps the first.
    pub fn attach_schedule_trigger(&self, trigger: Arc<dyn ScheduleTrigger>) {
        if let ControlInner::Owner { local, .. } = self.inner.as_ref() {
            let _ = local.schedule_trigger.set(trigger);
        }
    }

    /// Invokes one bounded control method on the current owner.
    ///
    /// # Errors
    ///
    /// Returns transport, protocol, host-dispatch, or response-decoding errors.
    pub async fn invoke<Request, Response>(
        &self,
        method: OrchestrationControlMethod,
        request: &Request,
    ) -> Result<Response, String>
    where
        Request: Serialize + ?Sized,
        Response: DeserializeOwned,
    {
        match self.inner.as_ref() {
            ControlInner::Unavailable { reason, .. } => Err(format!(
                "Shared orchestration is unavailable for this run: {reason}"
            )),
            ControlInner::Client { paths, closed } => {
                if closed.load(Ordering::Acquire) {
                    return Err("The orchestration host connection is closed.".into());
                }
                if method == OrchestrationControlMethod::AdversarialReviewStart {
                    return Err(
                        "Start review in the orchestration owner window. This window cannot control or project owner-launched sessions."
                            .into(),
                    );
                }
                invoke_socket(&paths.socket, method, request, REQUEST_TIMEOUT).await
            }
            ControlInner::Owner { local, closed, .. } => {
                if closed.load(Ordering::Acquire) {
                    return Err("The orchestration host connection is closed.".into());
                }
                let payload = serde_json::to_value(request).map_err(|error| error.to_string())?;
                let result = local.dispatch(method, payload).await?;
                serde_json::from_value(result).map_err(|error| error.to_string())
            }
        }
    }

    /// Every orchestration worker's session key, read from the whole agent
    /// list rather than the bounded control snapshot (64 agents). The owner
    /// reads its own store; a client pages the owner's `agent.list` to the end.
    ///
    /// # Errors
    ///
    /// Orchestration is unavailable or closed, or a page could not be read.
    pub async fn worker_session_keys(&self) -> Result<Vec<String>, String> {
        if let ControlInner::Owner { local, closed, .. } = self.inner.as_ref() {
            if closed.load(Ordering::Acquire) {
                return Err("The orchestration host connection is closed.".into());
            }
            return local
                .dispatcher
                .worker_session_keys()
                .map_err(|error| error.message);
        }
        let mut keys = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let mut request = json!({});
            if let Some(cursor) = &after {
                request["afterAgentId"] = json!(cursor);
            }
            let page: OrchestrationAgentPageV1 = self
                .invoke(OrchestrationControlMethod::AgentList, &request)
                .await?;
            keys.extend(
                page.agents
                    .into_iter()
                    .filter(|agent| {
                        is_orchestration_worker(
                            agent.parent_agent_id.as_deref(),
                            agent.role.as_deref(),
                        )
                    })
                    .filter_map(|agent| agent.session_key),
            );
            if !page.has_more {
                return Ok(keys);
            }
            match page.next_after_agent_id {
                Some(next) if after.as_ref() != Some(&next) => after = Some(next),
                _ => return Err("The orchestration agent pages did not advance.".into()),
            }
        }
    }

    /// Invokes the control method corresponding to a frozen desktop channel.
    ///
    /// # Errors
    ///
    /// Returns an error for unmigrated channels or any invocation failure.
    pub async fn invoke_channel<Request, Response>(
        &self,
        channel: &str,
        request: &Request,
    ) -> Result<Response, String>
    where
        Request: Serialize + ?Sized,
        Response: DeserializeOwned,
    {
        let method = channel_method(channel)
            .ok_or_else(|| "This orchestration operation has not migrated.".to_owned())?;
        self.invoke(method, request).await
    }

    /// Closes the server/dispatcher and releases the exact owner lock inode.
    ///
    /// # Errors
    ///
    /// Returns the first socket, store, or lock teardown failure.
    pub async fn close(&self) -> Result<(), String> {
        match self.inner.as_ref() {
            ControlInner::Unavailable { lock, .. } => {
                let owned = lock
                    .lock()
                    .map_err(|_| "The orchestration host lock was poisoned.".to_owned())?
                    .take();
                match owned {
                    Some(owned) => owned.release(),
                    None => Ok(()),
                }
            }
            ControlInner::Client { closed, .. } => {
                closed.store(true, Ordering::Release);
                Ok(())
            }
            ControlInner::Owner {
                local,
                server,
                lock,
                closed,
            } => {
                if closed.swap(true, Ordering::AcqRel) {
                    return Ok(());
                }
                let mut failure = None;
                if let Some(server) = server.lock().await.take()
                    && let Err(error) = server.close().await
                {
                    failure = Some(error);
                }
                if let Err(error) = local.dispatcher.close().await {
                    failure.get_or_insert(error.message);
                }
                let owned = lock
                    .lock()
                    .map_err(|_| "The orchestration host lock was poisoned.".to_owned())?
                    .take();
                if let Some(owned) = owned
                    && let Err(error) = owned.release()
                {
                    failure.get_or_insert(error);
                }
                failure.map_or(Ok(()), Err)
            }
        }
    }
}

#[async_trait]
impl HostDispatcher for OrchestrationHostControl {
    async fn dispatch(&self, operation: &str, payload: Value) -> Result<Value, DispatchError> {
        let method = channel_method(operation).ok_or_else(|| {
            DispatchError::new(
                "operation-not-owned",
                "That operation belongs to another Rust host service.",
                false,
                true,
            )
        })?;
        let result = match self.inner.as_ref() {
            // The owner window is served in process, so its payload and result
            // skip the serde round trip `invoke` needs for typed callers.
            ControlInner::Owner { local, closed, .. } => {
                if closed.load(Ordering::Acquire) {
                    Err("The orchestration host connection is closed.".to_owned())
                } else {
                    local.dispatch(method, payload).await
                }
            }
            _ => self.invoke::<_, Value>(method, &payload).await,
        };
        result.map_err(|error| control_dispatch_error(&error, read_only(method)))
    }
}

fn control_dispatch_error(message: &str, replay_safe: bool) -> DispatchError {
    DispatchError::new(
        "orchestration-control-failed",
        safe_message(message),
        replay_safe,
        replay_safe,
    )
}

struct LocalHost {
    dispatcher: Arc<OrchestrationHostDispatcher>,
    store: Arc<Mutex<interface_orchestration::OrchestrationStore>>,
    runtime_epoch: String,
    provider_effects_v1: bool,
    change_sequence: AtomicU64,
    /// The desktop host's Threads, for `thread.schedule.trigger`.
    schedule_trigger: std::sync::OnceLock<Arc<dyn ScheduleTrigger>>,
}

impl LocalHost {
    async fn dispatch(
        &self,
        method: OrchestrationControlMethod,
        payload: Value,
    ) -> Result<Value, String> {
        if method == OrchestrationControlMethod::HostPing {
            let revision = self
                .store
                .lock()
                .map_err(|_| "The orchestration store lock was poisoned.".to_owned())?
                .revision();
            return Ok(json!({
                "version": 1,
                "runtimeEpoch": self.runtime_epoch,
                "revision": revision,
                "changeSequence": self.change_sequence.load(Ordering::Acquire),
                "capabilities": {
                    "version": 1,
                    "providerEffectsV1": self.provider_effects_v1
                }
            }));
        }
        if read_only(method) {
            return self.dispatch_inner(method, payload).await;
        }
        let result = self.dispatch_inner(method, payload).await?;
        self.change_sequence.fetch_add(1, Ordering::AcqRel);
        Ok(result)
    }

    async fn dispatch_inner(
        &self,
        method: OrchestrationControlMethod,
        payload: Value,
    ) -> Result<Value, String> {
        match method {
            OrchestrationControlMethod::AgentList => self.agent_list(payload),
            OrchestrationControlMethod::AgentGet => self.agent_get(payload),
            OrchestrationControlMethod::ThreadScheduleTrigger => {
                let input: ScheduleTriggerRequest =
                    serde_json::from_value(payload).map_err(invalid_request)?;
                let trigger = self.schedule_trigger.get().ok_or_else(|| {
                    "This Interface does not run scheduled prompts (open the Interface app)."
                        .to_owned()
                })?;
                trigger.trigger(&input.target).await
            }
            OrchestrationControlMethod::HostPing => unreachable!("handled before dispatch"),
            other => {
                let channel = method_channel(other)
                    .ok_or_else(|| "This orchestration operation has not migrated.".to_owned())?;
                self.dispatcher
                    .dispatch(channel, payload)
                    .await
                    .map_err(|error| error.message)
            }
        }
    }

    fn agent_list(&self, payload: Value) -> Result<Value, String> {
        let input: AgentListRequest = serde_json::from_value(payload).map_err(invalid_request)?;
        let snapshot = self.dispatcher.snapshot().map_err(|error| error.message)?;
        let page = project_orchestration_agents_for_control(
            &snapshot.agents,
            &AgentPageInput {
                after_agent_id: input.after_agent_id,
                limit: input.limit.unwrap_or(32),
            },
        )
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Agent page cursor not found.".to_owned())?;
        serde_json::to_value(page).map_err(|error| error.to_string())
    }

    fn agent_get(&self, payload: Value) -> Result<Value, String> {
        let input: AgentGetRequest = serde_json::from_value(payload).map_err(invalid_request)?;
        let snapshot = self.dispatcher.snapshot().map_err(|error| error.message)?;
        let agent = snapshot
            .agents
            .iter()
            .find(|agent| agent.agent_id == input.agent_id)
            .ok_or_else(|| "Agent not found.".to_owned())?;
        serde_json::to_value(project_orchestration_agent_for_control(agent))
            .map_err(|error| error.to_string())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentListRequest {
    #[serde(default)]
    after_agent_id: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ScheduleTriggerRequest {
    target: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentGetRequest {
    agent_id: String,
}

#[allow(clippy::needless_pass_by_value)]
fn invalid_request(error: serde_json::Error) -> String {
    format!("Invalid orchestration request: {error}")
}

fn read_only(method: OrchestrationControlMethod) -> bool {
    matches!(
        method,
        OrchestrationControlMethod::HostPing
            | OrchestrationControlMethod::OrchestrationSnapshot
            | OrchestrationControlMethod::AgentGet
            | OrchestrationControlMethod::AgentList
            | OrchestrationControlMethod::AgentTranscript
            | OrchestrationControlMethod::AgentInbox
            | OrchestrationControlMethod::CoordinationStatus
            | OrchestrationControlMethod::OrchestratorModels
            | OrchestrationControlMethod::EventsSince
    )
}

fn channel_method(channel: &str) -> Option<OrchestrationControlMethod> {
    Some(match channel {
        value if value == CHANNELS.get_orchestration_snapshot => {
            OrchestrationControlMethod::OrchestrationSnapshot
        }
        value if value == CHANNELS.read_orchestration_timeline => {
            OrchestrationControlMethod::EventsSince
        }
        value if value == CHANNELS.create_orchestration_agent => {
            OrchestrationControlMethod::AgentCreate
        }
        value if value == CHANNELS.send_orchestration_message => {
            OrchestrationControlMethod::AgentSend
        }
        value if value == CHANNELS.reply_orchestration_message => {
            OrchestrationControlMethod::AgentReply
        }
        value if value == CHANNELS.read_orchestration_transcript => {
            OrchestrationControlMethod::AgentTranscript
        }
        value if value == CHANNELS.read_orchestration_inbox => {
            OrchestrationControlMethod::AgentInbox
        }
        value if value == CHANNELS.stop_orchestration_agent => {
            OrchestrationControlMethod::AgentStop
        }
        value if value == CHANNELS.steer_orchestration_agent => {
            OrchestrationControlMethod::AgentSteer
        }
        value if value == CHANNELS.claim_orchestration_role => {
            OrchestrationControlMethod::RoleClaim
        }
        value if value == CHANNELS.relinquish_orchestration_role => {
            OrchestrationControlMethod::RoleRelinquish
        }
        value if value == CHANNELS.escrow_orchestration_worktree => {
            OrchestrationControlMethod::WorktreeEscrow
        }
        value if value == CHANNELS.set_orchestrator_policy => {
            OrchestrationControlMethod::OrchestratorPolicySet
        }
        value if value == CHANNELS.clear_orchestrator_policy => {
            OrchestrationControlMethod::OrchestratorPolicyClear
        }
        value if value == CHANNELS.bind_orchestration_parent => {
            OrchestrationControlMethod::AgentBindParent
        }
        value if value == CHANNELS.register_orchestration_session => {
            OrchestrationControlMethod::AgentRegisterSession
        }
        value if value == CHANNELS.submit_coordinated_worker => {
            OrchestrationControlMethod::WorkerSubmit
        }
        value if value == CHANNELS.integrate_coordinated_worker => {
            OrchestrationControlMethod::WorkerIntegrate
        }
        value if value == CHANNELS.abandon_coordinated_task => {
            OrchestrationControlMethod::WorkerAbandon
        }
        value if value == CHANNELS.ack_coordinated_obligation => {
            OrchestrationControlMethod::WorkerAck
        }
        value if value == CHANNELS.get_coordination_status => {
            OrchestrationControlMethod::CoordinationStatus
        }
        value if value == CHANNELS.get_orchestrator_models => {
            OrchestrationControlMethod::OrchestratorModels
        }
        value if value == CHANNELS.start_adversarial_review => {
            OrchestrationControlMethod::AdversarialReviewStart
        }
        _ => return None,
    })
}

fn method_channel(method: OrchestrationControlMethod) -> Option<&'static str> {
    Some(match method {
        OrchestrationControlMethod::OrchestrationSnapshot => CHANNELS.get_orchestration_snapshot,
        OrchestrationControlMethod::EventsSince => CHANNELS.read_orchestration_timeline,
        OrchestrationControlMethod::AgentCreate => CHANNELS.create_orchestration_agent,
        OrchestrationControlMethod::AgentSend => CHANNELS.send_orchestration_message,
        OrchestrationControlMethod::AgentReply => CHANNELS.reply_orchestration_message,
        OrchestrationControlMethod::AgentTranscript => CHANNELS.read_orchestration_transcript,
        OrchestrationControlMethod::AgentInbox => CHANNELS.read_orchestration_inbox,
        OrchestrationControlMethod::AgentStop => CHANNELS.stop_orchestration_agent,
        OrchestrationControlMethod::AgentSteer => CHANNELS.steer_orchestration_agent,
        OrchestrationControlMethod::RoleClaim => CHANNELS.claim_orchestration_role,
        OrchestrationControlMethod::RoleRelinquish => CHANNELS.relinquish_orchestration_role,
        OrchestrationControlMethod::WorktreeEscrow => CHANNELS.escrow_orchestration_worktree,
        OrchestrationControlMethod::OrchestratorPolicySet => CHANNELS.set_orchestrator_policy,
        OrchestrationControlMethod::OrchestratorPolicyClear => CHANNELS.clear_orchestrator_policy,
        OrchestrationControlMethod::AgentBindParent => CHANNELS.bind_orchestration_parent,
        OrchestrationControlMethod::AgentRegisterSession => CHANNELS.register_orchestration_session,
        OrchestrationControlMethod::WorkerSubmit => CHANNELS.submit_coordinated_worker,
        OrchestrationControlMethod::WorkerIntegrate => CHANNELS.integrate_coordinated_worker,
        OrchestrationControlMethod::WorkerAbandon => CHANNELS.abandon_coordinated_task,
        OrchestrationControlMethod::WorkerAck => CHANNELS.ack_coordinated_obligation,
        OrchestrationControlMethod::CoordinationStatus => CHANNELS.get_coordination_status,
        OrchestrationControlMethod::OrchestratorModels => CHANNELS.get_orchestrator_models,
        OrchestrationControlMethod::AdversarialReviewStart => CHANNELS.start_adversarial_review,
        OrchestrationControlMethod::HostPing
        | OrchestrationControlMethod::OrchestrationApply
        | OrchestrationControlMethod::AgentGet
        | OrchestrationControlMethod::AgentList
        | OrchestrationControlMethod::ThreadScheduleTrigger => return None,
    })
}

struct RunningServer {
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
    path: PathBuf,
    identity: (u64, u64),
}

impl RunningServer {
    fn start(path: &Path, owner_uid: u32, local: Arc<LocalHost>) -> Result<Self, String> {
        if let Ok(metadata) = std::fs::symlink_metadata(path) {
            if !metadata.file_type().is_socket()
                || metadata.file_type().is_symlink()
                || metadata.uid() != owner_uid
            {
                return Err("Existing orchestration socket path is unsafe.".into());
            }
            std::fs::remove_file(path).map_err(|error| error.to_string())?;
        }
        let listener = UnixListener::bind(path).map_err(|error| error.to_string())?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(OWNER_FILE_MODE))
            .map_err(|error| error.to_string())?;
        let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
        if !metadata.file_type().is_socket()
            || metadata.file_type().is_symlink()
            || metadata.uid() != owner_uid
            || metadata.mode() & 0o777 != OWNER_FILE_MODE
        {
            let _ = std::fs::remove_file(path);
            return Err("Orchestration socket did not become owner-private.".into());
        }
        let identity = (metadata.dev(), metadata.ino());
        let (shutdown, mut receiver) = watch::channel(false);
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    changed = receiver.changed() => {
                        if changed.is_err() || *receiver.borrow() {
                            break;
                        }
                    }
                    accepted = listener.accept() => {
                        match accepted {
                            Ok((stream, _)) => {
                                if !stream
                                    .peer_cred()
                                    .is_ok_and(|cred| {
                                        interface_common::unix::effective_uid().ok()
                                            == Some(cred.uid())
                                    })
                                {
                                    continue;
                                }
                                let local = Arc::clone(&local);
                                tokio::spawn(async move { handle_connection(stream, local).await; });
                            }
                            Err(error) => {
                                // A transient accept failure must not retire the
                                // owner socket; only the shutdown watch ends it.
                                tracing::warn!(%error, "orchestration control accept failed");
                                tokio::time::sleep(ACCEPT_RETRY_BACKOFF).await;
                            }
                        }
                    }
                }
            }
        });
        Ok(Self {
            shutdown,
            task,
            path: path.to_owned(),
            identity,
        })
    }

    async fn close(self) -> Result<(), String> {
        let _ = self.shutdown.send(true);
        let _ = self.task.await;
        match std::fs::symlink_metadata(&self.path) {
            Ok(metadata)
                if metadata.file_type().is_socket()
                    && (metadata.dev(), metadata.ino()) == self.identity =>
            {
                std::fs::remove_file(&self.path).map_err(|error| error.to_string())
            }
            Ok(_) => {
                Err("The orchestration socket changed before shutdown; it was not removed.".into())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }
}

async fn handle_connection(stream: UnixStream, local: Arc<LocalHost>) {
    let mut stream = BufReader::new(stream);
    let mut line = Vec::new();
    // The oversize check below still rejects the request, but the read itself
    // has to stay bounded so a writer that never sends a newline cannot make
    // the host allocate without limit. The client's own deadline bounds how
    // long a silent connection may hold its task and descriptor; nothing has
    // been dispatched yet, so giving up here affects no provider effect.
    let result = timeout(
        REQUEST_TIMEOUT,
        (&mut stream)
            .take((ORCHESTRATION_CONTROL_MAX_LINE_BYTES + 2) as u64)
            .read_until(b'\n', &mut line),
    )
    .await
    .unwrap_or_else(|_| Err(std::io::ErrorKind::TimedOut.into()));
    let response = match result {
        Ok(0) => response_error(
            "invalid-request",
            OrchestrationControlErrorCode::InvalidRequest,
            "The orchestration host request was empty.",
        ),
        Ok(_) if line.len() > ORCHESTRATION_CONTROL_MAX_LINE_BYTES + 1 => response_error(
            "invalid-request",
            OrchestrationControlErrorCode::InvalidRequest,
            "Orchestration host request exceeds the 1 MiB limit.",
        ),
        Ok(_) => match serde_json::from_slice::<OrchestrationControlRequest>(&line) {
            Ok(request) if request.version == ORCHESTRATION_CONTROL_VERSION => {
                let id = request.id.clone();
                match local
                    .dispatch(request.method, Value::Object(request.params))
                    .await
                {
                    Ok(result) => OrchestrationControlResponse {
                        version: ORCHESTRATION_CONTROL_VERSION,
                        id,
                        payload: OrchestrationControlResponsePayload::Result { result },
                    },
                    Err(error) => response_error(
                        &id,
                        OrchestrationControlErrorCode::Failed,
                        &safe_message(&error),
                    ),
                }
            }
            Ok(request) => response_error(
                &request.id,
                OrchestrationControlErrorCode::InvalidRequest,
                "The orchestration control version is unsupported.",
            ),
            Err(_) => response_error(
                "invalid-request",
                OrchestrationControlErrorCode::InvalidRequest,
                "Interface received an invalid orchestration request.",
            ),
        },
        Err(_) => response_error(
            "invalid-request",
            OrchestrationControlErrorCode::Unavailable,
            "The local orchestration host is not available.",
        ),
    };
    let mut encoded = serde_json::to_vec(&response).unwrap_or_else(|_| {
        br#"{"version":1,"id":"invalid-request","error":{"code":"failed","message":"The orchestration host could not encode its response."}}"#.to_vec()
    });
    encoded.push(b'\n');
    let stream = stream.get_mut();
    let _ = stream.write_all(&encoded).await;
    let _ = stream.shutdown().await;
}

fn response_error(
    id: &str,
    code: OrchestrationControlErrorCode,
    message: &str,
) -> OrchestrationControlResponse {
    OrchestrationControlResponse {
        version: ORCHESTRATION_CONTROL_VERSION,
        id: id.into(),
        payload: OrchestrationControlResponsePayload::Error {
            error: OrchestrationControlErrorBody {
                code,
                message: safe_message(message),
                data: None,
            },
        },
    }
}

async fn invoke_socket<Request, Response>(
    socket: &Path,
    method: OrchestrationControlMethod,
    params: &Request,
    deadline: Duration,
) -> Result<Response, String>
where
    Request: Serialize + ?Sized,
    Response: DeserializeOwned,
{
    let params = serde_json::to_value(params).map_err(|error| error.to_string())?;
    let Value::Object(params) = params else {
        return Err("Orchestration request parameters must be an object.".into());
    };
    let id = Uuid::new_v4().to_string();
    let request = OrchestrationControlRequest {
        version: ORCHESTRATION_CONTROL_VERSION,
        id: id.clone(),
        method,
        params,
    };
    timeout(deadline, async {
        let mut stream = UnixStream::connect(socket)
            .await
            .map_err(|_| "The local orchestration host is not available.".to_owned())?;
        if !stream
            .peer_cred()
            .is_ok_and(|cred| interface_common::unix::effective_uid().ok() == Some(cred.uid()))
        {
            return Err("The local orchestration host failed owner verification.".into());
        }
        let mut encoded = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
        encoded.push(b'\n');
        if encoded.len() > ORCHESTRATION_CONTROL_MAX_LINE_BYTES + 1 {
            return Err("Orchestration host request exceeds the 1 MiB limit.".into());
        }
        stream
            .write_all(&encoded)
            .await
            .map_err(|_| "The local orchestration host is not available.".to_owned())?;
        stream
            .shutdown()
            .await
            .map_err(|_| "The local orchestration host is not available.".to_owned())?;
        let mut response = Vec::new();
        stream
            .take((ORCHESTRATION_CONTROL_MAX_LINE_BYTES + 2) as u64)
            .read_to_end(&mut response)
            .await
            .map_err(|_| "The local orchestration host is not available.".to_owned())?;
        if response.is_empty() {
            return Err("The orchestration host closed before replying.".into());
        }
        if response.len() > ORCHESTRATION_CONTROL_MAX_LINE_BYTES + 1 {
            return Err("Orchestration host response exceeds the 1 MiB limit.".into());
        }
        let response: OrchestrationControlResponse = serde_json::from_slice(&response)
            .map_err(|_| "Interface received an invalid orchestration response.".to_owned())?;
        if response.id != id {
            return Err("Orchestration host response id did not match the request.".into());
        }
        match response.payload {
            OrchestrationControlResponsePayload::Result { result } => {
                serde_json::from_value(result).map_err(|error| error.to_string())
            }
            OrchestrationControlResponsePayload::Error { error } => {
                Err(safe_message(&error.message))
            }
        }
    })
    .await
    .map_err(|_| "The orchestration host did not reply before the local deadline.".to_owned())?
}

fn safe_message(value: &str) -> String {
    let mut value = value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    if value.len() > 16_384 {
        value = bounded_utf8(&value, 16_384);
    }
    if value.is_empty() {
        "Orchestration host operation failed.".into()
    } else {
        value
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use interface_contracts::orchestration::{
        AgentNodeState, AgentNodeV1, AgentScopeV1, OrchestrationActorKind, OrchestrationActorV1,
        OrchestrationMutation, OrchestrationMutationMetaV1,
    };
    use interface_runtime::{EngineHistoryStore, HistoryLimits, RuntimeServices};

    fn private_root() -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("temporary root");
        std::fs::set_permissions(
            root.path(),
            std::fs::Permissions::from_mode(OWNER_DIRECTORY_MODE),
        )
        .expect("private root");
        root
    }

    fn runtime_for_owner(root: &Path, runtime_epoch: &str) -> interface_runtime::SessionRuntime {
        runtime_for_owner_with_agents(root, runtime_epoch, &[])
    }

    /// An orchestration root agent `root` (an orchestrator) and one finished
    /// worker under it per entry of `workers`, each bound to the session key
    /// `codex#<id>` and created in order.
    pub(crate) fn finished_workers(workers: &[&str]) -> Vec<(String, Option<&'static str>)> {
        std::iter::once(("root".to_owned(), None))
            .chain(workers.iter().map(|id| ((*id).to_owned(), Some("root"))))
            .collect()
    }

    /// Like `runtime_for_owner`, with `agents` (id, parent) registered first:
    /// a parentless agent is an orchestrator, every other a completed worker.
    fn runtime_for_owner_with_agents(
        root: &Path,
        runtime_epoch: &str,
        agents: &[(String, Option<&'static str>)],
    ) -> interface_runtime::SessionRuntime {
        let runtime_root = root.join("runtime");
        std::fs::create_dir(&runtime_root).expect("runtime root");
        std::fs::set_permissions(
            &runtime_root,
            std::fs::Permissions::from_mode(OWNER_DIRECTORY_MODE),
        )
        .expect("runtime mode");
        let sessions = interface_registry::SessionRegistry::open(&runtime_root).expect("registry");
        let history_root = runtime_root.join("history");
        std::fs::create_dir(&history_root).expect("history root");
        std::fs::set_permissions(
            &history_root,
            std::fs::Permissions::from_mode(OWNER_DIRECTORY_MODE),
        )
        .expect("history mode");
        let history =
            EngineHistoryStore::open(&history_root, HistoryLimits::default()).expect("history");
        let orchestration = interface_orchestration::OrchestrationStore::open(
            root,
            "transport-test",
            runtime_epoch,
            "2026-01-01T00:00:00.000Z",
        )
        .expect("orchestration");
        let mut orchestration = orchestration;
        for (index, (id, parent)) in agents.iter().enumerate() {
            let at = format!(
                "2026-01-01T00:{:02}:{:02}.000Z",
                (index + 1) / 60,
                (index + 1) % 60
            );
            let agent = AgentNodeV1 {
                version: 1,
                agent_id: id.clone(),
                name: None,
                engine_id: "codex".into(),
                session_key: Some(format!("codex#{id}")),
                provider_session_id: None,
                runtime_epoch: runtime_epoch.into(),
                parent_agent_id: parent.map(str::to_owned),
                role: Some(
                    if parent.is_some() {
                        "worker"
                    } else {
                        "orchestrator"
                    }
                    .into(),
                ),
                depth: u64::from(parent.is_some()),
                state: if parent.is_some() {
                    AgentNodeState::Completed
                } else {
                    AgentNodeState::Ready
                },
                capabilities: Vec::new(),
                scope: AgentScopeV1 {
                    canonical_cwd: root.to_string_lossy().into_owned(),
                    canonical_project_root: None,
                    access_mode: "project".into(),
                    scope_digest: "a".repeat(64),
                    worktree_lease_id: None,
                },
                created_at: at.clone(),
                updated_at: at.clone(),
            };
            let revision = orchestration.revision();
            orchestration
                .apply(
                    revision,
                    OrchestrationMutation::RegisterAgent {
                        agent,
                        meta: OrchestrationMutationMetaV1 {
                            event_id: format!("event-{id}"),
                            at,
                            actor: OrchestrationActorV1 {
                                kind: OrchestrationActorKind::Host,
                                actor_id: "transport-test".into(),
                            },
                            summary: "registered".into(),
                        },
                    },
                )
                .expect("register agent");
        }
        interface_runtime::SessionRuntime::builder(
            RuntimeServices::new(sessions, history).with_orchestration(orchestration),
        )
        .build()
        .expect("runtime")
    }

    #[tokio::test]
    async fn unavailable_control_refuses_only_shared_operations_and_closes_cleanly() {
        let control = OrchestrationHostControl::unavailable("injected startup failure");
        let error = control
            .dispatch(CHANNELS.get_orchestration_snapshot, json!({}))
            .await
            .expect_err("unavailable orchestration");
        assert_eq!(error.code, "orchestration-control-failed");
        assert!(error.message.contains("injected startup failure"));
        control.close().await.expect("unavailable control close");
    }

    #[tokio::test]
    async fn degraded_owner_retains_the_election_lease_until_close() {
        let temporary = private_root();
        let shared = temporary.path().join("Interface");
        let prepared = prepare_interactive(&shared).await.expect("owner lease");
        assert!(prepared.owns_stores());

        let runtime_root = shared.join("runtime-without-shared-stores");
        std::fs::create_dir(&runtime_root).expect("runtime root");
        std::fs::set_permissions(
            &runtime_root,
            std::fs::Permissions::from_mode(OWNER_DIRECTORY_MODE),
        )
        .expect("runtime mode");
        let sessions = interface_registry::SessionRegistry::open(&runtime_root).expect("registry");
        let history_root = runtime_root.join("history");
        std::fs::create_dir(&history_root).expect("history root");
        std::fs::set_permissions(
            &history_root,
            std::fs::Permissions::from_mode(OWNER_DIRECTORY_MODE),
        )
        .expect("history mode");
        let history =
            EngineHistoryStore::open(&history_root, HistoryLimits::default()).expect("history");
        let runtime =
            interface_runtime::SessionRuntime::builder(RuntimeServices::new(sessions, history))
                .build()
                .expect("runtime without shared stores");

        let degraded = OrchestrationHostControl::start_with_services_and_projection(
            prepared,
            &runtime,
            Arc::new(DesktopCollaborationProjection::new()),
            None,
            None,
            None,
        )
        .await
        .expect("degraded orchestration control");
        let error = degraded
            .dispatch(CHANNELS.get_orchestration_snapshot, json!({}))
            .await
            .expect_err("degraded shared operation");
        assert_eq!(error.code, "orchestration-control-failed");
        assert!(
            prepare_interactive(&shared).await.is_err(),
            "degradation must not admit a second shared-state writer"
        );

        degraded.close().await.expect("close degraded owner");
        assert!(
            prepare_interactive(&shared)
                .await
                .expect("owner lease after degraded close")
                .owns_stores()
        );
    }

    /// An owner over `agents` (see `runtime_for_owner_with_agents`) in a new
    /// private root. Keep the directory alive as long as the owner.
    pub(crate) async fn owner_with_agents(
        agents: &[(String, Option<&'static str>)],
    ) -> (OrchestrationHostControl, tempfile::TempDir) {
        let temporary = private_root();
        let shared = temporary.path().join("Interface");
        let prepared = prepare_interactive(&shared).await.expect("owner lease");
        let runtime = runtime_for_owner_with_agents(&shared, prepared.runtime_epoch(), agents);
        let owner = OrchestrationHostControl::start(prepared, &runtime)
            .await
            .expect("owner host");
        (owner, temporary)
    }

    /// Past the control snapshot's 64 agents, the owner and a client (which
    /// pages `agent.list`, 32 a page) both still name every worker.
    #[tokio::test]
    async fn owner_and_client_name_every_worker_past_the_snapshot_bound() {
        let ids = (1..=69)
            .map(|index| format!("w{index:02}"))
            .collect::<Vec<_>>();
        let ids = ids.iter().map(String::as_str).collect::<Vec<_>>();
        let (owner, temporary) = owner_with_agents(&finished_workers(&ids)).await;
        let expected = ids
            .iter()
            .map(|id| format!("codex#{id}"))
            .collect::<Vec<_>>();

        let snapshot = owner
            .dispatch(CHANNELS.get_orchestration_snapshot, json!({}))
            .await
            .expect("snapshot");
        let named = snapshot["agents"].as_array().expect("agents");
        assert_eq!(named.len(), 64, "the control snapshot is bounded");
        assert!(!named.iter().any(|agent| agent["sessionKey"] == "codex#w01"));
        assert_eq!(
            owner.worker_session_keys().await.expect("owner keys"),
            expected
        );

        let client = OrchestrationHostControl::connect_client(
            prepare_interactive(&temporary.path().join("Interface"))
                .await
                .expect("client connection"),
        )
        .expect("client host");
        assert_eq!(
            client.worker_session_keys().await.expect("client keys"),
            expected
        );
        client.close().await.expect("close client");
        assert!(client.worker_session_keys().await.is_err());
        owner.close().await.expect("close owner");
        assert!(owner.worker_session_keys().await.is_err());
        assert!(
            OrchestrationHostControl::unavailable("off")
                .worker_session_keys()
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn second_process_connects_without_opening_authoritative_stores() {
        let temporary = private_root();
        let shared = temporary.path().join("Interface");
        let prepared = prepare_interactive(&shared).await.expect("owner lease");
        assert!(prepared.owns_stores());
        let runtime = runtime_for_owner(&shared, prepared.runtime_epoch());
        let owner = OrchestrationHostControl::start(prepared, &runtime)
            .await
            .expect("owner host");

        let socket = shared.join("host.sock");
        let metadata = std::fs::symlink_metadata(&socket).expect("socket metadata");
        assert!(metadata.file_type().is_socket());
        assert_eq!(metadata.mode() & 0o777, OWNER_FILE_MODE);

        let prepared_client = prepare_interactive(&shared)
            .await
            .expect("client connection");
        assert!(!prepared_client.owns_stores());
        let client = OrchestrationHostControl::start(prepared_client, &runtime)
            .await
            .expect("client host");
        client.close().await.expect("close client");
        assert!(socket.exists(), "a client must not remove the owner socket");

        owner.close().await.expect("close owner");
        assert!(!socket.exists());
        assert!(!shared.join("host.lock").exists());
    }

    #[tokio::test]
    async fn owner_dispatches_adversarial_review_channel_through_the_local_runtime() {
        assert_eq!(
            channel_method(CHANNELS.start_adversarial_review),
            Some(OrchestrationControlMethod::AdversarialReviewStart)
        );
        assert_eq!(
            method_channel(OrchestrationControlMethod::AdversarialReviewStart),
            Some(CHANNELS.start_adversarial_review)
        );

        let temporary = private_root();
        let shared = temporary.path().join("Interface");
        let prepared = prepare_interactive(&shared).await.expect("owner lease");
        let runtime = runtime_for_owner(&shared, prepared.runtime_epoch());
        let owner = OrchestrationHostControl::start(prepared, &runtime)
            .await
            .expect("owner host");

        let error = owner
            .dispatch(
                CHANNELS.start_adversarial_review,
                json!({
                    "builderEngineId": "codex",
                    "reviewerEngineId": "gemini",
                    "canonicalCwd": shared,
                    "objective": "Prove the owner route without contacting a provider.",
                    "clientRequestId": "owner-route-test",
                }),
            )
            .await
            .expect_err("unsupported pair should be rejected by the owner dispatcher");
        assert_eq!(error.code, "orchestration-control-failed");
        assert!(error.message.contains("cross-model Claude/Codex pair"));

        owner.close().await.expect("close owner");
    }

    #[tokio::test]
    async fn client_refuses_adversarial_review_before_opening_the_owner_socket() {
        let temporary = private_root();
        let paths = resolve_paths(temporary.path());
        let client = OrchestrationHostControl::connect_client(PreparedOrchestrationHost::Client {
            paths,
            runtime_epoch: "client-test-epoch".into(),
        })
        .expect("client control");

        let error = client
            .dispatch(CHANNELS.start_adversarial_review, json!({}))
            .await
            .expect_err("client must fail before attempting the absent owner socket");
        assert_eq!(error.code, "orchestration-control-failed");
        assert!(
            error
                .message
                .contains("Start review in the orchestration owner window")
        );
        assert!(!error.replay_safe);

        client.close().await.expect("close client");
    }

    #[tokio::test]
    async fn provider_free_one_shot_owner_is_not_an_interactive_provider_host() {
        let temporary = private_root();
        let shared = temporary.path().join("Interface");
        let prepared = prepare_one_shot(&shared).await.expect("one-shot lease");
        let runtime = runtime_for_owner(&shared, prepared.runtime_epoch());
        let owner = OrchestrationHostControl::start(prepared, &runtime)
            .await
            .expect("one-shot owner");

        let ping = connect_existing(&resolve_paths(&shared))
            .await
            .expect("one-shot ping");
        assert!(!provider_effects(&ping));
        let prepared_client = prepare_one_shot(&shared).await.expect("one-shot client");
        assert!(!prepared_client.owns_stores());
        OrchestrationHostControl::connect_client(prepared_client)
            .expect("client without runtime")
            .close()
            .await
            .expect("close client");
        assert!(prepare_interactive(&shared).await.is_err());

        owner.close().await.expect("close owner");
        assert!(
            prepare_interactive(&shared)
                .await
                .expect("interactive lease after handoff")
                .owns_stores()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_silent_client_is_answered_and_released_after_the_request_timeout() {
        let temporary = private_root();
        let shared = temporary.path().join("Interface");
        let prepared = prepare_interactive(&shared).await.expect("owner lease");
        let runtime = runtime_for_owner(&shared, prepared.runtime_epoch());
        let owner = OrchestrationHostControl::start(prepared, &runtime)
            .await
            .expect("owner host");
        let mut stream = UnixStream::connect(shared.join("host.sock"))
            .await
            .expect("connect");
        let connected = tokio::time::Instant::now();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.expect("response");
        assert!(connected.elapsed() >= REQUEST_TIMEOUT);
        let response: OrchestrationControlResponse =
            serde_json::from_slice(&response).expect("control response");
        let OrchestrationControlResponsePayload::Error { error } = response.payload else {
            panic!("a silent client must be refused");
        };
        assert_eq!(error.code, OrchestrationControlErrorCode::Unavailable);
        owner.close().await.expect("close owner");
    }

    #[tokio::test]
    async fn oversized_request_is_bounded_before_dispatch() {
        let temporary = private_root();
        let shared = temporary.path().join("Interface");
        let prepared = prepare_interactive(&shared).await.expect("owner lease");
        let runtime = runtime_for_owner(&shared, prepared.runtime_epoch());
        let owner = OrchestrationHostControl::start(prepared, &runtime)
            .await
            .expect("owner host");
        let mut stream = UnixStream::connect(shared.join("host.sock"))
            .await
            .expect("connect");
        stream
            .write_all(&vec![b'x'; ORCHESTRATION_CONTROL_MAX_LINE_BYTES + 2])
            .await
            .expect("oversized write");
        stream.write_all(b"\n").await.expect("newline");
        stream.shutdown().await.expect("shutdown");
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.expect("response");
        let response: OrchestrationControlResponse =
            serde_json::from_slice(&response).expect("control response");
        let OrchestrationControlResponsePayload::Error { error } = response.payload else {
            panic!("oversized request must be refused");
        };
        assert_eq!(error.code, OrchestrationControlErrorCode::InvalidRequest);
        assert!(error.message.contains("1 MiB"));
        owner.close().await.expect("close owner");
    }
}
