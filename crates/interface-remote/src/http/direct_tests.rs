use super::*;
use crate::RemoteHub;
use crate::TrustPersistence;
use interface_contracts::{
    remote_trust::{RemoteTrustData, TrustCommand},
    terminal::TerminalSnapshot,
};
use std::sync::atomic::AtomicUsize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

#[derive(Default)]
struct TrustStore(StdMutex<RemoteTrustData>);
impl TrustPersistence for TrustStore {
    fn load(&self) -> Result<RemoteTrustData, RemoteError> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn save(&self, value: &RemoteTrustData) -> Result<(), RemoteError> {
        *self.0.lock().unwrap() = value.clone();
        Ok(())
    }
}
struct Backend {
    writes: AtomicUsize,
}
impl super::super::RemoteBackend for Backend {
    fn snapshot(&self) -> Result<TerminalSnapshot, RemoteError> {
        Ok(TerminalSnapshot {
            revision: 1,
            terminals: vec![crate::workspaces::tests::terminal()],
        })
    }
    fn enqueue(&self, _: crate::replay::StreamKey, _: Vec<u8>) -> Result<(), RemoteError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
fn fixture() -> (Arc<DirectRuntime>, Arc<Backend>) {
    fixture_with(ConnectionIdentity::generate().unwrap())
}
fn fixture_with(identity: ConnectionIdentity) -> (Arc<DirectRuntime>, Arc<Backend>) {
    let hub = Arc::new(RemoteHub::default());
    hub.configure_trust(Arc::new(TrustStore::default()))
        .unwrap();
    hub.activate("http://127.0.0.1:4321".into(), None).unwrap();
    let backend = Arc::new(Backend {
        writes: AtomicUsize::new(0),
    });
    let runtime = Arc::new(DirectRuntime {
        identity,
        port: 58747,
        http: Arc::new(HttpState {
            hub,
            backend: backend.clone(),
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
    (runtime, backend)
}

#[derive(Default)]
struct ConnectionStore {
    config: StdMutex<Option<ConnectionConfigurationV1>>,
    listener: StdMutex<Option<ConnectionListenerV1>>,
    saves: AtomicUsize,
}
impl ConnectionPersistence for ConnectionStore {
    fn load(&self) -> Result<Option<ConnectionConfigurationV1>, RemoteError> {
        Ok(self.config.lock().unwrap().clone())
    }
    fn save(&self, value: &ConnectionConfigurationV1) -> Result<(), RemoteError> {
        self.saves.fetch_add(1, Ordering::SeqCst);
        *self.config.lock().unwrap() = Some(value.clone());
        Ok(())
    }
    fn load_listener(&self) -> Result<Option<ConnectionListenerV1>, RemoteError> {
        Ok(self.listener.lock().unwrap().clone())
    }
    fn save_listener(&self, value: &ConnectionListenerV1) -> Result<(), RemoteError> {
        self.saves.fetch_add(1, Ordering::SeqCst);
        *self.listener.lock().unwrap() = Some(value.clone());
        Ok(())
    }
}

#[tokio::test]
async fn direct_port_and_identity_survive_restart_and_port_conflicts_fail_closed() {
    let (runtime, backend) = fixture();
    let server = RemoteServer::new(runtime.http.hub.clone(), backend, std::path::PathBuf::new());
    let store = Arc::new(ConnectionStore::default());
    let controller = DirectController::new(store.clone(), MacProfile::Development).unwrap();
    controller.enable(&server, None).await.unwrap();
    let first = controller.pair_direct().await.unwrap();
    first.invitation.validate_at(now()).unwrap();
    let port = store.listener.lock().unwrap().as_ref().unwrap().port;
    assert!(port >= 1024);
    for origin in &first.invitation.nearby_origins {
        assert!(origin.ends_with(&format!(":{port}")));
        assert!(nearby_origin(origin));
    }
    let identity = first.invitation.invitation.mac_id;
    controller.stop().await;
    drop(controller);
    let restored = DirectController::new(store.clone(), MacProfile::Development).unwrap();
    let saves = store.saves.load(Ordering::SeqCst);
    restored.restore(&server).await.unwrap();
    assert_eq!(
        restored.state.lock().await.running.as_ref().unwrap().port,
        port
    );
    assert_eq!(
        store.saves.load(Ordering::SeqCst),
        saves,
        "restoring the saved port and identity rewrites nothing"
    );
    assert_eq!(
        restored.status().await.unwrap().mac_id.as_deref(),
        Some(identity.as_str())
    );
    restored.stop().await;
    let conflict = TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, port))
        .await
        .unwrap();
    assert_eq!(
        restored.restore(&server).await.unwrap_err().code,
        "connection-port-unavailable"
    );
    assert!(!restored.status().await.unwrap().enabled);
    assert!(restored.pair_direct().await.is_err());
    assert_eq!(store.listener.lock().unwrap().as_ref().unwrap().port, port);
    drop(conflict);
    restored.restore(&server).await.unwrap();
    restored.stop().await;
}
fn request(method: &str, path: &str, body: &serde_json::Value) -> TunnelRequestV5 {
    let issued_at = now();
    TunnelRequestV5 {
        version: 5,
        issued_at,
        expires_at: issued_at + 90,
        method: method.into(),
        path: path.into(),
        body: STANDARD.encode(serde_json::to_vec(body).unwrap()),
        cookie: None,
        control: None,
        command: None,
        invitation_id: None,
    }
}
fn encrypted(
    runtime: &DirectRuntime,
    phone: &ConnectionIdentity,
    request: &TunnelRequestV5,
) -> EncryptedPacketV5 {
    phone
        .seal(
            &runtime.identity.public_key(),
            &runtime.identity.mac_id(),
            Uuid::new_v4(),
            "request",
            &serde_json::to_vec(request).unwrap(),
        )
        .unwrap()
}
fn decode(
    runtime: &DirectRuntime,
    phone: &ConnectionIdentity,
    packet: &EncryptedPacketV5,
) -> TunnelResponseV5 {
    serde_json::from_slice(
        &phone
            .open(
                packet,
                &runtime.identity.mac_id(),
                "response",
                Some(&runtime.identity.public_key()),
            )
            .unwrap(),
    )
    .unwrap()
}
async fn exchange(
    runtime: &DirectRuntime,
    phone: &ConnectionIdentity,
    request: &TunnelRequestV5,
) -> TunnelResponseV5 {
    decode(
        runtime,
        phone,
        &runtime
            .handle(encrypted(runtime, phone, request))
            .await
            .unwrap(),
    )
}
fn reply_json(response: &TunnelResponseV5) -> serde_json::Value {
    serde_json::from_slice(&STANDARD.decode(&response.body).unwrap()).unwrap()
}

/// The Mac's route refresh needs the same grant and sends only valid routes.
async fn routes_answer_with_the_same_grant(
    runtime: &DirectRuntime,
    phone: &ConnectionIdentity,
    cookie: &str,
) {
    let mut routes = request("GET", "/api/v1/routes", &serde_json::Value::Null);
    assert_eq!(exchange(runtime, phone, &routes).await.status, 401);
    routes.cookie = Some(cookie.to_owned());
    let reply: MacRoutesV1 =
        serde_json::from_value(reply_json(&exchange(runtime, phone, &routes).await)).unwrap();
    assert!(reply.valid());
    assert!(
        reply
            .origins
            .iter()
            .all(|origin| origin.ends_with(":58747"))
    );
}

/// Roster v3 answers through the same encrypted route and grant as v2.
async fn roster_v3_answers_with_the_same_grant(
    runtime: &DirectRuntime,
    phone: &ConnectionIdentity,
    cookie: &str,
) {
    let mut projects = request("GET", "/api/v3/workspaces", &serde_json::Value::Null);
    assert_eq!(exchange(runtime, phone, &projects).await.status, 401);
    projects.cookie = Some(cookie.to_owned());
    let v3 = reply_json(&exchange(runtime, phone, &projects).await);
    assert_eq!(v3["version"], 3);
    assert!(v3["projects"].is_object());
    routes_answer_with_the_same_grant(runtime, phone, cookie).await;
    phone_v4_answers_with_the_same_grant(runtime, phone, cookie).await;
}

/// Roster v4 and phone Threads answer through the same route and grant, and
/// only their exact paths reach a handler.
async fn phone_v4_answers_with_the_same_grant(
    runtime: &DirectRuntime,
    phone: &ConnectionIdentity,
    cookie: &str,
) {
    let mut roster = request("GET", "/api/v4/workspaces", &serde_json::Value::Null);
    assert_eq!(exchange(runtime, phone, &roster).await.status, 401);
    roster.cookie = Some(cookie.to_owned());
    assert_eq!(
        reply_json(&exchange(runtime, phone, &roster).await)["version"],
        4
    );
    let code = |response: &TunnelResponseV5| reply_json(response)["code"].clone();
    let mut inbox = request("GET", "/api/v2/inbox", &serde_json::Value::Null);
    assert_eq!(exchange(runtime, phone, &inbox).await.status, 401);
    inbox.cookie = Some(cookie.to_owned());
    assert_eq!(
        code(&exchange(runtime, phone, &inbox).await),
        "threads-unavailable"
    );
    let mut list = request("GET", "/api/v1/threads", &serde_json::Value::Null);
    assert_eq!(exchange(runtime, phone, &list).await.status, 401);
    list.cookie = Some(cookie.to_owned());
    // This fixture Mac has no Threads: the handler, not the router, says so.
    assert_eq!(
        code(&exchange(runtime, phone, &list).await),
        "threads-unavailable"
    );
    for (method, path) in [
        ("GET", "/api/v1/threadsx"),
        ("POST", "/api/v2/inbox"),
        ("GET", "/api/v2/inbox?all=true"),
        ("GET", "/api/v1/threads/claude"),
        ("GET", "/api/v1/threads/claude/a?limit=1"),
        ("GET", "/api/v1/threads/claude/a/approval"),
        ("POST", "/api/v1/threads/claude/a/input"),
        ("POST", "/api/v4/workspaces"),
    ] {
        let mut refused = request(method, path, &serde_json::Value::Null);
        refused.cookie = Some(cookie.to_owned());
        assert_eq!(
            code(&exchange(runtime, phone, &refused).await),
            "invalid-request",
            "{method} {path}"
        );
    }
}

#[tokio::test]
async fn encrypted_gateway_requires_invitation_mac_approval_and_current_grants_without_replaying_input()
 {
    let (runtime, backend) = fixture();
    let phone = ConnectionIdentity::generate().unwrap();
    let id = Uuid::new_v4().to_string();
    let secret = "ab".repeat(32);
    let enroll_body =
        serde_json::json!({"version":3,"id":id,"name":"Fixture iPhone","secret":secret});
    let mut enrollment = request("POST", "/api/v3/enrollment", &enroll_body);
    assert_ne!(exchange(&runtime, &phone, &enrollment).await.status, 200);
    let issued_at = now();
    *runtime.invitation.lock().unwrap() = Some(Invitation {
        value: PairingInvitationV5 {
            version: 5,
            kind: InvitationKind::InterfacePairing,
            mac_id: runtime.identity.mac_id(),
            mac_name: "Fixture".into(),
            profile: MacProfile::Production,
            public_key: runtime.identity.public_key(),
            invitation_id: "cd".repeat(32),
            issued_at,
            expires_at: issued_at + 300,
            relay_origin: None,
        },
        claimant: None,
    });
    enrollment.invitation_id = Some("cd".repeat(32));
    let enrolled = exchange(&runtime, &phone, &enrollment).await;
    assert_eq!(enrolled.status, 200);
    assert_eq!(reply_json(&enrolled)["approved"], false);
    let mut other = request(
        "POST",
        "/api/v3/enrollment",
        &serde_json::json!({"version":3,"id":Uuid::new_v4(),"name":"Other","secret":"ef".repeat(32)}),
    );
    other.invitation_id = enrollment.invitation_id.clone();
    assert_ne!(exchange(&runtime, &phone, &other).await.status, 200);
    let unlock = request(
        "POST",
        "/api/v3/unlock",
        &serde_json::json!({"version":3,"id":id,"secret":secret}),
    );
    assert_eq!(exchange(&runtime, &phone, &unlock).await.status, 401);
    runtime
        .http
        .hub
        .manage_trust(3, TrustCommand::Approve { id: id.clone() })
        .unwrap();
    let grant = exchange(&runtime, &phone, &unlock).await;
    assert_eq!(grant.status, 200);
    let cookie = grant
        .set_cookie
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let mut roster = request("GET", "/api/v2/workspaces", &serde_json::Value::Null);
    assert_eq!(exchange(&runtime, &phone, &roster).await.status, 401);
    roster.cookie = Some(cookie.clone());
    assert_eq!(
        reply_json(&exchange(&runtime, &phone, &roster).await)["version"],
        2
    );
    roster_v3_answers_with_the_same_grant(&runtime, &phone, &cookie).await;
    let terminal = crate::workspaces::tests::terminal();
    let target = format!(
        "/api/v1/terminals/{}/{}/",
        terminal.id, terminal.generation.0
    );
    let mut claim = request(
        "POST",
        &(target.clone() + "control"),
        &serde_json::Value::Null,
    );
    claim.cookie = Some(cookie.clone());
    let lease = reply_json(&exchange(&runtime, &phone, &claim).await)["lease"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut write = request("POST", &(target + "input"), &serde_json::Value::Null);
    write.body = STANDARD.encode(b"fixture reply\r");
    write.cookie = Some(cookie);
    write.control = Some(lease);
    write.command = Some(Uuid::new_v4().to_string());
    let packet = encrypted(&runtime, &phone, &write);
    let duplicate: EncryptedPacketV5 =
        serde_json::from_value(serde_json::to_value(&packet).unwrap()).unwrap();
    assert_eq!(
        decode(&runtime, &phone, &runtime.handle(packet).await.unwrap()).status,
        202
    );
    assert!(runtime.handle(duplicate).await.is_err());
    assert_eq!(exchange(&runtime, &phone, &write).await.status, 202);
    assert_eq!(backend.writes.load(Ordering::SeqCst), 1);
    runtime
        .http
        .hub
        .manage_trust(3, TrustCommand::Forget { id })
        .unwrap();
    assert_eq!(exchange(&runtime, &phone, &roster).await.status, 401);
    assert_eq!(exchange(&runtime, &phone, &write).await.status, 401);
}

async fn post_packet(address: std::net::SocketAddr, packet: &EncryptedPacketV5) -> (u16, Vec<u8>) {
    let body = serde_json::to_vec(packet).unwrap();
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    let headers = format!(
        "POST /v5/packet HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(headers.as_bytes()).await.unwrap();
    stream.write_all(&body).await.unwrap();
    // Read to the declared length: not every server closes on `Connection: close`.
    let mut reply = Vec::new();
    let mut chunk = [0; 8192];
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Some(split) = reply.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                let head = std::str::from_utf8(&reply[..split])
                    .unwrap()
                    .to_ascii_lowercase();
                let length = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .map(|value| value.trim().parse::<usize>().unwrap());
                if length.is_some_and(|length| reply.len() >= split + 4 + length) {
                    break;
                }
            }
            let read = stream.read(&mut chunk).await.unwrap();
            if read == 0 {
                break;
            }
            reply.extend_from_slice(&chunk[..read]);
        }
    })
    .await
    .unwrap();
    let split = reply
        .windows(4)
        .position(|bytes| bytes == b"\r\n\r\n")
        .unwrap();
    let status = std::str::from_utf8(&reply[..split])
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    (status, reply[split + 4..].to_vec())
}
#[tokio::test]
async fn real_relay_socket_requires_mac_key_proof_and_forwards_only_authenticated_ciphertext() {
    let (runtime, _) = fixture();
    let phone = ConnectionIdentity::generate().unwrap();
    let relay = crate::relay::relay_router(runtime.identity.mac_id()).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, relay).await.unwrap();
    });
    let ping = request("GET", "/api/v5/ping", &serde_json::Value::Null);
    assert_eq!(
        post_packet(address, &encrypted(&runtime, &phone, &ping))
            .await
            .0,
        503
    );
    // Knowing the allowlisted public key is insufficient to register a Mac.
    let (mut impostor, _) = tokio_tungstenite::connect_async(format!("ws://{address}/v5/mac"))
        .await
        .unwrap();
    impostor
        .send(Message::Text(
            serde_json::to_string(&RelayRegistration {
                version: 5,
                public_key: runtime.identity.public_key(),
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();
    assert!(matches!(impostor.next().await, Some(Ok(Message::Text(_)))));
    impostor
        .send(Message::Text(
            serde_json::to_string(&RelayProof {
                version: 5,
                proof: STANDARD.encode([0; 32]),
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();
    assert!(!matches!(
        tokio::time::timeout(Duration::from_secs(2), impostor.next()).await,
        Ok(Some(Ok(Message::Text(_))))
    ));
    assert_eq!(
        post_packet(address, &encrypted(&runtime, &phone, &ping))
            .await
            .0,
        503
    );
    let clone = runtime.clone();
    let mac =
        tokio::spawn(async move { relay_session(clone, &format!("ws://{address}/v5/mac")).await });
    for _ in 0..100 {
        if runtime.relay_online.load(Ordering::Acquire) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(runtime.relay_online.load(Ordering::Acquire));
    let (status, bytes) = post_packet(address, &encrypted(&runtime, &phone, &ping)).await;
    assert_eq!(status, 200);
    let packet: EncryptedPacketV5 = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        reply_json(&decode(&runtime, &phone, &packet)),
        serde_json::json!({"version":5})
    );
    let roster = request("GET", "/api/v2/workspaces", &serde_json::Value::Null);
    let (status, bytes) = post_packet(address, &encrypted(&runtime, &phone, &roster)).await;
    assert_eq!(status, 200);
    let packet: EncryptedPacketV5 = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(decode(&runtime, &phone, &packet).status, 401);
    mac.abort();
    server.abort();
}

/// The Mac identity (private key 7) that `deploy/phone-relay-worker/local-check.sh`
/// allows on the local relay Worker.
const RELAY_WORKER_TEST_MAC: &str =
    "b33e4d1c20df27891225e731ac50234b2a202ec4442427bfbae62ca6a6f393b4";

/// The real Mac relay session against the Cloudflare relay Worker running in
/// Cloudflare's local runtime. Takes about 70 s: the Mac must outlive its 50 s
/// read deadline on the Worker's keepalives alone.
#[tokio::test]
#[ignore = "needs the local relay Worker: deploy/phone-relay-worker/local-check.sh"]
async fn relay_worker_carries_the_real_mac_session() {
    let address: std::net::SocketAddr = std::env::var("INTERFACE_RELAY_WORKER")
        .expect("INTERFACE_RELAY_WORKER=127.0.0.1:<port>")
        .parse()
        .unwrap();
    let (runtime, _) =
        fixture_with(ConnectionIdentity::from_private(&format!("{:064x}", 7)).unwrap());
    assert_eq!(runtime.identity.mac_id(), RELAY_WORKER_TEST_MAC);
    let phone = ConnectionIdentity::generate().unwrap();
    let ping = request("GET", "/api/v5/ping", &serde_json::Value::Null);
    let ping_reaches_mac = || async {
        let (status, bytes) = post_packet(address, &encrypted(&runtime, &phone, &ping)).await;
        assert_eq!(status, 200);
        let packet: EncryptedPacketV5 = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            reply_json(&decode(&runtime, &phone, &packet)),
            serde_json::json!({"version":5})
        );
    };
    let online = |runtime: Arc<DirectRuntime>| async move {
        for _ in 0..250 {
            if runtime.relay_online.load(Ordering::Acquire) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the Mac never reached the relay");
    };
    let session = |runtime: Arc<DirectRuntime>| {
        tokio::spawn(async move { relay_session(runtime, &format!("ws://{address}/v5/mac")).await })
    };
    assert_eq!(
        post_packet(address, &encrypted(&runtime, &phone, &ping))
            .await
            .0,
        503
    );
    // Knowing the allowlisted public key is insufficient to register a Mac.
    let (mut impostor, _) = tokio_tungstenite::connect_async(format!("ws://{address}/v5/mac"))
        .await
        .unwrap();
    for message in [
        serde_json::to_string(&RelayRegistration {
            version: 5,
            public_key: runtime.identity.public_key(),
        }),
        serde_json::to_string(&RelayProof {
            version: 5,
            proof: STANDARD.encode([0; 32]),
        }),
    ] {
        impostor
            .send(Message::Text(message.unwrap().into()))
            .await
            .unwrap();
        assert!(
            !message_is_ready(&mut impostor).await,
            "an impostor registered"
        );
    }
    assert_eq!(
        post_packet(address, &encrypted(&runtime, &phone, &ping))
            .await
            .0,
        503
    );

    let first = session(runtime.clone());
    online(runtime.clone()).await;
    ping_reaches_mac().await;
    let roster = request("GET", "/api/v2/workspaces", &serde_json::Value::Null);
    let (status, bytes) = post_packet(address, &encrypted(&runtime, &phone, &roster)).await;
    assert_eq!(status, 200);
    let packet: EncryptedPacketV5 = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(decode(&runtime, &phone, &packet).status, 401);

    // Only the Worker's keepalives can carry the Mac past its read deadline.
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert!(!first.is_finished(), "the Mac left a silent relay");
    ping_reaches_mac().await;

    // A reconnect (a network change) replaces the old socket at once.
    runtime.relay_online.store(false, Ordering::Release);
    let second = session(runtime.clone());
    online(runtime.clone()).await;
    assert!(
        tokio::time::timeout(Duration::from_secs(5), first)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    ping_reaches_mac().await;
    second.abort();
}
/// Reads the relay's answer to one registration step: whatever text arrives
/// before the relay closes the socket or 2 s pass.
async fn message_is_ready<S>(socket: &mut S) -> bool
where
    S: futures::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    match tokio::time::timeout(Duration::from_secs(2), socket.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => {
            serde_json::from_str::<serde_json::Value>(&text).ok()
                == Some(serde_json::json!({"version":5,"ready":true}))
        }
        _ => false,
    }
}

#[test]
fn only_local_network_peers_reach_the_nearby_listener() {
    for allowed in [
        "127.0.0.1",
        "10.0.0.5",
        "172.16.3.4",
        "192.168.1.20",
        "169.254.9.9",
        "::ffff:192.168.1.20",
    ] {
        assert!(nearby_peer(allowed.parse().unwrap()), "{allowed}");
    }
    for refused in [
        "8.8.8.8",
        // The tailnet range, admitted until the Tailscale route was retired.
        "100.64.0.1",
        "100.127.255.254",
        "100.63.255.255",
        "100.128.0.1",
        "172.32.0.1",
        "::ffff:8.8.8.8",
        "2001:db8::1",
    ] {
        assert!(!nearby_peer(refused.parse().unwrap()), "{refused}");
    }
}

#[tokio::test]
async fn the_served_listener_admits_a_loopback_peer() {
    let (runtime, _) = fixture();
    let phone = ConnectionIdentity::generate().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(serve_direct(listener, runtime.clone()));
    let ping = request("GET", "/api/v5/ping", &serde_json::Value::Null);
    let (status, _) = post_packet(address, &encrypted(&runtime, &phone, &ping)).await;
    assert_eq!(status, 200);
    server.abort();
}

/// Whether the server closes `stream` within `wait`, reading and discarding
/// whatever it sends first.
async fn closes_within(stream: &mut tokio::net::TcpStream, wait: Duration) -> bool {
    let mut chunk = [0; 1024];
    tokio::time::timeout(wait, async {
        while let Ok(read) = stream.read(&mut chunk).await {
            if read == 0 {
                return;
            }
        }
    })
    .await
    .is_ok()
}

#[tokio::test]
async fn uploads_that_never_send_their_body_hold_no_request_slot() {
    let (runtime, _) = fixture();
    let phone = ConnectionIdentity::generate().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(serve_direct(listener, runtime.clone()));
    let mut stalled = Vec::new();
    for _ in 0..8 {
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        // `Expect` makes the server say when it starts reading the body, so
        // the test knows each upload is past the perimeter before it goes on.
        stream
            .write_all(
                format!(
                    "POST /v5/packet HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: 100000\r\nExpect: 100-continue\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut reply = [0; 25];
        tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut reply))
            .await
            .unwrap()
            .unwrap();
        assert!(reply.starts_with(b"HTTP/1.1 100"));
        stalled.push(stream);
    }
    // Relay packets take the same slots and are dropped when none is free.
    assert_eq!(runtime.requests.available_permits(), 8);
    let ping = request("GET", "/api/v5/ping", &serde_json::Value::Null);
    let (status, _) = tokio::time::timeout(
        Duration::from_secs(2),
        post_packet(address, &encrypted(&runtime, &phone, &ping)),
    )
    .await
    .unwrap();
    assert_eq!(status, 200);
    drop(stalled);
    server.abort();
}

#[tokio::test]
async fn the_listener_caps_connections_and_closes_silent_ones() {
    let (runtime, _) = fixture();
    let phone = ConnectionIdentity::generate().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let silence = Duration::from_millis(400);
    let limits = NearbyLimits {
        connections: 2,
        silence,
        ..NEARBY_LIMITS
    };
    let server = tokio::spawn(serve_nearby(listener, runtime.clone(), limits));
    let started = tokio::time::Instant::now();
    let mut idle = Vec::new();
    for _ in 0..2 {
        idle.push(tokio::net::TcpStream::connect(address).await.unwrap());
    }
    let mut extra = tokio::net::TcpStream::connect(address).await.unwrap();
    assert!(
        closes_within(&mut extra, silence / 2).await,
        "a connection past the cap stays open"
    );
    for stream in &mut idle {
        assert!(closes_within(stream, Duration::from_secs(5)).await);
    }
    assert!(
        started.elapsed() >= silence,
        "a silent connection closed early"
    );
    // Closing them freed their permits.
    let ping = request("GET", "/api/v5/ping", &serde_json::Value::Null);
    let (status, _) = post_packet(address, &encrypted(&runtime, &phone, &ping)).await;
    assert_eq!(status, 200);
    server.abort();
}

#[tokio::test]
async fn a_connection_is_not_cut_while_the_mac_answers_it() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let _peer = tokio::net::TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (stream, _) = listener.accept().await.unwrap();
    let silence = Duration::from_millis(100);
    let mut stream = NearbyStream {
        stream,
        silence,
        deadline: None,
        answering: Arc::new(AtomicBool::new(false)),
        head: None,
        started: Arc::new(AtomicBool::new(false)),
        _permit: Arc::new(Semaphore::new(1)).try_acquire_owned().unwrap(),
        _share: None,
    };
    let answering = Answering::start(&stream.answering.clone());
    let mut chunk = [0; 16];
    assert!(
        tokio::time::timeout(silence * 4, stream.read(&mut chunk))
            .await
            .is_err(),
        "the silence deadline ran while the Mac was answering"
    );
    drop(answering);
    let read = tokio::time::timeout(silence * 20, stream.read(&mut chunk))
        .await
        .expect("a silent connection stays open");
    assert_eq!(read.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
}

#[tokio::test]
async fn the_listener_drops_a_peer_outside_the_nearby_networks_before_reading() {
    let (runtime, _) = fixture();
    let phone = ConnectionIdentity::generate().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    // This loopback peer stands in for one outside the nearby networks.
    let limits = NearbyLimits {
        admit: |_| false,
        ..NEARBY_LIMITS
    };
    let server = tokio::spawn(serve_nearby(listener, runtime.clone(), limits));
    let ping = request("GET", "/api/v5/ping", &serde_json::Value::Null);
    let body = serde_json::to_vec(&encrypted(&runtime, &phone, &ping)).unwrap();
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    // A valid packet gets no answer at all, not even the perimeter's 403; the
    // writes may already fail on the closed socket.
    let _ = stream
        .write_all(
            format!(
                "POST /v5/packet HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await;
    let _ = stream.write_all(&body).await;
    let mut reply = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut chunk = [0; 1024];
        while let Ok(read) = stream.read(&mut chunk).await {
            if read == 0 {
                return;
            }
            reply.extend_from_slice(&chunk[..read]);
        }
    })
    .await
    .expect("the refused peer's connection stays open");
    assert!(reply.is_empty(), "the refused peer got an answer");
    server.abort();
}

#[tokio::test]
async fn every_nearby_response_closes_its_connection() {
    let (runtime, _) = fixture();
    let phone = ConnectionIdentity::generate().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(serve_direct(listener, runtime.clone()));
    let ping = request("GET", "/api/v5/ping", &serde_json::Value::Null);
    let body = serde_json::to_vec(&encrypted(&runtime, &phone, &ping)).unwrap();
    for (path, body) in [("/v5/packet", body), ("/elsewhere", Vec::new())] {
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        // The request asks to keep the connection; the Mac answers and closes it.
        stream
            .write_all(
                format!(
                    "POST {path} HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        stream.write_all(&body).await.unwrap();
        let mut reply = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut reply))
            .await
            .expect("the Mac kept the connection open")
            .unwrap();
        let head = String::from_utf8_lossy(&reply).to_ascii_lowercase();
        assert!(head.contains("\r\nconnection: close\r\n"), "{path}: {head}");
    }
    server.abort();
}

#[test]
fn relay_retries_back_off_to_thirty_seconds_and_restart_after_a_session() {
    let mut wait = None;
    let mut failures = Vec::new();
    for _ in 0..7 {
        let next = relay_retry(wait, false);
        failures.push(next.as_secs());
        wait = Some(next);
    }
    assert_eq!(failures, [3, 6, 12, 24, 30, 30, 30]);
    assert_eq!(relay_retry(wait, true), RELAY_RETRY_FIRST);
    assert_eq!(relay_retry(None, true), RELAY_RETRY_FIRST);
}

#[tokio::test]
async fn each_pairing_renders_only_its_own_qr_and_replaces_the_accepted_invitation() {
    let (runtime, backend) = fixture();
    let server = RemoteServer::new(runtime.http.hub.clone(), backend, std::path::PathBuf::new());
    let controller = DirectController::new(
        Arc::new(ConnectionStore::default()),
        MacProfile::Development,
    )
    .unwrap();
    controller.enable(&server, None).await.unwrap();
    let accepted = async || {
        let state = controller.state.lock().await;
        let invitation = state
            .running
            .as_ref()
            .unwrap()
            .runtime
            .invitation
            .lock()
            .unwrap();
        invitation.as_ref().unwrap().value.invitation_id.clone()
    };
    let legacy = controller.pair().await.unwrap();
    let invitation = legacy.invitation.unwrap();
    assert_eq!(
        legacy.qr_rows.unwrap(),
        qr_rows(PAIRING_PREFIX, &invitation).unwrap()
    );
    assert_eq!(accepted().await, invitation.invitation_id);
    let direct = controller.pair_direct().await.unwrap();
    let rows = qr_rows(PAIRING_PREFIX_V6, &direct.invitation).unwrap();
    assert_eq!(direct.qr_rows, rows);
    assert!(rows.iter().all(|row| {
        row.len() == rows.len() && row.bytes().all(|cell| matches!(cell, b'0' | b'1'))
    }));
    assert_ne!(
        direct.invitation.invitation.invitation_id,
        invitation.invitation_id
    );
    assert_eq!(accepted().await, direct.invitation.invitation.invitation_id);
    controller.stop().await;
    assert!(controller.pair().await.is_err());
    assert!(controller.pair_direct().await.is_err());
}

#[tokio::test]
async fn relay_replaces_stale_mac_with_newest_proven_connection() {
    let (runtime, _) = fixture();
    let phone = ConnectionIdentity::generate().unwrap();
    let relay = crate::relay::relay_router(runtime.identity.mac_id()).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, relay).await.unwrap();
    });

    // First Mac connects, proves identity, and goes silent (simulating a half-open stale link).
    let (mut stale_mac, _) = tokio_tungstenite::connect_async(format!("ws://{address}/v5/mac"))
        .await
        .unwrap();
    stale_mac
        .send(Message::Text(
            serde_json::to_string(&RelayRegistration {
                version: 5,
                public_key: runtime.identity.public_key(),
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();
    let Some(Ok(Message::Text(challenge_text))) = stale_mac.next().await else {
        panic!("expected challenge");
    };
    let challenge_packet: EncryptedPacketV5 = serde_json::from_str(&challenge_text).unwrap();
    let secret = runtime
        .identity
        .open(
            &challenge_packet,
            &runtime.identity.mac_id(),
            "relayChallenge",
            None,
        )
        .unwrap();
    stale_mac
        .send(Message::Text(
            serde_json::to_string(&RelayProof {
                version: 5,
                proof: STANDARD.encode(&secret),
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();
    let Some(Ok(Message::Text(ready_text))) = stale_mac.next().await else {
        panic!("expected ready");
    };
    assert!(ready_text.contains("\"ready\":true"));

    // A newer Mac connection with the same key proves identity and must immediately take over.
    let clone = runtime.clone();
    let fresh_mac =
        tokio::spawn(async move { relay_session(clone, &format!("ws://{address}/v5/mac")).await });
    for _ in 0..100 {
        if runtime.relay_online.load(Ordering::Acquire) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        runtime.relay_online.load(Ordering::Acquire),
        "newest proven Mac must replace the stale Mac socket"
    );

    let ping = request("GET", "/api/v5/ping", &serde_json::Value::Null);
    let (status, _) = post_packet(address, &encrypted(&runtime, &phone, &ping)).await;
    assert_eq!(status, 200);

    fresh_mac.abort();
    server.abort();
}

/// A listener on `[::]`, which macOS serves to both families: a client on
/// `127.0.0.1` and one on `[::1]` are two addresses on one machine.
async fn dual_stack_listener() -> (TcpListener, u16) {
    let listener = TcpListener::bind("[::]:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}
fn v4(port: u16) -> std::net::SocketAddr {
    std::net::SocketAddr::from(([127, 0, 0, 1], port))
}
fn v6(port: u16) -> std::net::SocketAddr {
    std::net::SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, port))
}
/// The encrypted ping a phone sends, answered `{"version":5}`.
async fn phone_pings(runtime: &DirectRuntime, address: std::net::SocketAddr) -> bool {
    let phone = ConnectionIdentity::generate().unwrap();
    let ping = request("GET", "/api/v5/ping", &serde_json::Value::Null);
    let (status, bytes) = post_packet(address, &encrypted(runtime, &phone, &ping)).await;
    status == 200
        && reply_json(&decode(
            runtime,
            &phone,
            &serde_json::from_slice(&bytes).unwrap(),
        )) == serde_json::json!({"version":5})
}

#[tokio::test]
async fn one_address_holding_slow_connections_never_shuts_a_phone_out() {
    let (runtime, _) = fixture();
    let (listener, port) = dual_stack_listener().await;
    // The real limits; `::1` stands in for another device on the network.
    let limits = NearbyLimits {
        admit: |_| true,
        ..NEARBY_LIMITS
    };
    let server = tokio::spawn(serve_nearby(listener, runtime.clone(), limits));
    let mut held = Vec::new();
    for _ in 0..NEARBY_LIMITS.per_address {
        let mut stream = tokio::net::TcpStream::connect(v6(port)).await.unwrap();
        stream.write_all(b"P").await.unwrap();
        held.push(stream);
    }
    let mut extra = tokio::net::TcpStream::connect(v6(port)).await.unwrap();
    assert!(
        closes_within(&mut extra, Duration::from_secs(1)).await,
        "one address held more than its share"
    );
    // A phone at its busiest: 4 connections to its route and 4 route probes.
    for _ in 0..2 {
        let pings =
            futures::future::join_all((0..8).map(|_| phone_pings(&runtime, v4(port)))).await;
        assert!(pings.iter().all(|answered| *answered), "{pings:?}");
    }
    drop(held);
    server.abort();
}

#[tokio::test]
async fn a_trickled_request_head_loses_its_connection_at_the_deadline() {
    let (runtime, _) = fixture();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let head = Duration::from_millis(600);
    let limits = NearbyLimits {
        connections: 1,
        request_head: head,
        ..NEARBY_LIMITS
    };
    let server = tokio::spawn(serve_nearby(listener, runtime.clone(), limits));
    let started = tokio::time::Instant::now();
    let mut slow = tokio::net::TcpStream::connect(address).await.unwrap();
    // Never silent for the 10 s silence deadline, never finishing the head.
    let trickle = async {
        for byte in
            b"POST /v5/packet HTTP/1.1\r\nHost: x\r\nX-Pad: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        {
            if slow.write_all(&[*byte]).await.is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(5), trickle)
        .await
        .expect("the trickle was never cut");
    assert!(closes_within(&mut slow, Duration::from_secs(2)).await);
    assert!(started.elapsed() >= head, "the head was cut early");
    // Its one slot is free again.
    assert!(phone_pings(&runtime, address).await);
    server.abort();
}

/// A head and body written in `pieces` with `pause` between them, as a phone
/// on a weak network delivers them.
async fn post_packet_slowly(
    address: std::net::SocketAddr,
    packet: &EncryptedPacketV5,
    pieces: usize,
    pause: Duration,
) -> u16 {
    let body = serde_json::to_vec(packet).unwrap();
    let mut bytes = format!(
        "POST /v5/packet HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    bytes.extend_from_slice(&body);
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    for piece in bytes.chunks(bytes.len().div_ceil(pieces)) {
        stream.write_all(piece).await.unwrap();
        tokio::time::sleep(pause).await;
    }
    let mut reply = Vec::new();
    tokio::time::timeout(Duration::from_secs(15), stream.read_to_end(&mut reply))
        .await
        .unwrap()
        .unwrap();
    std::str::from_utf8(&reply[9..12]).unwrap().parse().unwrap()
}

#[tokio::test]
async fn a_phone_on_a_weak_network_is_still_answered() {
    let (runtime, _) = fixture();
    let phone = ConnectionIdentity::generate().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(serve_direct(listener, runtime.clone()));
    let ping = request("GET", "/api/v5/ping", &serde_json::Value::Null);
    // The whole request in 6 pieces over 3 s, each pause well inside the
    // silence deadline: the head arrives after about 1 s and the body later.
    assert_eq!(
        post_packet_slowly(
            address,
            &encrypted(&runtime, &phone, &ping),
            6,
            Duration::from_millis(500)
        )
        .await,
        200
    );
    server.abort();
}

/// The relay on `[::]`, served with each connection's address as the
/// `interface-relay` binary serves it.
async fn relay_on_two_addresses(runtime: &DirectRuntime) -> (u16, JoinHandle<()>) {
    let relay = crate::relay::relay_router(runtime.identity.mac_id()).unwrap();
    let (listener, port) = dual_stack_listener().await;
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            relay.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (port, server)
}
async fn relay_online_within(runtime: &DirectRuntime, wait: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + wait;
    while tokio::time::Instant::now() < deadline {
        if runtime.relay_online.load(Ordering::Acquire) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}
/// Someone who knows only the relay's address: opens `/v5/mac` sockets from
/// `address` without a word and reopens each the moment the relay drops it.
fn stranger(address: std::net::SocketAddr, sockets: usize) -> Vec<JoinHandle<()>> {
    (0..sockets)
        .map(|_| {
            tokio::spawn(async move {
                loop {
                    let url = format!("ws://{address}/v5/mac");
                    if let Ok((mut socket, _)) = tokio_tungstenite::connect_async(url).await {
                        while let Some(Ok(_)) = socket.next().await {}
                    }
                    tokio::task::yield_now().await;
                }
            })
        })
        .collect()
}

#[tokio::test]
async fn strangers_holding_relay_sockets_never_keep_the_mac_out() {
    let (runtime, _) = fixture();
    let phone = ConnectionIdentity::generate().unwrap();
    let (port, server) = relay_on_two_addresses(&runtime).await;
    // Twice the whole waiting room, reopened as fast as the relay drops them.
    let strangers = stranger(v6(port), 2 * crate::relay_admission::WAITING_SOCKETS);
    tokio::time::sleep(Duration::from_millis(300)).await;
    // The Mac registers, and registers again after each of two network changes.
    for _ in 0..3 {
        runtime.relay_online.store(false, Ordering::Release);
        let clone = runtime.clone();
        let mac = tokio::spawn(async move {
            relay_session(clone, &format!("ws://{}/v5/mac", v4(port))).await
        });
        assert!(
            relay_online_within(&runtime, Duration::from_secs(5)).await,
            "a stranger kept the Mac from the relay"
        );
        let ping = request("GET", "/api/v5/ping", &serde_json::Value::Null);
        assert_eq!(
            post_packet(v4(port), &encrypted(&runtime, &phone, &ping))
                .await
                .0,
            200
        );
        mac.abort();
    }
    strangers.iter().for_each(JoinHandle::abort);
    server.abort();
}

#[tokio::test]
async fn a_mac_behind_the_strangers_address_still_registers() {
    let (runtime, _) = fixture();
    let (port, server) = relay_on_two_addresses(&runtime).await;
    // A shared address: silent sockets that fill its share, held open.
    let mut silent = Vec::new();
    for _ in 0..crate::relay_admission::WAITING_PER_ADDRESS {
        let (socket, _) = tokio_tungstenite::connect_async(format!("ws://{}/v5/mac", v4(port)))
            .await
            .unwrap();
        silent.push(socket);
    }
    let clone = runtime.clone();
    let mac =
        tokio::spawn(
            async move { relay_session(clone, &format!("ws://{}/v5/mac", v4(port))).await },
        );
    assert!(relay_online_within(&runtime, Duration::from_secs(5)).await);
    // The Mac took the oldest silent socket's place.
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), silent[0].next()).await,
        Ok(None | Some(Err(_) | Ok(Message::Close(_))))
    ));
    mac.abort();
    server.abort();
}

#[tokio::test]
async fn a_mac_registers_past_its_own_unfinished_attempts() {
    let (runtime, _) = fixture();
    let (port, server) = relay_on_two_addresses(&runtime).await;
    // Attempts lost mid-way in a network change: each named the Mac and
    // stopped before the proof, so they still wait for it.
    let mut lost = Vec::new();
    for _ in 0..crate::relay_admission::WAITING_PER_ADDRESS + 2 {
        let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{}/v5/mac", v4(port)))
            .await
            .unwrap();
        socket
            .send(Message::Text(
                serde_json::to_string(&RelayRegistration {
                    version: 5,
                    public_key: runtime.identity.public_key(),
                })
                .unwrap()
                .into(),
            ))
            .await
            .unwrap();
        lost.push(socket);
    }
    let clone = runtime.clone();
    let mac =
        tokio::spawn(
            async move { relay_session(clone, &format!("ws://{}/v5/mac", v4(port))).await },
        );
    assert!(relay_online_within(&runtime, Duration::from_secs(5)).await);
    mac.abort();
    server.abort();
}

/// A relay process of its own: its runtime ends every connection when it is
/// shut down, as a relay's exit or redeploy does.
fn relay_process(mac_id: String, port: u16) -> (tokio::runtime::Runtime, u16) {
    let listener = std::net::TcpListener::bind(std::net::SocketAddr::from((
        std::net::Ipv6Addr::UNSPECIFIED,
        port,
    )))
    .unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let process = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    process.spawn(async move {
        let relay = crate::relay::relay_router(mac_id).unwrap();
        axum::serve(
            TcpListener::from_std(listener).unwrap(),
            relay.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (process, port)
}

#[tokio::test]
async fn the_mac_comes_back_after_its_relay_restarts() {
    let (runtime, _) = fixture();
    let phone = ConnectionIdentity::generate().unwrap();
    let (first, port) = relay_process(runtime.identity.mac_id(), 0);
    // The Mac's own reconnect loop, pointed at the local relay.
    let mac = tokio::spawn(relay_loop(runtime.clone(), format!("ws://{}", v4(port))));
    assert!(relay_online_within(&runtime, Duration::from_secs(5)).await);
    // A redeploy: the relay and every connection to it end, and a new relay
    // starts at the same address.
    // Waits until its listener and connections are gone, as a process exit does.
    tokio::task::spawn_blocking(move || first.shutdown_timeout(Duration::from_secs(5)))
        .await
        .unwrap();
    let (second, _) = relay_process(runtime.identity.mac_id(), port);
    // The Mac waits 3 s after a session ends, then registers with the new relay.
    let ping = request("GET", "/api/v5/ping", &serde_json::Value::Null);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut answered = false;
    while !answered && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(200)).await;
        answered = post_packet(v4(port), &encrypted(&runtime, &phone, &ping))
            .await
            .0
            == 200;
    }
    assert!(answered, "the Mac never came back to the restarted relay");
    runtime.active.store(false, Ordering::Release);
    mac.abort();
    second.shutdown_background();
}

#[tokio::test]
async fn slow_uploads_to_the_relay_never_keep_the_mac_or_a_phone_out() {
    let (runtime, _) = fixture();
    let phone = ConnectionIdentity::generate().unwrap();
    let (port, server) = relay_on_two_addresses(&runtime).await;
    // Uploads that announce a body and never send it, more than the relay
    // works on at once, from one other address.
    let mut uploads = Vec::new();
    for _ in 0..40 {
        let mut stream = tokio::net::TcpStream::connect(v6(port)).await.unwrap();
        stream
            .write_all(
                b"POST /v5/packet HTTP/1.1\r\nHost: relay\r\nContent-Type: application/json\r\nContent-Length: 100000\r\n\r\n{",
            )
            .await
            .unwrap();
        uploads.push(stream);
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let clone = runtime.clone();
    let mac =
        tokio::spawn(
            async move { relay_session(clone, &format!("ws://{}/v5/mac", v4(port))).await },
        );
    assert!(
        relay_online_within(&runtime, Duration::from_secs(5)).await,
        "slow uploads kept the Mac from the relay"
    );
    // A phone at its busiest through the relay: 4 requests and a route probe.
    let pings = futures::future::join_all((0..5).map(|_| async {
        let ping = request("GET", "/api/v5/ping", &serde_json::Value::Null);
        post_packet(v4(port), &encrypted(&runtime, &phone, &ping))
            .await
            .0
    }))
    .await;
    assert!(pings.iter().all(|status| *status == 200), "{pings:?}");
    drop(uploads);
    mac.abort();
    server.abort();
}
