//! A bounded, single-owner relay. Only ciphertext crosses this process.
use crate::connection_crypto::{ConnectionIdentity, constant_time_eq, hex};
use crate::relay_admission::{Waiting, displaced};
use axum::{
    Extension, Json, Router,
    extract::{
        ConnectInfo, DefaultBodyLimit, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use interface_contracts::remote_connection::{EncryptedPacketV5, MAX_PACKET_BYTES, lowercase_hex};
use rand_core::{OsRng, TryRngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, Semaphore, mpsc, oneshot};
use uuid::Uuid;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayRegistration {
    pub version: u8,
    pub public_key: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayProof {
    pub version: u8,
    pub proof: String,
}

struct ConnectedMac {
    id: Uuid,
    send: mpsc::Sender<EncryptedPacketV5>,
    replaced: oneshot::Sender<()>,
}
struct RelayState {
    allowed_mac: String,
    mac: Mutex<Option<ConnectedMac>>,
    pending: Mutex<BTreeMap<String, oneshot::Sender<EncryptedPacketV5>>>,
    handlers: Semaphore,
    /// Requests each client address has in `handlers`.
    handler_addresses: StdMutex<BTreeMap<String, usize>>,
    /// Sockets still proving they are the Mac (`relay_admission`).
    waiting: StdMutex<WaitingSockets>,
}

/// Requests the relay works on at once, and from one client address. A phone
/// sends through one `URLSession`, which keeps at most 4 connections to the
/// relay, plus a route probe; a forwarded request holds its place while it
/// waits up to 12 s for the Mac. So 16 per address is three times a phone at
/// its busiest (two devices behind one address still fit), and one address
/// uploading slowly can take at most half of the 32.
const HANDLERS: usize = 32;
const HANDLERS_PER_ADDRESS: usize = 16;

/// One request's place in its address's count, given back when dropped.
struct HandlerShare {
    state: Arc<RelayState>,
    address: String,
}
impl HandlerShare {
    fn claim(state: &Arc<RelayState>, address: &str) -> Option<Self> {
        let mut counts = state
            .handler_addresses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let count = counts.entry(address.to_owned()).or_default();
        if *count >= HANDLERS_PER_ADDRESS {
            return None;
        }
        *count += 1;
        Some(Self {
            state: Arc::clone(state),
            address: address.to_owned(),
        })
    }
}
impl Drop for HandlerShare {
    fn drop(&mut self) {
        let mut counts = self
            .state
            .handler_addresses
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

/// A poisoned lock still holds a consistent list: every change is one push or remove.
fn lock(waiting: &StdMutex<WaitingSockets>) -> std::sync::MutexGuard<'_, WaitingSockets> {
    waiting
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The sockets still proving they are the Mac, oldest first.
#[derive(Default)]
struct WaitingSockets {
    next: u64,
    sockets: Vec<WaitingSocket>,
}
struct WaitingSocket {
    id: u64,
    address: Option<String>,
    named: Arc<AtomicBool>,
    displace: oneshot::Sender<()>,
}
/// One socket's place among the waiting ones, from its arrival until it has
/// proved the Mac's key or failed to. Dropping it gives the place up.
struct Arrival {
    state: Arc<RelayState>,
    id: u64,
    /// Set once the socket's first message names the allowed Mac.
    named: Arc<AtomicBool>,
    /// Fires when a newer socket takes this one's place.
    displaced: oneshot::Receiver<()>,
}
impl Drop for Arrival {
    fn drop(&mut self) {
        lock(&self.state.waiting)
            .sockets
            .retain(|socket| socket.id != self.id);
    }
}
impl RelayState {
    /// Makes room for a new socket from `address`: it is never refused, and
    /// it displaces at most one older waiting socket (`relay_admission`).
    fn arrive(self: &Arc<Self>, address: Option<String>) -> Arrival {
        let mut waiting = lock(&self.waiting);
        let listed: Vec<Waiting<'_>> = waiting
            .sockets
            .iter()
            .map(|socket| Waiting {
                address: socket.address.as_deref(),
                named: socket.named.load(Ordering::Acquire),
            })
            .collect();
        if let Some(index) = displaced(&listed, address.as_deref()) {
            let _ = waiting.sockets.remove(index).displace.send(());
        }
        waiting.next += 1;
        let id = waiting.next;
        let named = Arc::new(AtomicBool::new(false));
        let (displace, displaced) = oneshot::channel();
        waiting.sockets.push(WaitingSocket {
            id,
            address,
            named: Arc::clone(&named),
            displace,
        });
        Arrival {
            state: Arc::clone(self),
            id,
            named,
            displaced,
        }
    }
}

/// The client address a request came from, when the relay can tell. Behind
/// Fly's proxy every connection comes from the proxy, which names the client
/// in `Fly-Client-IP`; that header is believed only from a peer on a private
/// network (the proxy), never from a peer on the internet, who could write it.
#[derive(Clone)]
struct ClientAddress(Option<String>);
fn client_address(request: &axum::extract::Request) -> ClientAddress {
    let Some(ConnectInfo(peer)) = request.extensions().get::<ConnectInfo<SocketAddr>>() else {
        return ClientAddress(None);
    };
    let peer = peer.ip().to_canonical();
    let forwarded = request
        .headers()
        .get("fly-client-ip")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<IpAddr>().ok())
        .filter(|_| private_peer(peer));
    ClientAddress(Some(forwarded.unwrap_or(peer).to_canonical().to_string()))
}
fn private_peer(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        IpAddr::V6(ip) => ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local(),
    }
}

/// # Errors
/// Refuses to run without an explicit public Mac identity allowlist entry.
pub fn relay_router(allowed_mac: String) -> Result<Router, &'static str> {
    if !lowercase_hex(&allowed_mac, 32) {
        return Err("A 64-character public Mac identity is required.");
    }
    let state = Arc::new(RelayState {
        allowed_mac,
        mac: Mutex::new(None),
        pending: Mutex::new(BTreeMap::new()),
        handlers: Semaphore::new(HANDLERS),
        handler_addresses: StdMutex::new(BTreeMap::new()),
        waiting: StdMutex::new(WaitingSockets::default()),
    });
    Ok(Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/v5/mac", get(register))
        .route("/v5/packet", post(forward))
        .layer(DefaultBodyLimit::max(MAX_PACKET_BYTES))
        .layer(middleware::from_fn_with_state(state.clone(), perimeter))
        .with_state(state))
}
async fn perimeter(
    State(state): State<Arc<RelayState>>,
    mut request: axum::extract::Request,
    next: Next,
) -> Response {
    let address = client_address(&request);
    // The Mac's registration never waits on other requests: it is answered at
    // once, and the socket it opens has its own room (`relay_admission`).
    // Everything else shares the handlers, and one address only its part.
    let _held = if request.uri().path() == "/v5/mac" {
        None
    } else {
        let share = match address.0.as_deref() {
            Some(client) => match HandlerShare::claim(&state, client) {
                Some(share) => Some(share),
                None => return StatusCode::TOO_MANY_REQUESTS.into_response(),
            },
            None => None,
        };
        let Ok(permit) = state.handlers.try_acquire() else {
            return StatusCode::TOO_MANY_REQUESTS.into_response();
        };
        Some((share, permit))
    };
    request.extensions_mut().insert(address);
    // No cookies, CORS, credentials, HTML, request bodies or contents are logged.
    let mut response = tokio::time::timeout(Duration::from_secs(15), next.run(request))
        .await
        .unwrap_or_else(|_| StatusCode::GATEWAY_TIMEOUT.into_response());
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}
async fn register(
    State(state): State<Arc<RelayState>>,
    Extension(ClientAddress(address)): Extension<ClientAddress>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let arrival = state.arrive(address);
    upgrade
        .max_message_size(MAX_PACKET_BYTES)
        .max_frame_size(MAX_PACKET_BYTES)
        .on_upgrade(move |socket| async move {
            mac_socket(socket, state, arrival).await;
        })
}
async fn receive_json<T: serde::de::DeserializeOwned>(
    socket: &mut WebSocket,
    max: usize,
) -> Option<T> {
    let message = tokio::time::timeout(Duration::from_secs(5), socket.recv())
        .await
        .ok()??
        .ok()?;
    let Message::Text(text) = message else {
        return None;
    };
    if text.len() > max {
        return None;
    }
    serde_json::from_str(&text).ok()
}
async fn send_json<T: Serialize>(socket: &mut WebSocket, value: &T) -> bool {
    let Ok(text) = serde_json::to_string(value) else {
        return false;
    };
    matches!(
        tokio::time::timeout(
            Duration::from_secs(3),
            socket.send(Message::Text(text.into()))
        )
        .await,
        Ok(Ok(()))
    )
}
async fn prove_mac(socket: &mut WebSocket, allowed: &str, named: &AtomicBool) -> bool {
    let Some(registration) = receive_json::<RelayRegistration>(socket, 1024).await else {
        return false;
    };
    let Ok(bytes) = crate::connection_crypto::unhex(&registration.public_key, 65) else {
        return false;
    };
    if registration.version != 5 || hex(&Sha256::digest(bytes)) != allowed {
        return false;
    }
    named.store(true, Ordering::Release);
    let Ok(identity) = ConnectionIdentity::generate() else {
        return false;
    };
    let mut challenge = [0; 32];
    if OsRng.try_fill_bytes(&mut challenge).is_err() {
        return false;
    }
    let Ok(packet) = identity.seal(
        &registration.public_key,
        allowed,
        Uuid::new_v4(),
        "relayChallenge",
        &challenge,
    ) else {
        return false;
    };
    if !send_json(socket, &packet).await {
        return false;
    }
    let Some(proof) = receive_json::<RelayProof>(socket, 1024).await else {
        return false;
    };
    let Ok(proof_bytes) = STANDARD.decode(proof.proof) else {
        return false;
    };
    proof.version == 5 && constant_time_eq(&proof_bytes, &challenge)
}
async fn mac_socket(mut socket: WebSocket, state: Arc<RelayState>, mut arrival: Arrival) {
    let proved = tokio::select! {
        proved = prove_mac(&mut socket, &state.allowed_mac, &arrival.named) => proved,
        _ = &mut arrival.displaced => false,
    };
    // Proved or not, the socket is no longer waiting.
    drop(arrival);
    if !proved {
        return;
    }
    let id = Uuid::new_v4();
    let (send, mut receive) = mpsc::channel(8);
    let (replaced, mut replaced_rx) = oneshot::channel();
    {
        let mut mac = state.mac.lock().await;
        if let Some(previous) = mac.take() {
            let _ = previous.replaced.send(());
            state.pending.lock().await.clear();
        }
        *mac = Some(ConnectedMac { id, send, replaced });
    }
    if !send_json(&mut socket, &serde_json::json!({"version":5,"ready":true})).await {
        disconnect(&state, id).await;
        return;
    }
    let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
    let mut last_seen = tokio::time::Instant::now();
    loop {
        tokio::select! {
            _ = &mut replaced_rx => {
                let _ = socket.send(Message::Close(None)).await;
                break;
            },
            message = socket.recv() => match message {
                Some(Ok(Message::Text(text))) => {
                    last_seen = tokio::time::Instant::now();
                    let Ok(packet) = serde_json::from_str::<EncryptedPacketV5>(&text) else { break; };
                    if packet.version != 5 || packet.mac_id != state.allowed_mac { break; }
                    if let Some(waiter) = state.pending.lock().await.remove(&packet.request_id) { let _ = waiter.send(packet); }
                },
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => { last_seen = tokio::time::Instant::now(); },
                _ => break,
            },
            packet = receive.recv() => {
                let Some(packet) = packet else { break; };
                if !send_json(&mut socket, &packet).await { break; }
            },
            _ = heartbeat.tick() => {
                if last_seen.elapsed() > Duration::from_secs(45) { break; }
                if !matches!(tokio::time::timeout(Duration::from_secs(3), socket.send(Message::Ping(Vec::new().into()))).await, Ok(Ok(()))) { break; }
            }
        }
    }
    disconnect(&state, id).await;
}
async fn disconnect(state: &RelayState, id: Uuid) {
    let mut mac = state.mac.lock().await;
    if mac.as_ref().is_some_and(|active| active.id == id) {
        *mac = None;
        state.pending.lock().await.clear();
    }
}
async fn forward(
    State(state): State<Arc<RelayState>>,
    Json(packet): Json<EncryptedPacketV5>,
) -> Response {
    if packet.version != 5
        || packet.mac_id != state.allowed_mac
        || Uuid::parse_str(&packet.request_id).is_err()
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Some(send) = state.mac.lock().await.as_ref().map(|mac| mac.send.clone()) else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let id = packet.request_id.clone();
    let (sender, receiver) = oneshot::channel();
    {
        let mut pending = state.pending.lock().await;
        if pending.len() >= 8 {
            return StatusCode::TOO_MANY_REQUESTS.into_response();
        }
        if pending.contains_key(&id) {
            return StatusCode::CONFLICT.into_response();
        }
        pending.insert(id.clone(), sender);
    }
    if send.try_send(packet).is_err() {
        state.pending.lock().await.remove(&id);
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let response = tokio::time::timeout(Duration::from_secs(12), receiver).await;
    state.pending.lock().await.remove(&id);
    match response {
        Ok(Ok(packet)) => Json(packet).into_response(),
        // Once forwarded, relay failure cannot establish that a write was unsent.
        _ => StatusCode::GATEWAY_TIMEOUT.into_response(),
    }
}
