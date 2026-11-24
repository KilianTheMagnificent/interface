use crate::{RemoteError, RemoteHub, replay::StreamKey};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{
        DefaultBodyLimit, Path, Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use interface_contracts::{
    ipc::{HistoryPage, unavailable_history},
    remote::{RemoteAccessRequest, RemoteAccessStatus, RemoteHistoryV2, RemoteWorkspaceSnapshotV2},
    remote_approval::{
        ApprovalDecisionV4, ApprovalObservationV4, ApprovalResponseV4, ApprovalSnapshotV4,
    },
    remote_threads::{
        REMOTE_THREAD_SEND_MAX_BYTES, REMOTE_THREADS_VERSION, RemoteInboxV2,
        RemoteThreadApprovalResponseV1, RemoteThreadDecisionV1, RemoteThreadDetailV1,
        RemoteThreadSendV1, RemoteThreadsV1,
    },
    remote_trust::{
        EnrollmentRequest, EnrollmentStatus, RememberedListener, TrustCommand, TrustStatus,
        UnlockRequest,
    },
    terminal::{TerminalSnapshot, TerminalSummary, TerminalWorkspaceRecord},
};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    future::Future,
    path::{Path as FilePath, PathBuf},
    pin::Pin,
    sync::Arc,
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::{Mutex, Semaphore, oneshot},
    task::JoinHandle,
};
use uuid::Uuid;
mod direct;
pub use direct::ConnectionPersistence;

pub type HistoryFuture<'a> =
    Pin<Box<dyn Future<Output = Result<HistoryPage, RemoteError>> + Send + 'a>>;
/// Phone Threads v1 reads and the one effect, answering an approval.
pub type ThreadsFuture<'a> =
    Pin<Box<dyn Future<Output = Result<RemoteThreadsV1, RemoteError>> + Send + 'a>>;
pub type InboxFuture<'a> =
    Pin<Box<dyn Future<Output = Result<RemoteInboxV2, RemoteError>> + Send + 'a>>;
pub type ThreadDetailFuture<'a> =
    Pin<Box<dyn Future<Output = Result<RemoteThreadDetailV1, RemoteError>> + Send + 'a>>;
pub type ThreadAnswerFuture<'a> =
    Pin<Box<dyn Future<Output = Result<(), RemoteError>> + Send + 'a>>;

fn threads_unavailable() -> RemoteError {
    error(
        "threads-unavailable",
        "Threads are unavailable on this Mac. Update Interface on your Mac.",
    )
}

pub trait RemoteBackend: Send + Sync + 'static {
    /// # Errors
    /// Refuses unavailable terminal state.
    fn snapshot(&self) -> Result<TerminalSnapshot, RemoteError>;
    /// # Errors
    /// Refuses an unavailable authoritative workspace roster.
    fn workspaces(&self) -> Result<Vec<TerminalWorkspaceRecord>, RemoteError> {
        Ok(Vec::new())
    }
    /// The session registry the sidebar files terminals by, indexed for the
    /// terminals bound to `keys`; empty files every terminal by its own folder.
    /// # Errors
    /// Refuses an unavailable registry.
    fn session_projects(
        &self,
        _keys: &std::collections::BTreeSet<&str>,
    ) -> Result<crate::workspaces::SessionProjects, RemoteError> {
        Ok(crate::workspaces::SessionProjects::default())
    }
    /// Reads only the key resolved from a host-confirmed terminal binding.
    fn history(&self, _key: String, _limit: u64) -> HistoryFuture<'_> {
        Box::pin(async {
            Ok(unavailable_history(
                "Conversation history is unavailable. Use Terminal view.",
            ))
        })
    }
    /// # Errors
    /// Returns unavailable without a host draft store; this operation never sends input.
    fn draft(
        &self,
        _key: StreamKey,
        _edit: Option<(u64, String)>,
    ) -> Result<interface_contracts::harness::SharedDraft, RemoteError> {
        Err(error(
            "draft-unavailable",
            "Shared drafts are unavailable on this host.",
        ))
    }
    /// Confirmed task evidence for one exact terminal generation.
    /// # Errors
    /// Returns unavailable when the host has no durable task store.
    fn results(
        &self,
        _key: StreamKey,
    ) -> Result<Vec<interface_contracts::harness::TaskEvidence>, RemoteError> {
        Err(error(
            "results-unavailable",
            "Task results are unavailable on this host.",
        ))
    }
    /// Synchronous, bounded admission to the terminal's existing input lane.
    /// # Errors
    /// Refuses stale generations or a full input lane before enqueue.
    fn enqueue(&self, key: StreamKey, bytes: Vec<u8>) -> Result<(), RemoteError>;
    /// Phone Threads v1: the Mac's Threads, needs you first.
    fn threads(&self) -> ThreadsFuture<'_> {
        Box::pin(async { Err(threads_unavailable()) })
    }
    /// The shared Threads/Agent owner inbox, version 2.
    fn inbox(&self) -> InboxFuture<'_> {
        Box::pin(async { Err(threads_unavailable()) })
    }
    /// One listed thread's recent turns and the approval it is parked on.
    fn thread_detail(&self, _key: String) -> ThreadDetailFuture<'_> {
        Box::pin(async { Err(threads_unavailable()) })
    }
    /// Answers the approval a listed thread is parked on, once. The caller
    /// already holds the phone's lease and a reserved command receipt; the
    /// backend checks the request is still the Mac's current one, that no
    /// other writer (a Code pane) holds the session, and keeps the delivery's
    /// certainty: a refusal is replay-safe only when nothing reached the agent.
    fn answer_thread(
        &self,
        _key: String,
        _approval_id: String,
        _allow: bool,
    ) -> ThreadAnswerFuture<'_> {
        Box::pin(async { Err(threads_unavailable()) })
    }
    /// Sends a follow-up to a listed thread at rest, once, under the same
    /// lease and receipt as an answer and the same single-writer rule.
    fn send_to_thread(&self, _key: String, _text: String) -> ThreadAnswerFuture<'_> {
        Box::pin(async { Err(threads_unavailable()) })
    }
}

struct Running {
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}
pub struct RemoteServer {
    hub: Arc<RemoteHub>,
    backend: Arc<dyn RemoteBackend>,
    assets_root: PathBuf,
    running: Mutex<Option<Running>>,
    administration: Mutex<()>,
    connection: Option<direct::DirectController>,
}

impl RemoteServer {
    pub fn new(hub: Arc<RemoteHub>, backend: Arc<dyn RemoteBackend>, assets_root: PathBuf) -> Self {
        Self {
            hub,
            backend,
            assets_root,
            running: Mutex::new(None),
            administration: Mutex::new(()),
            connection: None,
        }
    }

    /// # Errors
    /// Refuses invalid private connection configuration without rotating identity.
    pub fn with_connection_store(
        mut self,
        store: Arc<dyn ConnectionPersistence>,
        profile: interface_contracts::remote_connection::MacProfile,
    ) -> Result<Self, RemoteError> {
        self.connection = Some(direct::DirectController::new(store, profile)?);
        Ok(self)
    }

    /// # Errors
    /// Only desktop IPC may enable connections or issue invitations.
    pub async fn manage_connection(
        &self,
        request: interface_contracts::remote_connection::ConnectionAdminRequest,
    ) -> Result<interface_contracts::remote_connection::ConnectionStatusV5, RemoteError> {
        let _admission = self.administration.lock().await;
        let connection = self.connection.as_ref().ok_or_else(|| {
            error(
                "connection-unavailable",
                "Secure phone access is unavailable in this host.",
            )
        })?;
        if matches!(
            request.command,
            interface_contracts::remote_connection::ConnectionCommand::Disable {}
        ) {
            self.stop().await;
            if let Some(mut listener) = self.hub.remembered_listener()? {
                listener.enabled = false;
                self.hub.remember_listener(listener)?;
            }
        }
        connection.manage(self, request).await
    }

    /// # Errors
    /// V6 pairing is desktop-only; v5 administration remains unchanged.
    pub async fn pair_direct(
        &self,
        request: interface_contracts::remote_connection::PairingRequestV6,
    ) -> Result<interface_contracts::remote_connection::PairingStatusV6, RemoteError> {
        let _admission = self.administration.lock().await;
        if request.version != 6 {
            return Err(error(
                "invalid-request",
                "Unsupported phone pairing version.",
            ));
        }
        self.connection
            .as_ref()
            .ok_or_else(|| {
                error(
                    "connection-unavailable",
                    "Secure phone access is unavailable in this host.",
                )
            })?
            .pair_direct()
            .await
    }

    /// # Errors
    /// Refuses invalid origins, missing assets, listener failures or invalid grants.
    pub async fn manage(
        &self,
        request: RemoteAccessRequest,
    ) -> Result<RemoteAccessStatus, RemoteError> {
        let _admission = self.administration.lock().await;
        match request {
            RemoteAccessRequest::Status {} => self.hub.status(),
            RemoteAccessRequest::Disable {} => {
                self.stop().await;
                if let Some(connection) = &self.connection {
                    connection.disable_configuration().await?;
                }
                if let Some(mut listener) = self.hub.remembered_listener()? {
                    listener.enabled = false;
                    self.hub.remember_listener(listener)?;
                }
                self.hub.status()
            }
            RemoteAccessRequest::Revoke { id } => {
                self.hub.revoke(parse_uuid(&id)?)?;
                self.hub.status()
            }
            RemoteAccessRequest::Pair {} => {
                let code = self.hub.pairing_code()?;
                let mut status = self.hub.status()?;
                status.pairing_code = Some(code);
                Ok(status)
            }
            // The legacy v1 shape keeps its `origin`; only the empty
            // (loopback) value is served since Tailscale was retired.
            RemoteAccessRequest::Enable { origin } if !origin.trim().is_empty() => Err(error(
                "invalid-origin",
                "Tailscale addresses are no longer supported. Pair iPhone connects nearby or through your relay.",
            )),
            RemoteAccessRequest::Enable { .. } => self.enable(0).await,
        }
    }

    /// # Errors
    /// Refuses invalid or unavailable native device administration.
    pub fn manage_trust(
        &self,
        version: u8,
        command: TrustCommand,
    ) -> Result<TrustStatus, RemoteError> {
        self.hub.manage_trust(version, command)
    }

    /// Restores only a previously opted-in listener, at its exact saved port.
    /// A Tailscale address saved before that route was retired (2026-10-14) is
    /// kept in the trust file but no longer served: the listener comes back on
    /// loopback only, so the phone connection behind it still restores.
    /// # Errors
    /// A conflicting port or failed store keeps access off.
    pub async fn restore(&self) -> Result<(), RemoteError> {
        if let Some(listener) = self.hub.remembered_listener()?.filter(|l| l.enabled) {
            self.enable(listener.port).await?;
        }
        if let Some(connection) = &self.connection {
            connection.restore(self).await?;
        }
        Ok(())
    }

    async fn enable(&self, requested_port: u16) -> Result<RemoteAccessStatus, RemoteError> {
        let mut running = self.running.lock().await;
        if running.is_some() {
            return Err(error(
                "already-enabled",
                "Disable remote access before changing its address.",
            ));
        }
        let remembered = self.hub.remembered_listener()?;
        let port = if requested_port == 0 {
            remembered.as_ref().map_or(0, |l| l.port)
        } else {
            requested_port
        };
        let assets = load_assets(&self.assets_root)
            .await?
            .into_iter()
            .map(|(route, asset)| (route, (asset.content_type, Bytes::from(asset.bytes))))
            .collect();
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .map_err(|_| {
                error(
                    "bind-failed",
                    "The local remote-access listener could not start.",
                )
            })?;
        let address = listener
            .local_addr()
            .map_err(|_| error("bind-failed", "The local listener address is unavailable."))?;
        let local_origin = format!("http://{address}");
        // The saved origin field stays as written (frozen trust schema); no
        // public origin is served any more.
        self.hub.remember_listener(RememberedListener {
            enabled: true,
            port: address.port(),
            origin: remembered.map(|l| l.origin).unwrap_or_default(),
        })?;
        self.hub.activate(local_origin, None)?;
        let state = Arc::new(HttpState {
            hub: self.hub.clone(),
            backend: self.backend.clone(),
            assets,
            streams: Arc::new(Semaphore::new(8)),
            requests: Semaphore::new(32),
            histories: Semaphore::new(2),
        });
        let router = routes(state);
        let (stop, stopped) = oneshot::channel();
        let hub = self.hub.clone();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await;
            hub.disable();
        });
        *running = Some(Running { stop, task });
        self.hub.status()
    }

    pub async fn stop(&self) {
        self.hub.disable();
        if let Some(connection) = &self.connection {
            connection.stop().await;
        }
        let mut running = self.running.lock().await;
        self.hub.disable();
        if let Some(running) = running.take() {
            let _ = running.stop.send(());
            let mut task = running.task;
            if tokio::time::timeout(Duration::from_secs(3), &mut task)
                .await
                .is_err()
            {
                task.abort();
            }
        }
    }
}

fn error(code: &'static str, message: &'static str) -> RemoteError {
    RemoteError::new(code, message, true)
}
fn parse_uuid(value: &str) -> Result<Uuid, RemoteError> {
    Uuid::parse_str(value).map_err(|_| error("invalid-id", "A valid identifier is required."))
}

struct Asset {
    content_type: &'static str,
    bytes: Vec<u8>,
}
async fn load_assets(root: &FilePath) -> Result<BTreeMap<&'static str, Asset>, RemoteError> {
    let mut assets = BTreeMap::new();
    for (route, file, content_type) in [
        ("/", "remote.html", "text/html; charset=utf-8"),
        ("/remote.js", "remote.js", "text/javascript; charset=utf-8"),
        ("/remote.css", "remote.css", "text/css; charset=utf-8"),
        (
            "/manifest.webmanifest",
            "manifest.webmanifest",
            "application/manifest+json",
        ),
        ("/icon.svg", "icon.svg", "image/svg+xml"),
        // Safari's Add to Home Screen ignores SVG icons and would otherwise
        // photograph the page.
        ("/apple-touch-icon.png", "apple-touch-icon.png", "image/png"),
    ] {
        let path = root.join(file);
        let metadata = tokio::fs::symlink_metadata(&path).await.map_err(|_| {
            error(
                "assets-unavailable",
                "Build the iPhone interface before enabling remote access.",
            )
        })?;
        if !metadata.is_file() || metadata.len() > 4 * 1024 * 1024 {
            return Err(error(
                "assets-invalid",
                "The iPhone interface files are invalid.",
            ));
        }
        let bytes = tokio::fs::read(path).await.map_err(|_| {
            error(
                "assets-unavailable",
                "The iPhone interface could not be loaded.",
            )
        })?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(error(
                "assets-invalid",
                "The iPhone interface files are too large.",
            ));
        }
        assets.insert(
            route,
            Asset {
                content_type,
                bytes,
            },
        );
    }
    Ok(assets)
}

struct HttpState {
    hub: Arc<RemoteHub>,
    backend: Arc<dyn RemoteBackend>,
    /// Route to content type and body; a response shares the body, never copies it.
    assets: BTreeMap<&'static str, (&'static str, Bytes)>,
    streams: Arc<Semaphore>,
    requests: Semaphore,
    histories: Semaphore,
}

fn routes(state: Arc<HttpState>) -> Router {
    Router::new()
        .route("/", get(asset))
        .route("/remote.js", get(asset))
        .route("/remote.css", get(asset))
        .route("/icon.svg", get(asset))
        .route("/apple-touch-icon.png", get(asset))
        .route("/manifest.webmanifest", get(asset))
        .route("/api/v1/pair", post(pair))
        .route("/api/v3/enrollment", post(enrollment))
        .route("/api/v3/enrollment/status", post(enrollment_status))
        .route("/api/v3/unlock", post(unlock))
        .route("/api/v3/lock", post(lock_session))
        .route("/api/v1/terminals", get(terminals))
        .route("/api/v2/workspaces", get(workspaces))
        .route("/api/v3/workspaces", get(workspaces_v3))
        .route("/api/v4/workspaces", get(workspaces_v4))
        .route("/api/v1/threads", get(threads))
        .route("/api/v2/inbox", get(inbox))
        .route("/api/v1/threads/{engine}/{session}", get(thread_detail))
        .route(
            "/api/v1/threads/{engine}/{session}/control",
            post(claim_thread).delete(release_thread),
        )
        .route(
            "/api/v1/threads/{engine}/{session}/approval",
            post(answer_thread),
        )
        .route(
            "/api/v1/threads/{engine}/{session}/send",
            post(send_to_thread),
        )
        .route("/api/v1/voice-control", post(voice_control))
        .route("/api/v2/terminals/{id}/{generation}/history", get(history))
        .route("/api/v1/terminals/{id}/{generation}/results", get(results))
        .route(
            "/api/v1/terminals/{id}/{generation}/draft",
            get(read_draft).post(save_draft),
        )
        .route(
            "/api/v4/terminals/{id}/{generation}/approval",
            post(approval),
        )
        .route(
            "/api/v4/terminals/{id}/{generation}/approval/respond",
            post(approval_response),
        )
        .route(
            "/api/v1/terminals/{id}/{generation}/control",
            post(claim).delete(release),
        )
        .route("/api/v1/terminals/{id}/{generation}/input", post(input))
        .route("/api/v1/terminals/{id}/{generation}/stream", get(stream))
        .layer(DefaultBodyLimit::max(16 * 1024))
        .layer(middleware::from_fn_with_state(state.clone(), perimeter))
        .with_state(state)
}

async fn perimeter(
    State(state): State<Arc<HttpState>>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let headers = request.headers();
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    let must_have_origin = !matches!(
        *request.method(),
        axum::http::Method::GET | axum::http::Method::HEAD
    ) || headers.contains_key(header::UPGRADE);
    let mut response = if state.hub.accepts(host, origin, must_have_origin) {
        if let Ok(_permit) = state.requests.try_acquire() {
            tokio::time::timeout(Duration::from_secs(10), next.run(request))
                .await
                .unwrap_or_else(|_| StatusCode::REQUEST_TIMEOUT.into_response())
        } else {
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    } else {
        StatusCode::FORBIDDEN.into_response()
    };
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; font-src 'self' data:; manifest-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

impl IntoResponse for RemoteError {
    fn into_response(self) -> Response {
        let status = match self.code {
            "unauthorized" => StatusCode::UNAUTHORIZED,
            "control-lost" | "control-busy" | "command-conflict" => StatusCode::CONFLICT,
            _ => StatusCode::BAD_REQUEST,
        };
        (status, Json(self)).into_response()
    }
}

async fn asset(State(state): State<Arc<HttpState>>, uri: axum::http::Uri) -> Response {
    match state.assets.get(uri.path()) {
        Some((content_type, bytes)) => {
            ([(header::CONTENT_TYPE, *content_type)], bytes.clone()).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|part| part.trim().strip_prefix("interface_remote="))
}
fn authenticate(state: &HttpState, headers: &HeaderMap) -> Result<Uuid, RemoteError> {
    state.hub.authenticate(token(headers).unwrap_or(""))
}

/// The retired Control assistant's phone route (2026-10-14). Phones built
/// before the retirement still call it, so it stays served: an authenticated,
/// well-formed request is refused before any effect.
async fn voice_control(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
    Json(_request): Json<interface_contracts::voice_control::Request>,
) -> Result<Response, RemoteError> {
    authenticate(&state, &headers)?;
    Err(error(
        "voice-control-unavailable",
        "The Control assistant is no longer available.",
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pair {
    code: String,
    name: String,
}
async fn pair(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
    Json(pair): Json<Pair>,
) -> Result<Response, RemoteError> {
    let token = state.hub.enroll(&pair.code, &pair.name)?;
    let secure = headers
        .get(header::ORIGIN)
        .and_then(|o| o.to_str().ok())
        .is_some_and(|o| o.starts_with("https://"));
    let cookie = format!(
        "interface_remote={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age=43200{}",
        if secure { "; Secure" } else { "" }
    );
    let value = cookie
        .parse()
        .map_err(|_| error("pair-failed", "Pairing could not be completed."))?;
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(header::SET_COOKIE, value);
    Ok(response)
}

async fn enrollment(
    State(state): State<Arc<HttpState>>,
    Json(request): Json<EnrollmentRequest>,
) -> Result<Json<EnrollmentStatus>, RemoteError> {
    state.hub.request_enrollment(&request).map(Json)
}
async fn enrollment_status(
    State(state): State<Arc<HttpState>>,
    Json(request): Json<UnlockRequest>,
) -> Result<Json<EnrollmentStatus>, RemoteError> {
    state.hub.enrollment_status(&request).map(Json)
}
async fn unlock(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
    Json(request): Json<UnlockRequest>,
) -> Result<Response, RemoteError> {
    let token = state.hub.unlock(&request)?;
    let secure = headers
        .get(header::ORIGIN)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|h| h.starts_with("https://"));
    // Session-only cookie. Native clients retain it in memory, never in Keychain or preferences.
    let cookie = format!(
        "interface_remote={token}; HttpOnly; SameSite=Strict; Path=/{}",
        if secure { "; Secure" } else { "" }
    );
    let mut response = Json(serde_json::json!({"version":3,"expiresIn":900})).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        cookie
            .parse()
            .map_err(|_| error("unlock-failed", "Unlock failed."))?,
    );
    Ok(response)
}
async fn lock_session(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
) -> Result<StatusCode, RemoteError> {
    state.hub.lock_session(token(&headers).unwrap_or(""))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn terminals(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
) -> Result<Json<TerminalSnapshot>, RemoteError> {
    authenticate(&state, &headers)?;
    Ok(Json(crate::workspaces::without_attention(
        state.backend.snapshot()?,
    )))
}

async fn workspaces(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
) -> Result<Json<RemoteWorkspaceSnapshotV2>, RemoteError> {
    authenticate(&state, &headers)?;
    // The roster read waits on the lock a roster save holds across its drive
    // flush, so it runs off the async workers, as the draft reads do.
    let backend = Arc::clone(&state.backend);
    let (snapshot, saved) = tokio::task::spawn_blocking(move || {
        Ok::<_, RemoteError>((backend.snapshot()?, backend.workspaces()?))
    })
    .await
    .map_err(|_| {
        error(
            "workspaces-unavailable",
            "The workspace list is unavailable.",
        )
    })??;
    authenticate(&state, &headers)?;
    Ok(Json(crate::workspaces::workspace_snapshot(
        snapshot, &saved,
    )))
}

async fn workspaces_v3(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
) -> Result<Json<interface_contracts::remote::RemoteWorkspaceSnapshotV3>, RemoteError> {
    let (snapshot, saved, sessions) = roster_inputs(&state, &headers).await?;
    Ok(Json(crate::workspaces::workspace_snapshot_v3_indexed(
        snapshot, &saved, &sessions,
    )))
}

/// Roster v4: the v3 roster whose terminals say which sessions need you.
async fn workspaces_v4(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
) -> Result<Json<interface_contracts::remote::RemoteWorkspaceSnapshotV4>, RemoteError> {
    let (snapshot, saved, sessions) = roster_inputs(&state, &headers).await?;
    Ok(Json(crate::workspaces::workspace_snapshot_v4_indexed(
        snapshot, &saved, &sessions,
    )))
}

/// What rosters v3 and v4 are built from, read behind the grant on both sides.
async fn roster_inputs(
    state: &Arc<HttpState>,
    headers: &HeaderMap,
) -> Result<
    (
        TerminalSnapshot,
        Vec<TerminalWorkspaceRecord>,
        crate::workspaces::SessionProjects,
    ),
    RemoteError,
> {
    authenticate(state, headers)?;
    let backend = Arc::clone(&state.backend);
    let (snapshot, saved, sessions) = tokio::task::spawn_blocking(move || {
        let snapshot = backend.snapshot()?;
        let saved = backend.workspaces()?;
        let keys = snapshot
            .terminals
            .iter()
            .filter_map(|terminal| terminal.session_key.as_deref())
            .collect();
        let sessions = backend.session_projects(&keys)?;
        Ok::<_, RemoteError>((snapshot, saved, sessions))
    })
    .await
    .map_err(|_| {
        error(
            "workspaces-unavailable",
            "The workspace list is unavailable.",
        )
    })??;
    authenticate(state, headers)?;
    Ok((snapshot, saved, sessions))
}

/// `engine#sessionId` from the two path segments the phone addresses a
/// thread by. Only the characters engine ids and provider session ids use.
fn thread_key(engine: &str, session: &str) -> Result<String, RemoteError> {
    let engine_ok = (1..=32).contains(&engine.len())
        && engine
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b':' || b == b'-');
    let session_ok = (1..=128).contains(&session.len())
        && session
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if engine_ok && session_ok {
        Ok(format!("{engine}#{session}"))
    } else {
        Err(error("invalid-thread", "That is not a thread on your Mac."))
    }
}

async fn threads(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
) -> Result<Json<RemoteThreadsV1>, RemoteError> {
    authenticate(&state, &headers)?;
    let list = state.backend.threads().await?;
    authenticate(&state, &headers)?;
    Ok(Json(list))
}

async fn inbox(
    State(state): State<Arc<HttpState>>,
    headers: HeaderMap,
) -> Result<Json<RemoteInboxV2>, RemoteError> {
    authenticate(&state, &headers)?;
    let list = state.backend.inbox().await?;
    authenticate(&state, &headers)?;
    Ok(Json(list))
}

async fn thread_detail(
    State(state): State<Arc<HttpState>>,
    Path((engine, session)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, RemoteError> {
    authenticate(&state, &headers)?;
    let key = thread_key(&engine, &session)?;
    // A detail reads a provider transcript, as a history page does.
    let detail = {
        let _permit = state.histories.try_acquire().map_err(|_| {
            error(
                "history-busy",
                "Conversation history is busy. Try again shortly.",
            )
        })?;
        state.backend.thread_detail(key).await?
    };
    authenticate(&state, &headers)?;
    let body = serde_json::to_vec(&detail)
        .ok()
        .filter(|bytes| bytes.len() <= PHONE_BODY_LIMIT)
        .ok_or_else(|| {
            error(
                "thread-unavailable",
                "This thread is too large to show on the phone. Read it on your Mac.",
            )
        })?;
    Ok(([(header::CONTENT_TYPE, "application/json")], body).into_response())
}

async fn claim_thread(
    State(state): State<Arc<HttpState>>,
    Path((engine, session)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, RemoteError> {
    let device = authenticate(&state, &headers)?;
    let key = thread_key(&engine, &session)?;
    Ok(Json(
        serde_json::json!({ "lease": state.hub.claim_thread(device, &key)? }),
    ))
}

async fn release_thread(
    State(state): State<Arc<HttpState>>,
    Path((engine, session)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<StatusCode, RemoteError> {
    let device = authenticate(&state, &headers)?;
    state
        .hub
        .release_thread(device, &thread_key(&engine, &session)?);
    Ok(StatusCode::NO_CONTENT)
}

/// Answers the approval a thread is parked on: the phone's lease and a new
/// command id, the receipt reserved before the effect starts, the effect run
/// once to its end even if this request is dropped, and its outcome kept for
/// any repeat of the same command id.
async fn answer_thread(
    State(state): State<Arc<HttpState>>,
    Path((engine, session)): Path<(String, String)>,
    headers: HeaderMap,
    Json(answer): Json<RemoteThreadApprovalResponseV1>,
) -> Result<StatusCode, RemoteError> {
    let device = authenticate(&state, &headers)?;
    let key = thread_key(&engine, &session)?;
    if answer.version != REMOTE_THREADS_VERSION
        || answer.approval_id.is_empty()
        || answer.approval_id.len() > 256
    {
        return Err(error(
            "invalid-version",
            "Update the phone app to answer requests.",
        ));
    }
    let mut request = b"approval\0".to_vec();
    request.extend(
        serde_json::to_vec(&answer)
            .map_err(|_| error("invalid-approval", "Invalid approval response."))?,
    );
    let allow = answer.decision == RemoteThreadDecisionV1::AllowOnce;
    thread_effect(
        &state,
        device,
        key,
        &headers,
        &request,
        move |backend, key| {
            Box::pin(async move { backend.answer_thread(key, answer.approval_id, allow).await })
        },
    )
    .await
}

/// Sends a follow-up prompt to a thread at rest, under the same lease and
/// receipt rules as an answer.
async fn send_to_thread(
    State(state): State<Arc<HttpState>>,
    Path((engine, session)): Path<(String, String)>,
    headers: HeaderMap,
    Json(message): Json<RemoteThreadSendV1>,
) -> Result<StatusCode, RemoteError> {
    let device = authenticate(&state, &headers)?;
    let key = thread_key(&engine, &session)?;
    if message.version != REMOTE_THREADS_VERSION {
        return Err(error(
            "invalid-version",
            "Update the phone app to send messages.",
        ));
    }
    if message.text.trim().is_empty() || message.text.len() > REMOTE_THREAD_SEND_MAX_BYTES {
        return Err(error("input-size", "Send between 1 character and 12 KiB."));
    }
    let mut request = b"send\0".to_vec();
    request.extend(message.text.as_bytes());
    thread_effect(
        &state,
        device,
        key,
        &headers,
        &request,
        move |backend, key| {
            Box::pin(async move { backend.send_to_thread(key, message.text).await })
        },
    )
    .await
}

/// One thread effect: this phone's lease and a new command id, the receipt
/// reserved before the effect starts, the effect run once to its end even if
/// this request is dropped, and its outcome kept for any repeat of the id.
async fn thread_effect(
    state: &Arc<HttpState>,
    device: Uuid,
    key: String,
    headers: &HeaderMap,
    request: &[u8],
    run: impl FnOnce(Arc<dyn RemoteBackend>, String) -> ThreadAnswerFuture<'static> + Send + 'static,
) -> Result<StatusCode, RemoteError> {
    let lease = header_uuid(headers, "x-interface-control")?;
    let command = header_uuid(headers, "x-interface-command")?;
    let receipt = match state
        .hub
        .begin_thread_effect(device, &key, lease, command, request)?
    {
        crate::ThreadEffect::Recorded(result) => return result.map(|()| StatusCode::ACCEPTED),
        crate::ThreadEffect::Reserved(receipt) => receipt,
    };
    let backend = Arc::clone(&state.backend);
    let hub = Arc::clone(&state.hub);
    let effect = tokio::spawn(async move {
        let result = run(backend, key).await;
        hub.settle_thread_effect(receipt, result.clone());
        result
    });
    effect
        .await
        .unwrap_or_else(|_| {
            Err(RemoteError::new(
                "outcome-unknown",
                "This may have reached the agent. Check on your Mac before trying again.",
                false,
            ))
        })
        .map(|()| StatusCode::ACCEPTED)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DraftEdit {
    expected_revision: u64,
    text: String,
}
async fn read_draft(
    State(state): State<Arc<HttpState>>,
    Path((id, generation)): Path<(String, u64)>,
    headers: HeaderMap,
) -> Result<Json<interface_contracts::harness::SharedDraft>, RemoteError> {
    authenticate(&state, &headers)?;
    let key = target(&state, &id, generation)?;
    let backend = Arc::clone(&state.backend);
    let draft = tokio::task::spawn_blocking(move || backend.draft(key, None))
        .await
        .map_err(|_| error("draft-unavailable", "The draft could not be read."))??;
    authenticate(&state, &headers)?;
    target(&state, &id, generation)?;
    Ok(Json(draft))
}
async fn save_draft(
    State(state): State<Arc<HttpState>>,
    Path((id, generation)): Path<(String, u64)>,
    headers: HeaderMap,
    Json(edit): Json<DraftEdit>,
) -> Result<Json<interface_contracts::harness::SharedDraft>, RemoteError> {
    authenticate(&state, &headers)?;
    let key = target(&state, &id, generation)?;
    if edit.text.len() > 48_000 {
        return Err(error("draft-too-large", "Draft exceeds its size limit."));
    }
    // The save commits the whole harness store with fsyncs, so it runs off
    // the async workers. A lost worker is reported like any failed save: the
    // revision check refuses a stale retry, so reloading is always safe.
    let backend = Arc::clone(&state.backend);
    let draft = tokio::task::spawn_blocking(move || {
        backend.draft(key, Some((edit.expected_revision, edit.text)))
    })
    .await
    .map_err(|_| {
        error(
            "draft-conflict",
            "Draft changed elsewhere or could not be saved. Reload the saved draft before sharing edits.",
        )
    })??;
    authenticate(&state, &headers)?;
    target(&state, &id, generation)?;
    Ok(Json(draft))
}

async fn results(
    State(state): State<Arc<HttpState>>,
    Path((id, generation)): Path<(String, u64)>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, RemoteError> {
    authenticate(&state, &headers)?;
    let key = target(&state, &id, generation)?;
    let backend = Arc::clone(&state.backend);
    let results = tokio::task::spawn_blocking(move || backend.results(key))
        .await
        .map_err(|_| error("results-unavailable", "Task results could not be read."))??;
    authenticate(&state, &headers)?;
    target(&state, &id, generation)?;
    Ok(Json(
        serde_json::json!({"version":1,"terminalId":id,"generation":generation,"results":results}),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryQuery {
    limit: Option<u64>,
}

async fn history(
    State(state): State<Arc<HttpState>>,
    Path((id, generation)): Path<(String, u64)>,
    Query(query): Query<HistoryQuery>,
    headers: HeaderMap,
) -> Result<Response, RemoteError> {
    authenticate(&state, &headers)?;
    let (_, terminal) = target_summary(&state, &id, generation)?;
    let binding = crate::workspaces::history_key(&terminal);
    let page = if let Some(ref binding) = binding {
        let _permit = state.histories.try_acquire().map_err(|_| {
            error(
                "history-busy",
                "Conversation history is busy. Try again shortly.",
            )
        })?;
        state
            .backend
            .history(binding.clone(), query.limit.unwrap_or(500).clamp(500, 5000))
            .await?
    } else {
        unavailable_history(
            "This session is available in Terminal view. Conversation history requires a confirmed Claude or Codex session.",
        )
    };
    authenticate(&state, &headers)?;
    let current = state
        .backend
        .snapshot()?
        .terminals
        .into_iter()
        .find(|t| t.id == terminal.id && t.generation == terminal.generation)
        .ok_or_else(|| error("stale-terminal", "This session has changed. Reopen it."))?;
    if crate::workspaces::history_key(&current) != binding {
        return Err(error(
            "stale-terminal",
            "The provider session has changed. Reopen it.",
        ));
    }
    // Serialize the whole envelope once and bound it the way the tunnel
    // (direct.rs) and the phone (DirectConnection.swift) bound a body.
    let envelope = |history| RemoteHistoryV2 {
        version: 2,
        terminal_id: terminal.id,
        generation: terminal.generation,
        history,
    };
    let body = match serde_json::to_vec(&envelope(page)) {
        Ok(bytes) if bytes.len() <= PHONE_BODY_LIMIT => bytes,
        _ => serde_json::to_vec(&envelope(unavailable_history(
            "This history page exceeds the phone limit. Use Terminal view or read it on your Mac.",
        )))
        .map_err(|_| {
            error(
                "history-unavailable",
                "Conversation history is unavailable.",
            )
        })?,
    };
    Ok(([(header::CONTENT_TYPE, "application/json")], body).into_response())
}

/// Largest response body the tunnel forwards and the phone accepts.
const PHONE_BODY_LIMIT: usize = 2 * 1024 * 1024;

fn target(state: &HttpState, id: &str, generation: u64) -> Result<StreamKey, RemoteError> {
    target_summary(state, id, generation).map(|(key, _)| key)
}

/// Resolves a live terminal and its summary from one backend snapshot.
fn target_summary(
    state: &HttpState,
    id: &str,
    generation: u64,
) -> Result<(StreamKey, TerminalSummary), RemoteError> {
    let id = parse_uuid(id)?;
    let summary = (generation != 0)
        .then(|| state.backend.snapshot())
        .transpose()?
        .and_then(|snapshot| {
            snapshot
                .terminals
                .into_iter()
                .find(|t| t.id.as_uuid() == id && t.generation.0 == generation)
        })
        .ok_or_else(|| {
            error(
                "stale-terminal",
                "This terminal has closed or restarted. Return to the session list.",
            )
        })?;
    Ok(((id, generation), summary))
}

async fn claim(
    State(state): State<Arc<HttpState>>,
    Path((id, generation)): Path<(String, u64)>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, RemoteError> {
    let device = authenticate(&state, &headers)?;
    let key = target(&state, &id, generation)?;
    Ok(Json(
        serde_json::json!({ "lease": state.hub.claim(device, key)? }),
    ))
}
async fn release(
    State(state): State<Arc<HttpState>>,
    Path((id, generation)): Path<(String, u64)>,
    headers: HeaderMap,
) -> Result<StatusCode, RemoteError> {
    let device = authenticate(&state, &headers)?;
    state.hub.release(device, (parse_uuid(&id)?, generation));
    Ok(StatusCode::NO_CONTENT)
}
async fn input(
    State(state): State<Arc<HttpState>>,
    Path((id, generation)): Path<(String, u64)>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Result<StatusCode, RemoteError> {
    let device = authenticate(&state, &headers)?;
    let key = target(&state, &id, generation)?;
    let lease = header_uuid(&headers, "x-interface-control")?;
    let command = header_uuid(&headers, "x-interface-command")?;
    state.hub.input(device, key, lease, command, &bytes, || {
        state.hub.replay.invalidate_approval(key);
        state.backend.enqueue(key, bytes.to_vec())
    })?;
    Ok(StatusCode::ACCEPTED)
}

fn recognized_approval(
    summary: &TerminalSummary,
    key: StreamKey,
    cursor: u64,
    screen: &interface_contracts::terminal_controls::TerminalControlScreenV1,
) -> Option<crate::approval::RecognizedApproval> {
    summary
        .launch
        .and_then(|launch| crate::approval::recognize(launch.profile_id, key, cursor, screen))
}

async fn approval(
    State(state): State<Arc<HttpState>>,
    Path((id, generation)): Path<(String, u64)>,
    headers: HeaderMap,
    Json(observation): Json<ApprovalObservationV4>,
) -> Result<Json<ApprovalSnapshotV4>, RemoteError> {
    authenticate(&state, &headers)?;
    let (key, terminal) = target_summary(&state, &id, generation)?;
    if observation.version != 4 {
        return Err(error(
            "invalid-version",
            "Update the phone app to review requests.",
        ));
    }
    // Most observations during streaming are already stale: skip recognizing
    // (serializing and hashing) a screen that cannot be offered.
    let approval = state
        .hub
        .replay
        .approval_current(key, observation.cursor)
        .then(|| recognized_approval(&terminal, key, observation.cursor, &observation.screen))
        .flatten()
        .map(|r| r.view);
    Ok(Json(ApprovalSnapshotV4 {
        version: 4,
        approval,
    }))
}

async fn approval_response(
    State(state): State<Arc<HttpState>>,
    Path((id, generation)): Path<(String, u64)>,
    headers: HeaderMap,
    Json(response): Json<ApprovalResponseV4>,
) -> Result<StatusCode, RemoteError> {
    let device = authenticate(&state, &headers)?;
    let (key, terminal) = target_summary(&state, &id, generation)?;
    if response.version != 4 {
        return Err(error(
            "invalid-version",
            "Update the phone app to review requests.",
        ));
    }
    let recognized = recognized_approval(&terminal, key, response.cursor, &response.screen)
        .filter(|r| r.view.id == response.approval_id)
        .ok_or_else(|| {
            error(
                "approval-changed",
                "This request has changed. Review it again.",
            )
        })?;
    let bytes = match response.decision {
        ApprovalDecisionV4::AllowOnce => recognized
            .allow
            .ok_or_else(|| error("approval-unsupported", "Approve this request on your Mac."))?,
        ApprovalDecisionV4::Deny => vec![0x1b],
    };
    let fingerprint = serde_json::to_vec(&response)
        .map_err(|_| error("invalid-approval", "Invalid approval response."))?;
    let lease = header_uuid(&headers, "x-interface-control")?;
    let command = header_uuid(&headers, "x-interface-command")?;
    state
        .hub
        .input(device, key, lease, command, &fingerprint, || {
            state
                .hub
                .replay
                .respond_current(key, response.cursor, || state.backend.enqueue(key, bytes))
        })?;
    Ok(StatusCode::ACCEPTED)
}
fn header_uuid(headers: &HeaderMap, name: &str) -> Result<Uuid, RemoteError> {
    parse_uuid(
        headers
            .get(name)
            .and_then(|h| h.to_str().ok())
            .unwrap_or(""),
    )
}

#[derive(Deserialize)]
struct Cursor {
    #[serde(default)]
    cursor: u64,
}
async fn stream(
    State(state): State<Arc<HttpState>>,
    Path((id, generation)): Path<(String, u64)>,
    Query(cursor): Query<Cursor>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, RemoteError> {
    authenticate(&state, &headers)?;
    let key = target(&state, &id, generation)?;
    let permit = state
        .streams
        .clone()
        .try_acquire_owned()
        .map_err(|_| error("streams-full", "Too many terminal views are connected."))?;
    if state.hub.replay.read(key, cursor.cursor).is_none() {
        return Err(error(
            "output-unavailable",
            "Output history is unavailable for this terminal.",
        ));
    }
    let token = token(&headers).unwrap_or("").to_owned();
    Ok(upgrade
        .max_message_size(1024)
        .max_frame_size(1024)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            output_socket(socket, state, token, key, cursor.cursor).await;
        }))
}

async fn output_socket(
    mut socket: WebSocket,
    state: Arc<HttpState>,
    token: String,
    key: StreamKey,
    mut cursor: u64,
) {
    let mut timer = tokio::time::interval(Duration::from_millis(200));
    loop {
        tokio::select! {
            _ = timer.tick() => {
                if state.hub.authenticate(&token).is_err() { break; }
                if let Some(page) = state.hub.replay.read(key, cursor) {
                    if !page.bytes.is_empty() || page.gap {
                        let bytes = page.encode();
                        match tokio::time::timeout(Duration::from_secs(2), socket.send(Message::Binary(bytes.into()))).await {
                            Ok(Ok(())) => cursor = page.end,
                            _ => break,
                        }
                    }
                } else { break; }
            }
            received = socket.recv() => match received {
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {},
                _ => break,
            }
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(1), socket.send(Message::Close(None))).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    struct NoTerminals;
    impl RemoteBackend for NoTerminals {
        fn snapshot(&self) -> Result<TerminalSnapshot, RemoteError> {
            Ok(TerminalSnapshot {
                revision: 0,
                terminals: vec![],
            })
        }
        fn enqueue(&self, _: StreamKey, _: Vec<u8>) -> Result<(), RemoteError> {
            Err(error("unavailable", "No terminals."))
        }
    }

    #[tokio::test]
    async fn a_tailscale_address_is_refused_before_anything_starts() {
        let hub = Arc::new(RemoteHub::default());
        let server =
            RemoteServer::new(hub.clone(), Arc::new(NoTerminals), FilePath::new("").into());
        for origin in [
            "https://mac.example.ts.net",
            " https://mac.example.ts.net/ ",
        ] {
            let refused = server
                .manage(RemoteAccessRequest::Enable {
                    origin: origin.into(),
                })
                .await
                .unwrap_err();
            assert_eq!(refused.code, "invalid-origin");
            assert!(refused.replay_safe);
        }
        assert!(!hub.is_enabled().unwrap());
        assert!(server.running.lock().await.is_none());
    }

    #[tokio::test]
    async fn serves_the_touch_icon_and_refuses_a_build_without_it() {
        let root = std::env::temp_dir().join(format!("interface-remote-assets-{}", Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        for file in [
            "remote.html",
            "remote.js",
            "remote.css",
            "manifest.webmanifest",
            "icon.svg",
            "apple-touch-icon.png",
        ] {
            std::fs::write(root.join(file), file).unwrap();
        }
        let assets = load_assets(&root).await.unwrap();
        let icon = &assets["/apple-touch-icon.png"];
        assert_eq!(icon.content_type, "image/png");
        assert_eq!(icon.bytes, b"apple-touch-icon.png");
        std::fs::remove_file(root.join("apple-touch-icon.png")).unwrap();
        assert!(load_assets(&root).await.is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
#[path = "http_history_tests.rs"]
mod history_tests;

#[cfg(test)]
#[path = "http_approval_tests.rs"]
mod approval_tests;

#[cfg(test)]
#[path = "http_threads_tests.rs"]
mod threads_tests;

#[cfg(test)]
#[path = "http_harness_tests.rs"]
mod harness_tests;
#[cfg(test)]
#[path = "http_voice_tests.rs"]
mod voice_tests;
