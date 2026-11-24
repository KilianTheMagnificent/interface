//! The encrypted v5 entry point delegates all session authority to the retained handlers.
use super::{
    Arc, BTreeMap, DefaultBodyLimit, Duration, EnrollmentRequest, HeaderMap, HistoryQuery,
    HttpState, IntoResponse, JoinHandle, Json, Mutex, Next, Path, Query, RemoteError, RemoteServer,
    Response, Router, Semaphore, State, StatusCode, TcpListener, approval, approval_response,
    authenticate, claim, enrollment_status, error, header, history, input, lock_session,
    middleware, parse_uuid, post, release, terminals, unlock, workspaces,
};
use crate::{
    connection::{RequestWindow, relay_origin},
    connection_crypto::{ConnectionIdentity, hex},
    relay::{RelayProof, RelayRegistration},
};
use axum::{extract::connect_info::Connected, http::HeaderValue};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures::{SinkExt, StreamExt};
use interface_contracts::remote_connection::{
    ConnectionAdminRequest, ConnectionCommand, ConnectionConfigurationV1, ConnectionListenerV1,
    ConnectionStatusV5, EncryptedPacketV5, INVITATION_LIFETIME_SECONDS, InvitationKind,
    MAX_NEARBY_ORIGINS, MAX_PACKET_BYTES, MacProfile, MacRoutesV1, PAIRING_PREFIX,
    PAIRING_PREFIX_V6, PairingInvitationV5, PairingInvitationV6, PairingStatusV6, TunnelRequestV5,
    TunnelResponseV5, nearby_origin,
};
use mdns_sd::{ServiceDaemon, ServiceInfo};
use qrcode::{Color, QrCode};
use rand_core::{OsRng, TryRngCore};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::{
        Mutex as StdMutex,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub trait ConnectionPersistence: Send + Sync {
    /// # Errors
    /// Refuses corrupt or unsafe listener state.
    fn load_listener(&self) -> Result<Option<ConnectionListenerV1>, RemoteError>;
    /// # Errors
    /// A listener must not start after failed or uncertain persistence.
    fn save_listener(&self, value: &ConnectionListenerV1) -> Result<(), RemoteError>;
    /// # Errors
    /// Invalid or unavailable configuration must fail closed.
    fn load(&self) -> Result<Option<ConnectionConfigurationV1>, RemoteError>;
    /// # Errors
    /// Refuses failed or uncertain persistence.
    fn save(&self, value: &ConnectionConfigurationV1) -> Result<(), RemoteError>;
}
pub(super) struct DirectController {
    store: Arc<dyn ConnectionPersistence>,
    state: Mutex<ControllerState>,
    profile: MacProfile,
}
struct ControllerState {
    config: Option<ConnectionConfigurationV1>,
    running: Option<DirectRunning>,
}
struct DirectRunning {
    port: u16,
    runtime: Arc<DirectRuntime>,
    listener: JoinHandle<()>,
    relay: Option<JoinHandle<()>>,
    discovery: Option<ServiceDaemon>,
}
struct Invitation {
    value: PairingInvitationV5,
    claimant: Option<[u8; 32]>,
}
struct DirectRuntime {
    identity: ConnectionIdentity,
    port: u16,
    http: Arc<HttpState>,
    window: StdMutex<RequestWindow>,
    invitation: StdMutex<Option<Invitation>>,
    active: AtomicBool,
    relay_online: AtomicBool,
    requests: Semaphore,
}
/// The browser origin every tunnelled request presents to the retained handlers.
const TUNNEL_ORIGIN: &str = "https://interface.invalid";
fn connection_error() -> RemoteError {
    error(
        "connection-unavailable",
        "The secure phone connection is unavailable. Review iPhone remote access in Settings.",
    )
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |v| v.as_secs())
}

impl DirectController {
    pub(super) fn new(
        store: Arc<dyn ConnectionPersistence>,
        profile: MacProfile,
    ) -> Result<Self, RemoteError> {
        let config = store.load()?;
        if config.as_ref().is_some_and(|c| !c.valid()) {
            return Err(connection_error());
        }
        Ok(Self {
            store,
            state: Mutex::new(ControllerState {
                config,
                running: None,
            }),
            profile,
        })
    }
    pub(super) async fn restore(&self, server: &RemoteServer) -> Result<(), RemoteError> {
        let relay = self
            .state
            .lock()
            .await
            .config
            .as_ref()
            .filter(|c| c.enabled)
            .map(|c| c.relay_origin.clone());
        if let Some(relay) = relay {
            self.enable(server, relay).await?;
        }
        Ok(())
    }
    pub(super) async fn manage(
        &self,
        server: &RemoteServer,
        request: ConnectionAdminRequest,
    ) -> Result<ConnectionStatusV5, RemoteError> {
        if request.version != 5 {
            return Err(connection_error());
        }
        match request.command {
            ConnectionCommand::Status {} => self.status().await,
            ConnectionCommand::Enable { relay_origin } => {
                self.enable(server, relay_origin).await?;
                self.status().await
            }
            ConnectionCommand::Disable {} => {
                self.stop().await;
                self.disable_configuration().await?;
                self.status().await
            }
            ConnectionCommand::Pair {} => self.pair().await,
        }
    }
    /// Saves `config` unless it is already the saved configuration, so a
    /// restore at launch rewrites nothing.
    fn save_changed(
        &self,
        saved: Option<&ConnectionConfigurationV1>,
        config: &ConnectionConfigurationV1,
    ) -> Result<(), RemoteError> {
        if saved.is_some_and(|saved| {
            saved.version == config.version
                && saved.enabled == config.enabled
                && saved.private_key == config.private_key
                && saved.relay_origin == config.relay_origin
        }) {
            return Ok(());
        }
        self.store.save(config)
    }
    pub(super) async fn disable_configuration(&self) -> Result<(), RemoteError> {
        let mut state = self.state.lock().await;
        if let Some(mut config) = state.config.clone() {
            config.enabled = false;
            self.store.save(&config)?;
            state.config = Some(config);
        }
        Ok(())
    }
    async fn enable(
        &self,
        server: &RemoteServer,
        relay: Option<String>,
    ) -> Result<(), RemoteError> {
        if let Some(origin) = &relay {
            relay_origin(origin).map_err(|_| {
                error(
                    "invalid-relay",
                    "Enter the relay's HTTPS origin without a path.",
                )
            })?;
        }
        let mut state = self.state.lock().await;
        if state.running.is_some() {
            return Err(error(
                "already-enabled",
                "Turn off the secure connection before changing its relay.",
            ));
        }
        let identity = match state.config.as_ref() {
            Some(config) => ConnectionIdentity::from_private(&config.private_key),
            None => ConnectionIdentity::generate(),
        }
        .map_err(|_| connection_error())?;
        let remembered = self.store.load_listener()?;
        if remembered
            .as_ref()
            .is_some_and(|listener| !listener.valid())
        {
            return Err(connection_error());
        }
        let listener = TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, remembered.as_ref().map_or(0, |v| v.port)))
            .await
            .map_err(|_| error("connection-port-unavailable", "The saved phone connection port is busy. Close the conflicting service, then enable phone access again."))?;
        let port = listener
            .local_addr()
            .map_err(|_| connection_error())?
            .port();
        // A restore that rebinds the saved port has nothing new to persist.
        if remembered.is_none_or(|listener| listener.port != port) {
            self.store
                .save_listener(&ConnectionListenerV1 { version: 1, port })?;
        }
        if !server.hub.is_enabled()? {
            server.enable(0).await?;
        }
        let config = ConnectionConfigurationV1 {
            version: 1,
            enabled: true,
            private_key: identity.private_storage(),
            relay_origin: relay.clone(),
        };
        if self.save_changed(state.config.as_ref(), &config).is_err() {
            server.hub.disable();
            return Err(connection_error());
        }
        state.config = Some(config);
        let runtime = Arc::new(DirectRuntime {
            identity,
            port,
            http: Arc::new(HttpState {
                hub: server.hub.clone(),
                backend: server.backend.clone(),
                assets: BTreeMap::new(),
                streams: Arc::new(Semaphore::new(8)),
                requests: Semaphore::new(32),
                histories: Semaphore::new(2),
            }),
            window: StdMutex::new(RequestWindow::new(8192)),
            invitation: StdMutex::new(None),
            active: AtomicBool::new(true),
            relay_online: AtomicBool::new(false),
            requests: Semaphore::new(8),
        });
        let local_runtime = runtime.clone();
        let listener = tokio::spawn(async move {
            let _ = serve_direct(listener, local_runtime.clone()).await;
            local_runtime.active.store(false, Ordering::Release);
        });
        let relay = relay.map(|origin| {
            let runtime = runtime.clone();
            tokio::spawn(async move {
                relay_loop(runtime, origin).await;
            })
        });
        let discovery = advertise(&runtime.identity.mac_id(), port).ok();
        state.running = Some(DirectRunning {
            port,
            runtime,
            listener,
            relay,
            discovery,
        });
        Ok(())
    }
    pub(super) async fn stop(&self) {
        if let Some(running) = self.state.lock().await.running.take() {
            running.runtime.active.store(false, Ordering::Release);
            running.runtime.relay_online.store(false, Ordering::Release);
            if let Ok(mut invitation) = running.runtime.invitation.lock() {
                *invitation = None;
            }
            running.listener.abort();
            let _ = running.listener.await;
            if let Some(task) = running.relay {
                task.abort();
            }
            if let Some(discovery) = running.discovery {
                let _ = discovery.shutdown();
            }
        }
    }
    async fn status(&self) -> Result<ConnectionStatusV5, RemoteError> {
        let state = self.state.lock().await;
        let mac_id = state
            .config
            .as_ref()
            .map(|config| {
                ConnectionIdentity::from_private(&config.private_key)
                    .map(|identity| identity.mac_id())
                    .map_err(|_| connection_error())
            })
            .transpose()?;
        Ok(ConnectionStatusV5 {
            version: 5,
            enabled: state
                .running
                .as_ref()
                .is_some_and(|r| r.runtime.active.load(Ordering::Acquire)),
            mac_id,
            relay_origin: state.config.as_ref().and_then(|c| c.relay_origin.clone()),
            relay_online: state
                .running
                .as_ref()
                .is_some_and(|r| r.runtime.relay_online.load(Ordering::Acquire)),
            nearby_available: state
                .running
                .as_ref()
                .is_some_and(|r| r.discovery.is_some()),
            invitation: None,
            qr_rows: None,
        })
    }
    /// Issues a fresh invitation from the running identity, makes it the one
    /// enrollment accepts, and renders it with `present` under the same lock,
    /// so a concurrent stop cannot separate the invitation from its listener.
    async fn issue_invitation<T>(
        &self,
        present: impl FnOnce(ConnectionStatusV5, PairingInvitationV5, u16) -> Result<T, RemoteError>,
    ) -> Result<T, RemoteError> {
        let status = self.status().await?;
        let state = self.state.lock().await;
        let running = state.running.as_ref().ok_or_else(connection_error)?;
        let runtime = &running.runtime;
        if !runtime.active.load(Ordering::Acquire) || !runtime.http.hub.is_enabled()? {
            return Err(connection_error());
        }
        let mut bytes = [0; 32];
        OsRng
            .try_fill_bytes(&mut bytes)
            .map_err(|_| connection_error())?;
        let issued_at = now();
        let invitation = PairingInvitationV5 {
            version: 5,
            kind: InvitationKind::InterfacePairing,
            mac_id: runtime.identity.mac_id(),
            mac_name: match self.profile {
                MacProfile::Production => "Interface",
                MacProfile::Development => "Interface Dev",
            }
            .into(),
            profile: self.profile,
            public_key: runtime.identity.public_key(),
            invitation_id: hex(&bytes),
            issued_at,
            expires_at: issued_at + INVITATION_LIFETIME_SECONDS,
            relay_origin: status.relay_origin.clone(),
        };
        invitation.validate().map_err(|_| connection_error())?;
        *runtime.invitation.lock().map_err(|_| connection_error())? = Some(Invitation {
            value: invitation.clone(),
            claimant: None,
        });
        present(status, invitation, running.port)
    }

    /// The retained v5 pairing: the status carries the invitation and its QR.
    async fn pair(&self) -> Result<ConnectionStatusV5, RemoteError> {
        self.issue_invitation(|mut status, invitation, _| {
            status.qr_rows = Some(qr_rows(PAIRING_PREFIX, &invitation)?);
            status.invitation = Some(invitation);
            Ok(status)
        })
        .await
    }

    /// The v6 pairing: the same v5 invitation plus nearby routes, rendered only
    /// as the v6 QR.
    pub(super) async fn pair_direct(&self) -> Result<PairingStatusV6, RemoteError> {
        self.issue_invitation(|_, invitation, port| {
            let invitation = PairingInvitationV6 {
                version: 6,
                invitation,
                nearby_origins: nearby_origins(port),
            };
            invitation
                .validate_at(now())
                .map_err(|_| connection_error())?;
            Ok(PairingStatusV6 {
                version: 6,
                qr_rows: qr_rows(PAIRING_PREFIX_V6, &invitation)?,
                invitation,
            })
        })
        .await
    }
}

/// The QR matrix of `prefix` followed by the canonical JSON of `payload`, one
/// row of '1' (dark) and '0' (light) per line.
fn qr_rows(prefix: &str, payload: &impl Serialize) -> Result<Vec<String>, RemoteError> {
    let canonical = serde_json::to_value(payload)
        .and_then(|v| serde_json::to_string(&v))
        .map_err(|_| connection_error())?;
    let qr = QrCode::new(format!("{prefix}{canonical}")).map_err(|_| connection_error())?;
    Ok((0..qr.width())
        .map(|y| {
            (0..qr.width())
                .map(|x| if qr[(x, y)] == Color::Dark { '1' } else { '0' })
                .collect()
        })
        .collect())
}

/// UDP connect only asks the OS routing table for a source address; it sends no
/// datagram. Include the default route and local ranges to cover VPN/split routes
/// without a new dependency, network scan, DNS query or subprocess.
fn nearby_origins(port: u16) -> Vec<String> {
    let mut origins = Vec::new();
    for destination in [
        "192.0.2.1:9",
        "10.255.255.254:9",
        "172.31.255.254:9",
        "192.168.255.254:9",
        "169.254.255.254:9",
    ] {
        let Ok(socket) = std::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, 0)) else {
            continue;
        };
        if socket.connect(destination).is_err() {
            continue;
        }
        let Ok(address) = socket.local_addr() else {
            continue;
        };
        let origin = format!("http://{}:{port}", address.ip());
        if nearby_origin(&origin)
            && !origins.contains(&origin)
            && origins.len() < MAX_NEARBY_ORIGINS
        {
            origins.push(origin);
        }
    }
    origins
}
/// How often the Bonjour daemon re-reads the network interfaces for address
/// changes. Its default of 5 s woke the host all day; a Mac that changes
/// network now re-advertises within a minute, and paired phones keep their
/// saved routes meanwhile.
const BONJOUR_IP_CHECK_SECS: u32 = 60;

fn advertise(mac_id: &str, port: u16) -> Result<ServiceDaemon, ()> {
    let daemon = ServiceDaemon::new().map_err(|_| ())?;
    if daemon.disable_interface(mdns_sd::IfKind::IPv6).is_err()
        || daemon.set_ip_check_interval(BONJOUR_IP_CHECK_SECS).is_err()
    {
        let _ = daemon.shutdown();
        return Err(());
    }
    let name = format!("Interface-{}", &mac_id[..12]);
    let hostname = format!("interface-{}.local.", &mac_id[..12]);
    let service = ServiceInfo::new(
        "_interface._tcp.local.",
        &name,
        &hostname,
        "",
        port,
        &[("version", "5"), ("macId", mac_id)][..],
    )
    .map_err(|_| ())?
    .enable_addr_auto();
    if daemon.register(service).is_err() {
        let _ = daemon.shutdown();
        return Err(());
    }
    Ok(daemon)
}
/// The nearby listener's bounds: the peers it accepts at all, how many
/// connections it holds at once and from one address, how long a connection
/// may stay silent while the Mac waits for its request, and how long it has to
/// deliver that request's head.
#[derive(Clone, Copy)]
struct NearbyLimits {
    admit: fn(IpAddr) -> bool,
    connections: usize,
    per_address: usize,
    silence: Duration,
    request_head: Duration,
}
/// What a phone needs, from `apps/interface-ios`: it sends each packet as one
/// request on a fresh connection, through one `URLSession` that keeps at most
/// 4 connections to an address (the iOS default), and while it looks for the
/// Mac it probes at most 3 saved routes and 1 Bonjour hint at once: 8 at its
/// busiest, usually 4 or fewer. It gives up on a request after 15 s in all
/// (`timeoutIntervalForResource`), probes after 3 s.
///
/// So one address may hold 16 connections, twice a phone at its busiest (two
/// devices behind one address still fit), and never more than half of the 32
/// in all: someone else on the network cannot take every connection, however
/// slowly they send. A request's head gets 15 s, as long as the phone itself
/// waits for the whole request, so no request a phone is still waiting on is
/// ever cut; a body then has the perimeter's 12 s.
const NEARBY_LIMITS: NearbyLimits = NearbyLimits {
    admit: nearby_peer,
    connections: 32,
    per_address: 16,
    silence: Duration::from_secs(10),
    request_head: Duration::from_secs(15),
};
/// The nearby listener: one packet route, served with each connection's peer
/// address so the perimeter can refuse peers outside the local networks.
async fn serve_direct(listener: TcpListener, runtime: Arc<DirectRuntime>) -> std::io::Result<()> {
    serve_nearby(listener, runtime, NEARBY_LIMITS).await
}
async fn serve_nearby(
    listener: TcpListener,
    runtime: Arc<DirectRuntime>,
    limits: NearbyLimits,
) -> std::io::Result<()> {
    let router = Router::new()
        .route("/v5/packet", post(packet))
        .layer(DefaultBodyLimit::max(MAX_PACKET_BYTES))
        .layer(middleware::from_fn_with_state(
            runtime.clone(),
            direct_perimeter,
        ))
        .layer(middleware::map_response(close_connection))
        .with_state(runtime);
    let listener = NearbyListener {
        listener,
        connections: Arc::new(Semaphore::new(limits.connections)),
        addresses: Arc::default(),
        limits,
    };
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<NearbyPeer>(),
    )
    .await
}

/// Accepts only nearby peers, and only while their address and the listener
/// both have a connection free: any other socket is closed before a byte of it
/// is read.
struct NearbyListener {
    listener: TcpListener,
    connections: Arc<Semaphore>,
    addresses: Arc<StdMutex<BTreeMap<IpAddr, usize>>>,
    limits: NearbyLimits,
}
/// One connection's place in its address's count, given back when dropped.
struct AddressShare {
    addresses: Arc<StdMutex<BTreeMap<IpAddr, usize>>>,
    address: IpAddr,
}
impl AddressShare {
    fn claim(
        addresses: &Arc<StdMutex<BTreeMap<IpAddr, usize>>>,
        address: IpAddr,
        limit: usize,
    ) -> Option<Self> {
        let mut counts = addresses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let count = counts.entry(address).or_default();
        if *count >= limit {
            return None;
        }
        *count += 1;
        Some(Self {
            addresses: Arc::clone(addresses),
            address,
        })
    }
}
impl Drop for AddressShare {
    fn drop(&mut self) {
        let mut counts = self
            .addresses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(count) = counts.get_mut(&self.address) {
            *count -= 1;
            if *count == 0 {
                counts.remove(&self.address);
            }
        }
    }
}
impl axum::serve::Listener for NearbyListener {
    type Io = NearbyStream;
    type Addr = SocketAddr;
    async fn accept(&mut self) -> (NearbyStream, SocketAddr) {
        loop {
            let (stream, peer) =
                <TcpListener as axum::serve::Listener>::accept(&mut self.listener).await;
            let address = peer.ip().to_canonical();
            if !(self.limits.admit)(address) {
                continue;
            }
            let Some(share) =
                AddressShare::claim(&self.addresses, address, self.limits.per_address)
            else {
                continue;
            };
            let Ok(permit) = self.connections.clone().try_acquire_owned() else {
                continue;
            };
            return (
                NearbyStream {
                    stream,
                    silence: self.limits.silence,
                    deadline: None,
                    answering: Arc::new(AtomicBool::new(false)),
                    head: Some(Box::pin(tokio::time::sleep(self.limits.request_head))),
                    started: Arc::new(AtomicBool::new(false)),
                    _permit: permit,
                    _share: Some(share),
                },
                peer,
            );
        }
    }
    fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }
}

/// One admitted connection. It holds its connection permit and its address's
/// share for its whole life, and a read fails once the peer has sent nothing
/// for `silence` while the Mac waits for it. The wait pauses while the Mac
/// answers a request, and any byte written to the peer restarts it. A read also
/// fails once `head` has passed without the request's head arriving, however
/// steadily the peer trickles it.
struct NearbyStream {
    stream: tokio::net::TcpStream,
    silence: Duration,
    deadline: Option<Pin<Box<tokio::time::Sleep>>>,
    answering: Arc<AtomicBool>,
    /// Until `started`: the end of the time the request's head may take.
    head: Option<Pin<Box<tokio::time::Sleep>>>,
    /// Set by the perimeter once the request's head has arrived.
    started: Arc<AtomicBool>,
    _permit: tokio::sync::OwnedSemaphorePermit,
    _share: Option<AddressShare>,
}
impl AsyncRead for NearbyStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        if let Some(head) = this.head.as_mut() {
            if this.started.load(Ordering::Acquire) {
                this.head = None;
            } else if head.as_mut().poll(cx).is_ready() {
                return Poll::Ready(Err(std::io::ErrorKind::TimedOut.into()));
            }
        }
        if let Poll::Ready(read) = Pin::new(&mut this.stream).poll_read(cx, buf) {
            this.deadline = None;
            return Poll::Ready(read);
        }
        if this.answering.load(Ordering::Acquire) {
            this.deadline = None;
            return Poll::Pending;
        }
        let silence = this.silence;
        let deadline = this
            .deadline
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(silence)));
        match deadline.as_mut().poll(cx) {
            Poll::Ready(()) => Poll::Ready(Err(std::io::ErrorKind::TimedOut.into())),
            Poll::Pending => Poll::Pending,
        }
    }
}
impl AsyncWrite for NearbyStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        let written = Pin::new(&mut this.stream).poll_write(cx, buf);
        if matches!(written, Poll::Ready(Ok(1..))) {
            this.deadline = None;
        }
        written
    }
    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        let written = Pin::new(&mut this.stream).poll_write_vectored(cx, bufs);
        if matches!(written, Poll::Ready(Ok(1..))) {
            this.deadline = None;
        }
        written
    }
    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(cx)
    }
}

/// What a request knows of its connection: the peer's address, the flag that
/// pauses the connection's silence deadline while the Mac answers, and the flag
/// that ends its head deadline.
#[derive(Clone)]
struct NearbyPeer {
    address: SocketAddr,
    answering: Arc<AtomicBool>,
    started: Arc<AtomicBool>,
}
impl Connected<axum::serve::IncomingStream<'_, NearbyListener>> for NearbyPeer {
    fn connect_info(stream: axum::serve::IncomingStream<'_, NearbyListener>) -> Self {
        Self {
            address: *stream.remote_addr(),
            answering: stream.io().answering.clone(),
            started: stream.io().started.clone(),
        }
    }
}
/// Marks a connection as being answered until dropped.
struct Answering(Arc<AtomicBool>);
impl Answering {
    fn start(flag: &Arc<AtomicBool>) -> Self {
        flag.store(true, Ordering::Release);
        Self(flag.clone())
    }
}
impl Drop for Answering {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
/// Every phone packet is one request, so no connection is kept alive: an idle
/// kept-alive connection could otherwise be closed under the phone's next
/// request, which the phone must then treat as possibly delivered.
async fn close_connection(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CONNECTION, HeaderValue::from_static("close"));
    response
}

/// The networks a phone can share with this Mac: loopback, private and
/// link-local IPv4. A peer anywhere else (a Mac given a public address, or the
/// carrier-grade NAT range Tailscale assigned before its route was retired on
/// 2026-10-14) never reaches the parser; such a phone uses the relay.
fn nearby_peer(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .is_some_and(|ip| nearby_peer(IpAddr::V4(ip))),
    }
}
async fn direct_perimeter(
    State(runtime): State<Arc<DirectRuntime>>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<NearbyPeer>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    // The request's head has arrived; its body has the 12 s below.
    peer.started.store(true, Ordering::Release);
    if !nearby_peer(peer.address.ip()) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !runtime.active.load(Ordering::Acquire) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let mut response = tokio::time::timeout(Duration::from_secs(12), next.run(request))
        .await
        .unwrap_or_else(|_| StatusCode::REQUEST_TIMEOUT.into_response());
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
async fn packet(
    State(runtime): State<Arc<DirectRuntime>>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<NearbyPeer>,
    Json(packet): Json<EncryptedPacketV5>,
) -> Response {
    // The slot is taken only once the whole body has arrived: a slow upload
    // holds its own connection, never a slot the relay's packets share.
    let Ok(_permit) = runtime.requests.try_acquire() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    let _answering = Answering::start(&peer.answering);
    match runtime.handle(packet).await {
        Ok(packet) => Json(packet).into_response(),
        Err(_) => StatusCode::BAD_REQUEST.into_response(),
    }
}
impl DirectRuntime {
    async fn handle(&self, packet: EncryptedPacketV5) -> Result<EncryptedPacketV5, RemoteError> {
        if !self.active.load(Ordering::Acquire) || !self.http.hub.is_enabled()? {
            return Err(connection_error());
        }
        let id = parse_uuid(&packet.request_id)?;
        let mac_id = self.identity.mac_id();
        let plaintext = self
            .identity
            .open(&packet, &mac_id, "request", None)
            .map_err(|_| connection_error())?;
        let request: TunnelRequestV5 =
            serde_json::from_slice(&plaintext).map_err(|_| connection_error())?;
        if request.version != 5 {
            return Err(connection_error());
        }
        {
            let mut window = self.window.lock().map_err(|_| connection_error())?;
            if request.method == "GET" && request.path == "/api/v5/ping" {
                window
                    .check_freshness(id, request.issued_at, request.expires_at, now())
                    .map_err(|_| connection_error())?;
            } else {
                window
                    .admit(id, request.issued_at, request.expires_at, now())
                    .map_err(|_| connection_error())?;
            }
        }
        let reply = tokio::time::timeout(Duration::from_secs(10), self.dispatch(request))
            .await
            .unwrap_or_else(|_| StatusCode::REQUEST_TIMEOUT.into_response());
        let (parts, body) = reply.into_parts();
        let bytes = axum::body::to_bytes(body, 2 * 1024 * 1024)
            .await
            .map_err(|_| connection_error())?;
        let reply = TunnelResponseV5 {
            version: 5,
            status: parts.status.as_u16(),
            body: STANDARD.encode(bytes),
            content_type: parts
                .headers
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/octet-stream")
                .into(),
            set_cookie: parts
                .headers
                .get(header::SET_COOKIE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned),
        };
        self.identity
            .seal(
                &packet.sender_key,
                &mac_id,
                id,
                "response",
                &serde_json::to_vec(&reply).map_err(|_| connection_error())?,
            )
            .map_err(|_| connection_error())
    }
    /// The Mac's current nearby routes for the phone to save, behind the same grant.
    fn routes(&self, headers: &HeaderMap) -> Result<Response, RemoteError> {
        super::authenticate(&self.http, headers)?;
        Ok(Json(MacRoutesV1 {
            version: 1,
            origins: nearby_origins(self.port),
        })
        .into_response())
    }
    async fn dispatch(&self, request: TunnelRequestV5) -> Response {
        self.dispatch_checked(request)
            .await
            .unwrap_or_else(IntoResponse::into_response)
    }
    async fn dispatch_checked(&self, request: TunnelRequestV5) -> Result<Response, RemoteError> {
        let invalid = || error("invalid-request", "This phone request is not supported.");
        if request.path.len() > 256
            || !request.path.is_ascii()
            || request.path.contains(['%', '#', '\\'])
            || request.body.len() > 24 * 1024
        {
            return Err(invalid());
        }
        let body = STANDARD.decode(&request.body).map_err(|_| invalid())?;
        if body.len() > 16 * 1024 {
            return Err(invalid());
        }
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, HeaderValue::from_static(TUNNEL_ORIGIN));
        for (name, value) in [
            ("cookie", &request.cookie),
            ("x-interface-control", &request.control),
            ("x-interface-command", &request.command),
        ] {
            if let Some(value) = value {
                if value.len() > 128 {
                    return Err(invalid());
                }
                headers.insert(name, value.parse().map_err(|_| invalid())?);
            }
        }
        let state = State(self.http.clone());
        let method = request.method.as_str();
        let path = request.path.as_str();
        if path == "/api/v5/ping" && method == "GET" {
            return Ok(Json(serde_json::json!({"version":5})).into_response());
        }
        if path == "/api/v1/routes" && method == "GET" {
            return self.routes(&headers);
        }
        if path == "/api/v3/enrollment" && method == "POST" {
            let value: EnrollmentRequest = serde_json::from_slice(&body).map_err(|_| invalid())?;
            let mut active = self.invitation.lock().map_err(|_| connection_error())?;
            let invitation = active.as_mut().ok_or_else(invalid)?;
            if invitation.value.validate_at(now()).is_err()
                || request.invitation_id.as_deref() != Some(&invitation.value.invitation_id)
            {
                return Err(error(
                    "invitation-expired",
                    "Scan a fresh pairing QR code from your Mac.",
                ));
            }
            let digest: [u8; 32] =
                Sha256::digest(format!("{}|{}", value.id, value.secret).as_bytes()).into();
            if invitation
                .claimant
                .is_some_and(|claimant| claimant != digest)
            {
                return Err(invalid());
            }
            let reply = self.http.hub.request_enrollment(&value)?;
            invitation.claimant = Some(digest);
            return Ok(Json(reply).into_response());
        }
        if method == "POST" && path == "/api/v3/enrollment/status" {
            return Ok(enrollment_status(
                state,
                Json(serde_json::from_slice(&body).map_err(|_| invalid())?),
            )
            .await?
            .into_response());
        }
        if method == "POST" && path == "/api/v3/unlock" {
            return unlock(
                state,
                headers,
                Json(serde_json::from_slice(&body).map_err(|_| invalid())?),
            )
            .await;
        }
        if method == "POST" && path == "/api/v3/lock" {
            return Ok(lock_session(state, headers).await?.into_response());
        }
        if method == "GET" && path == "/api/v2/workspaces" {
            return Ok(workspaces(state, headers).await?.into_response());
        }
        if method == "GET" && path == "/api/v3/workspaces" {
            return Ok(super::workspaces_v3(state, headers).await?.into_response());
        }
        if method == "POST" && path == "/api/v1/voice-control" {
            return super::voice_control(
                state,
                headers,
                Json(serde_json::from_slice(&body).map_err(|_| invalid())?),
            )
            .await;
        }
        if method == "GET" && path == "/api/v1/terminals" {
            return Ok(terminals(state, headers).await?.into_response());
        }
        self.dispatch_terminal(method, path, headers, body).await
    }
    /// Roster v4 and phone Threads v1: the list, and `{engine}/{session}`
    /// with an optional `control` or `approval`; no query string.
    async fn dispatch_phone_v4(
        &self,
        method: &str,
        path: &str,
        headers: HeaderMap,
        body: Vec<u8>,
    ) -> Result<Response, RemoteError> {
        let invalid = || error("invalid-request", "This phone request is not supported.");
        let state = State(self.http.clone());
        match (method, path) {
            ("GET", "/api/v4/workspaces") => {
                return Ok(super::workspaces_v4(state, headers).await?.into_response());
            }
            ("GET", "/api/v1/threads") => {
                return Ok(super::threads(state, headers).await?.into_response());
            }
            ("GET", "/api/v2/inbox") => {
                return Ok(super::inbox(state, headers).await?.into_response());
            }
            _ => {}
        }
        let rest = path.strip_prefix("/api/v1/threads/").ok_or_else(invalid)?;
        let parts: Vec<&str> = rest.split('/').collect();
        if rest.contains('?') || !(2..=3).contains(&parts.len()) {
            return Err(invalid());
        }
        let target = Path((parts[0].to_owned(), parts[1].to_owned()));
        match (method, parts.get(2).copied()) {
            ("GET", None) => Ok(super::thread_detail(state, target, headers).await?),
            ("POST", Some("control")) => Ok(super::claim_thread(state, target, headers)
                .await?
                .into_response()),
            ("DELETE", Some("control")) => Ok(super::release_thread(state, target, headers)
                .await?
                .into_response()),
            ("POST", Some("approval")) => Ok(super::answer_thread(
                state,
                target,
                headers,
                Json(serde_json::from_slice(&body).map_err(|_| invalid())?),
            )
            .await?
            .into_response()),
            ("POST", Some("send")) => Ok(super::send_to_thread(
                state,
                target,
                headers,
                Json(serde_json::from_slice(&body).map_err(|_| invalid())?),
            )
            .await?
            .into_response()),
            _ => Err(invalid()),
        }
    }

    async fn dispatch_terminal(
        &self,
        method: &str,
        path: &str,
        headers: HeaderMap,
        body: Vec<u8>,
    ) -> Result<Response, RemoteError> {
        if path == "/api/v4/workspaces"
            || path == "/api/v2/inbox"
            || path.starts_with("/api/v1/threads")
        {
            return self.dispatch_phone_v4(method, path, headers, body).await;
        }
        let invalid = || error("invalid-request", "This phone request is not supported.");
        let state = State(self.http.clone());
        let (path, query) = path.split_once('?').unwrap_or((path, ""));
        let parts: Vec<_> = path.split('/').collect();
        if parts.len() < 7 || parts[1] != "api" || parts[3] != "terminals" {
            return Err(invalid());
        }
        let id = parts[4].to_owned();
        let generation = parts[5].parse::<u64>().map_err(|_| invalid())?;
        let target = Path((id.clone(), generation));
        match (method, parts[2], parts[6..].join("/").as_str()) {
            ("GET", "v1", "results") if query.is_empty() => {
                Ok(super::results(state, target, headers)
                    .await?
                    .into_response())
            }
            ("GET", "v1", "draft") if query.is_empty() => {
                Ok(super::read_draft(state, target, headers)
                    .await?
                    .into_response())
            }
            ("POST", "v1", "draft") if query.is_empty() => Ok(super::save_draft(
                state,
                target,
                headers,
                Json(serde_json::from_slice(&body).map_err(|_| invalid())?),
            )
            .await?
            .into_response()),
            ("GET", "v2", "history") => {
                let limit = if query.is_empty() {
                    None
                } else {
                    Some(
                        query
                            .strip_prefix("limit=")
                            .ok_or_else(invalid)?
                            .parse()
                            .map_err(|_| invalid())?,
                    )
                };
                Ok(
                    history(state, target, Query(HistoryQuery { limit }), headers)
                        .await?
                        .into_response(),
                )
            }
            ("GET", "v5", "output") => {
                let cursor = query
                    .strip_prefix("cursor=")
                    .ok_or_else(invalid)?
                    .parse()
                    .map_err(|_| invalid())?;
                authenticate(&self.http, &headers)?;
                let key = super::target(&self.http, &id, generation)?;
                let page = self.http.hub.replay.read(key, cursor).ok_or_else(invalid)?;
                authenticate(&self.http, &headers)?;
                if page.bytes.is_empty() && !page.gap {
                    Ok(StatusCode::NO_CONTENT.into_response())
                } else {
                    Ok(page.encode().into_response())
                }
            }
            ("POST", "v1", "control") if query.is_empty() => {
                Ok(claim(state, target, headers).await?.into_response())
            }
            ("DELETE", "v1", "control") if query.is_empty() => {
                Ok(release(state, target, headers).await?.into_response())
            }
            ("POST", "v1", "input") if query.is_empty() => {
                Ok(input(state, target, headers, body.into())
                    .await?
                    .into_response())
            }
            ("POST", "v4", "approval") if query.is_empty() => Ok(approval(
                state,
                target,
                headers,
                Json(serde_json::from_slice(&body).map_err(|_| invalid())?),
            )
            .await?
            .into_response()),
            ("POST", "v4", "approval/respond") if query.is_empty() => Ok(approval_response(
                state,
                target,
                headers,
                Json(serde_json::from_slice(&body).map_err(|_| invalid())?),
            )
            .await?
            .into_response()),
            _ => Err(invalid()),
        }
    }
}

/// The wait after the first failed attempt and after any session the relay
/// accepted, so a dropped connection (a network change or a wake) comes back
/// as promptly as it always has.
const RELAY_RETRY_FIRST: Duration = Duration::from_secs(3);
/// The longest wait while the relay stays unreachable or refuses the Mac.
const RELAY_RETRY_MAX: Duration = Duration::from_secs(30);

/// The wait before the next relay attempt: 3 s, then 6, 12, 24 and 30 s while
/// attempts keep failing, and 3 s again once a session reached the relay.
fn relay_retry(previous: Option<Duration>, reached_relay: bool) -> Duration {
    match previous {
        Some(previous) if !reached_relay => previous.saturating_mul(2).min(RELAY_RETRY_MAX),
        _ => RELAY_RETRY_FIRST,
    }
}

async fn relay_loop(runtime: Arc<DirectRuntime>, origin: String) {
    let address = origin.replacen("https://", "wss://", 1) + "/v5/mac";
    let mut wait = None;
    while runtime.active.load(Ordering::Acquire) {
        let _ = relay_session(runtime.clone(), &address).await;
        let next = relay_retry(wait, runtime.relay_online.swap(false, Ordering::AcqRel));
        tokio::time::sleep(next).await;
        wait = Some(next);
    }
}
async fn relay_session(runtime: Arc<DirectRuntime>, address: &str) -> Result<(), ()> {
    use tokio_tungstenite::tungstenite::{Message as WsMessage, protocol::WebSocketConfig};
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_PACKET_BYTES))
        .max_frame_size(Some(MAX_PACKET_BYTES));
    let (mut socket, _) = tokio::time::timeout(
        Duration::from_secs(10),
        tokio_tungstenite::connect_async_with_config(address, Some(config), false),
    )
    .await
    .map_err(|_| ())?
    .map_err(|_| ())?;
    let registration = RelayRegistration {
        version: 5,
        public_key: runtime.identity.public_key(),
    };
    socket
        .send(WsMessage::Text(
            serde_json::to_string(&registration).map_err(|_| ())?.into(),
        ))
        .await
        .map_err(|_| ())?;
    let challenge = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .map_err(|_| ())?
        .ok_or(())?
        .map_err(|_| ())?;
    let WsMessage::Text(challenge) = challenge else {
        return Err(());
    };
    if challenge.len() > 2048 {
        return Err(());
    }
    let challenge: EncryptedPacketV5 = serde_json::from_str(&challenge).map_err(|_| ())?;
    let proof = runtime
        .identity
        .open(
            &challenge,
            &runtime.identity.mac_id(),
            "relayChallenge",
            None,
        )
        .map_err(|_| ())?;
    if proof.len() != 32 {
        return Err(());
    }
    let proof = RelayProof {
        version: 5,
        proof: STANDARD.encode(proof),
    };
    socket
        .send(WsMessage::Text(
            serde_json::to_string(&proof).map_err(|_| ())?.into(),
        ))
        .await
        .map_err(|_| ())?;
    let ready = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .map_err(|_| ())?
        .ok_or(())?
        .map_err(|_| ())?;
    let WsMessage::Text(ready) = ready else {
        return Err(());
    };
    let ready: serde_json::Value = serde_json::from_str(&ready).map_err(|_| ())?;
    if ready != serde_json::json!({"version":5,"ready":true}) {
        return Err(());
    }
    runtime.relay_online.store(true, Ordering::Release);
    let (mut send, mut receive) = socket.split();
    let (responses, mut result) = tokio::sync::mpsc::channel(8);
    loop {
        tokio::select! {
            message = tokio::time::timeout(Duration::from_secs(50), receive.next()) => match message {
                Ok(Some(Ok(WsMessage::Text(text)))) => {
                    let packet: EncryptedPacketV5 = serde_json::from_str(&text).map_err(|_| ())?;
                    let runtime = runtime.clone(); let responses = responses.clone();
                    tokio::spawn(async move {
                        let Ok(_permit) = runtime.requests.try_acquire() else { return; };
                        if let Ok(packet) = runtime.handle(packet).await { let _ = responses.try_send(packet); }
                    });
                },
                Ok(Some(Ok(WsMessage::Ping(bytes)))) => { send.send(WsMessage::Pong(bytes)).await.map_err(|_| ())?; },
                Ok(Some(Ok(WsMessage::Pong(_)))) => {},
                _ => return Err(()),
            },
            packet = result.recv() => {
                let packet = packet.ok_or(())?;
                let text = serde_json::to_string(&packet).map_err(|_| ())?;
                tokio::time::timeout(Duration::from_secs(3), send.send(WsMessage::Text(text.into()))).await.map_err(|_| ())?.map_err(|_| ())?;
            }
        }
    }
}

#[cfg(test)]
#[path = "direct_tests.rs"]
mod tests;
