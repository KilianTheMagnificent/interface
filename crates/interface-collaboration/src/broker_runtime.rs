//! Host-owned lifetime for collaboration MCP listeners and provider bindings.

use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    io,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use interface_common::sync::lock;
use interface_contracts::engine::{
    CollaborationBindingError, CollaborationDepthFenceEntrant, CollaborationLease,
    CollaborationMcpSessionBinding, CollaborationMcpSessionFactory, CollaborationMcpSessionRequest,
    CollaborationMcpStdioDescriptor, CollaborationProviderBindings, CollaborationSourceRegistrar,
    EngineId, SessionKey,
};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

use crate::{
    broker::{
        BrokerDepthFence, BrokerError, BrokerRuntime as AuthorityCore, BrokerSessionBinding,
        BrokerSourceRegistration, CollaborationModelAuthority,
    },
    mcp::{
        CollaborationMcpAccessRequest, CollaborationMcpIntent, CollaborationMcpRequestIngress,
        CollaborationMcpSocket, CollaborationMcpSocketOptions, CollaborationModelIngress,
        FileIdentity, IngressCancellation, McpIngressError, unlink_matching_socket,
    },
    state::{CollaborationEngineId, CollaborationErrorCode},
};

const DEFAULT_NC_COMMAND: &str = "/usr/bin/nc";
/// How long the stale-socket sweep waits for a socket to answer before it
/// leaves the file alone.
const SWEEP_CONNECT_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Error)]
pub enum CollaborationBrokerRuntimeError {
    #[error("collaboration broker root is invalid")]
    InvalidRoot,
    #[error("collaboration broker command is unavailable")]
    CommandUnavailable,
    #[error("collaboration broker socket identity generation failed closed")]
    InvalidSocketIdentity,
    #[error("collaboration broker transport is unavailable")]
    TransportUnavailable,
    #[error("collaboration broker is stopping")]
    Stopping,
}

pub struct CollaborationBrokerRuntimeOptions {
    pub root: PathBuf,
    pub model_ingress: Arc<dyn CollaborationModelIngress>,
    pub command: PathBuf,
}

impl CollaborationBrokerRuntimeOptions {
    #[must_use]
    pub fn frozen(root: PathBuf, model_ingress: Arc<dyn CollaborationModelIngress>) -> Self {
        Self {
            root,
            model_ingress,
            command: PathBuf::from(DEFAULT_NC_COMMAND),
        }
    }
}

pub struct CollaborationBrokerRuntime {
    inner: Arc<BrokerInner>,
    bindings: CollaborationProviderBindings,
}

impl CollaborationBrokerRuntime {
    /// Starts the broker for `options.root`. The caller holds that root's
    /// collaboration journal writer lease, so no other broker binds there.
    pub async fn start(
        options: CollaborationBrokerRuntimeOptions,
    ) -> Result<Self, CollaborationBrokerRuntimeError> {
        let root = validate_root(&options.root)?;
        validate_command(&options.command)?;
        sweep_dead_sockets(&root.join("sockets")).await;
        let codex_capability = AuthorityCore::generate_capability();
        let core = AuthorityCore::new(&codex_capability)
            .map_err(|_| CollaborationBrokerRuntimeError::TransportUnavailable)?;
        let inner = Arc::new(BrokerInner {
            root,
            command: options.command,
            core: Mutex::new(core),
            model_ingress: options.model_ingress,
            servers: Mutex::new(BTreeMap::new()),
            socket_ids: Mutex::new(BTreeSet::new()),
            next_server: AtomicU64::new(1),
            accepting: AtomicBool::new(true),
        });
        let codex = inner
            .listen_bound(
                CollaborationEngineId::Codex,
                codex_capability,
                next_socket_id(),
            )
            .await?;
        let codex_descriptor = codex.descriptor.clone();
        let factory: Arc<dyn CollaborationMcpSessionFactory> = Arc::new(SessionFactory {
            inner: Arc::clone(&inner),
        });
        let registrar: Arc<dyn CollaborationSourceRegistrar> = Arc::new(CodexRegistrar {
            inner: Arc::clone(&inner),
        });
        let fence: Arc<dyn CollaborationDepthFenceEntrant> = Arc::new(CodexFenceEntrant {
            inner: Arc::clone(&inner),
        });
        Ok(Self {
            inner,
            bindings: CollaborationProviderBindings {
                session_factory: factory,
                codex_descriptor,
                register_codex_source: registrar,
                enter_codex_depth_fence: fence,
            },
        })
    }

    #[must_use]
    pub fn provider_bindings(&self) -> CollaborationProviderBindings {
        self.bindings.clone()
    }

    #[must_use]
    pub fn resolve_model_authority(
        &self,
        origin_connection_id: &str,
    ) -> Option<CollaborationModelAuthority> {
        lock(&self.inner.core).resolve_model_authority(origin_connection_id)
    }

    /// Revokes every capability synchronously, before the returned future can
    /// reach its first await. This mirrors the frozen JS `stop()` ordering.
    ///
    /// Each listener then drains for a bounded budget only. In-flight host
    /// calls are never cancelled to meet it — a call that may already have
    /// committed runs to completion detached — but listener and socket
    /// teardown always finishes.
    #[must_use]
    pub fn stop(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<(), CollaborationBrokerRuntimeError>> + Send>> {
        self.inner.quiesce();
        let inner = Arc::clone(&self.inner);
        Box::pin(async move { inner.stop_listeners().await })
    }

    pub async fn drain(&self) -> Result<(), CollaborationBrokerRuntimeError> {
        let servers = lock(&self.inner.servers)
            .values()
            .map(|server| Arc::clone(&server.socket))
            .collect::<Vec<_>>();
        for server in servers {
            server.drain().await;
        }
        Ok(())
    }
}

struct BrokerInner {
    root: PathBuf,
    command: PathBuf,
    core: Mutex<AuthorityCore>,
    model_ingress: Arc<dyn CollaborationModelIngress>,
    servers: Mutex<BTreeMap<u64, Arc<BoundServer>>>,
    socket_ids: Mutex<BTreeSet<String>>,
    next_server: AtomicU64,
    accepting: AtomicBool,
}

struct BoundServer {
    identifier: u64,
    descriptor: CollaborationMcpStdioDescriptor,
    socket: Arc<CollaborationMcpSocket>,
}

impl BrokerInner {
    fn assert_accepting(&self) -> Result<(), CollaborationBrokerRuntimeError> {
        if self.accepting.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(CollaborationBrokerRuntimeError::Stopping)
        }
    }

    async fn listen_bound(
        self: &Arc<Self>,
        engine: CollaborationEngineId,
        capability: String,
        socket_id: String,
    ) -> Result<Arc<BoundServer>, CollaborationBrokerRuntimeError> {
        self.assert_accepting()?;
        validate_socket_id(&socket_id)?;
        if !lock(&self.socket_ids).insert(socket_id.clone()) {
            return Err(CollaborationBrokerRuntimeError::InvalidSocketIdentity);
        }
        let socket_path = self.root.join("sockets").join(format!("{socket_id}.sock"));
        let ingress: Arc<dyn CollaborationMcpRequestIngress> = Arc::new(BrokerIngress {
            inner: Arc::clone(self),
        });
        let options =
            CollaborationMcpSocketOptions::frozen(socket_path, engine, capability, ingress);
        let socket = CollaborationMcpSocket::listen(options)
            .await
            .map_err(|_| CollaborationBrokerRuntimeError::TransportUnavailable)?;
        if !self.accepting.load(Ordering::Acquire) {
            let _ = socket.stop().await;
            return Err(CollaborationBrokerRuntimeError::Stopping);
        }
        let descriptor = CollaborationMcpStdioDescriptor {
            name: "iface_collaboration".into(),
            command: self.command.to_string_lossy().into_owned(),
            args: vec![
                "-U".into(),
                socket.socket_path().to_string_lossy().into_owned(),
            ],
            env: Vec::<Value>::new(),
        };
        descriptor
            .validate()
            .map_err(|_| CollaborationBrokerRuntimeError::TransportUnavailable)?;
        let identifier = self.next_server.fetch_add(1, Ordering::AcqRel);
        let bound = Arc::new(BoundServer {
            identifier,
            descriptor,
            socket,
        });
        lock(&self.servers).insert(identifier, Arc::clone(&bound));
        Ok(bound)
    }

    fn quiesce(&self) {
        if !self.accepting.swap(false, Ordering::AcqRel) {
            return;
        }
        lock(&self.core).stop();
    }

    async fn stop_listeners(&self) -> Result<(), CollaborationBrokerRuntimeError> {
        let servers = lock(&self.servers)
            .iter()
            .rev()
            .map(|(_, server)| Arc::clone(server))
            .collect::<Vec<_>>();
        let mut first_error = None;
        for server in servers {
            if server.socket.stop().await.is_err() && first_error.is_none() {
                first_error = Some(CollaborationBrokerRuntimeError::TransportUnavailable);
            }
            lock(&self.servers).remove(&server.identifier);
        }
        first_error.map_or(Ok(()), Err)
    }

    async fn dispose_session_binding(
        &self,
        token: BrokerSessionBinding,
        server: &BoundServer,
    ) -> Result<(), CollaborationBindingError> {
        lock(&self.core)
            .dispose_session_binding(token)
            .map_err(binding_error)?;
        let stopped = server.socket.stop().await;
        lock(&self.servers).remove(&server.identifier);
        stopped.map_err(|_| CollaborationBindingError::unavailable())
    }
}

struct BrokerIngress {
    inner: Arc<BrokerInner>,
}

impl BrokerIngress {
    fn authority(&self, origin: &str) -> Result<CollaborationModelAuthority, McpIngressError> {
        lock(&self.inner.core)
            .resolve_model_authority(origin)
            .ok_or_else(|| McpIngressError::new(CollaborationErrorCode::BrokerUnavailable))
    }
}

#[async_trait]
impl CollaborationMcpRequestIngress for BrokerIngress {
    async fn submit(
        &self,
        origin_connection_id: &str,
        intent: CollaborationMcpIntent,
        cancellation: IngressCancellation,
    ) -> Result<crate::state::CollaborationReceiptV1, McpIngressError> {
        let authority = self.authority(origin_connection_id)?;
        self.inner
            .model_ingress
            .submit(intent, authority, cancellation)
            .await
    }

    async fn status(
        &self,
        origin_connection_id: &str,
        request: CollaborationMcpAccessRequest,
        cancellation: IngressCancellation,
    ) -> Result<crate::state::CollaborationReceiptV1, McpIngressError> {
        let authority = self.authority(origin_connection_id)?;
        self.inner
            .model_ingress
            .status(request, authority, cancellation)
            .await
    }

    async fn cancel(
        &self,
        origin_connection_id: &str,
        request: CollaborationMcpAccessRequest,
        cancellation: IngressCancellation,
    ) -> Result<crate::state::CollaborationReceiptV1, McpIngressError> {
        let authority = self.authority(origin_connection_id)?;
        self.inner
            .model_ingress
            .cancel(request, authority, cancellation)
            .await
    }
}

struct SessionFactory {
    inner: Arc<BrokerInner>,
}

#[async_trait]
impl CollaborationMcpSessionFactory for SessionFactory {
    async fn create(
        &self,
        request: CollaborationMcpSessionRequest,
    ) -> Result<Arc<dyn CollaborationMcpSessionBinding>, CollaborationBindingError> {
        self.inner
            .assert_accepting()
            .map_err(|_| CollaborationBindingError::inactive())?;
        let engine = session_engine(&request.engine)?;
        if request.provisional_session_id.is_empty()
            || request.provisional_session_id.len() > crate::broker::MAX_PROVISIONAL_ID_BYTES
            || request.provisional_session_id.chars().any(char::is_control)
        {
            return Err(CollaborationBindingError::invalid());
        }
        let capability = AuthorityCore::generate_capability();
        let token = lock(&self.inner.core)
            .create_session_binding(
                engine,
                &request.provisional_session_id,
                request.resumed,
                &capability,
            )
            .map_err(binding_error)?;
        let Ok(server) = self
            .inner
            .listen_bound(engine, capability, next_socket_id())
            .await
        else {
            let _ = lock(&self.inner.core).dispose_session_binding(token);
            return Err(CollaborationBindingError::unavailable());
        };
        Ok(Arc::new(SessionBinding {
            inner: Arc::clone(&self.inner),
            token,
            server,
            disposed: AtomicBool::new(false),
        }))
    }
}

struct SessionBinding {
    inner: Arc<BrokerInner>,
    token: BrokerSessionBinding,
    server: Arc<BoundServer>,
    disposed: AtomicBool,
}

#[async_trait]
impl CollaborationMcpSessionBinding for SessionBinding {
    fn descriptor(&self) -> CollaborationMcpStdioDescriptor {
        self.server.descriptor.clone()
    }

    async fn confirm_source_session_key(
        &self,
        source_session_key: SessionKey,
    ) -> Result<(), CollaborationBindingError> {
        if self.disposed.load(Ordering::Acquire) {
            return Err(CollaborationBindingError::inactive());
        }
        lock(&self.inner.core)
            .confirm_source_session_key(self.token, &source_session_key)
            .map_err(binding_error)
    }

    async fn dispose(&self) -> Result<(), CollaborationBindingError> {
        if self.disposed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.inner
            .dispose_session_binding(self.token, &self.server)
            .await
    }
}

struct CodexRegistrar {
    inner: Arc<BrokerInner>,
}

#[async_trait]
impl CollaborationSourceRegistrar for CodexRegistrar {
    async fn register(
        &self,
        source_session_key: SessionKey,
    ) -> Result<Arc<dyn CollaborationLease>, CollaborationBindingError> {
        let token = lock(&self.inner.core)
            .register_codex_source(&source_session_key)
            .map_err(binding_error)?;
        Ok(Arc::new(CodexRegistration {
            inner: Arc::clone(&self.inner),
            token,
            disposed: AtomicBool::new(false),
        }))
    }
}

struct CodexRegistration {
    inner: Arc<BrokerInner>,
    token: BrokerSourceRegistration,
    disposed: AtomicBool,
}

#[async_trait]
impl CollaborationLease for CodexRegistration {
    async fn dispose(&self) -> Result<(), CollaborationBindingError> {
        if self.disposed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        lock(&self.inner.core)
            .dispose_codex_source(self.token)
            .map_err(binding_error)
    }
}

struct CodexFenceEntrant {
    inner: Arc<BrokerInner>,
}

#[async_trait]
impl CollaborationDepthFenceEntrant for CodexFenceEntrant {
    async fn enter(&self) -> Result<Arc<dyn CollaborationLease>, CollaborationBindingError> {
        let token = lock(&self.inner.core)
            .enter_codex_depth_fence()
            .map_err(binding_error)?;
        Ok(Arc::new(CodexFenceLease {
            inner: Arc::clone(&self.inner),
            token,
            disposed: AtomicBool::new(false),
        }))
    }
}

struct CodexFenceLease {
    inner: Arc<BrokerInner>,
    token: BrokerDepthFence,
    disposed: AtomicBool,
}

#[async_trait]
impl CollaborationLease for CodexFenceLease {
    async fn dispose(&self) -> Result<(), CollaborationBindingError> {
        if self.disposed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        lock(&self.inner.core).leave_codex_depth_fence(self.token);
        Ok(())
    }
}

fn validate_root(root: &Path) -> Result<PathBuf, CollaborationBrokerRuntimeError> {
    if !root.is_absolute()
        || root.parent().is_none()
        || root.as_os_str().as_encoded_bytes().contains(&0)
    {
        return Err(CollaborationBrokerRuntimeError::InvalidRoot);
    }
    Ok(root.to_path_buf())
}

fn validate_command(command: &Path) -> Result<(), CollaborationBrokerRuntimeError> {
    if !command.is_absolute() || command.as_os_str().as_encoded_bytes().contains(&0) {
        return Err(CollaborationBrokerRuntimeError::CommandUnavailable);
    }
    let metadata = std::fs::metadata(command)
        .map_err(|_| CollaborationBrokerRuntimeError::CommandUnavailable)?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err(CollaborationBrokerRuntimeError::CommandUnavailable);
    }
    Ok(())
}

/// Removes the socket files a broker left when its process ended without
/// stopping it. Only an owner-only socket named like a broker socket whose
/// connect is refused goes: a socket that accepts is a live listener, and
/// anything else, or anything that cannot be checked, stays.
async fn sweep_dead_sockets(directory: &Path) {
    let Ok(parent) = std::fs::symlink_metadata(directory) else {
        return;
    };
    if !parent.is_dir()
        || parent.file_type().is_symlink()
        || parent.permissions().mode() & 0o777 != 0o700
    {
        return;
    }
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(socket_id) = name.to_str().and_then(|name| name.strip_suffix(".sock")) else {
            continue;
        };
        if validate_socket_id(socket_id).is_err() {
            continue;
        }
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.file_type().is_socket()
            || metadata.uid() != parent.uid()
            || metadata.permissions().mode() & 0o777 != 0o600
        {
            continue;
        }
        let connected = tokio::time::timeout(
            SWEEP_CONNECT_TIMEOUT,
            tokio::net::UnixStream::connect(&path),
        )
        .await;
        if matches!(connected, Ok(Err(error)) if error.kind() == io::ErrorKind::ConnectionRefused) {
            let _ = unlink_matching_socket(&path, FileIdentity::of(&metadata));
        }
    }
}

fn validate_socket_id(value: &str) -> Result<(), CollaborationBrokerRuntimeError> {
    if !(8..=32).contains(&value.len())
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(CollaborationBrokerRuntimeError::InvalidSocketIdentity);
    }
    Ok(())
}

fn next_socket_id() -> String {
    Uuid::new_v4().simple().to_string()[..16].to_owned()
}

fn session_engine(engine: &EngineId) -> Result<CollaborationEngineId, CollaborationBindingError> {
    // Codex registers sources instead of binding sessions. Generic ACP
    // profiles, including Qwen, never inherit Grok's broker.
    CollaborationEngineId::from_wire(engine.as_str())
        .filter(|engine| *engine != CollaborationEngineId::Codex)
        .ok_or_else(CollaborationBindingError::invalid)
}

fn binding_error(error: BrokerError) -> CollaborationBindingError {
    match error {
        BrokerError::Stopping | BrokerError::BindingInactive => {
            CollaborationBindingError::inactive()
        }
        BrokerError::InvalidCapability
        | BrokerError::DuplicateCapability
        | BrokerError::InvalidSessionRequest
        | BrokerError::InvalidSource
        | BrokerError::SourceMismatch
        | BrokerError::DuplicateSourceOwner
        | BrokerError::BindingAlreadyConfirmed => CollaborationBindingError::invalid(),
    }
}
