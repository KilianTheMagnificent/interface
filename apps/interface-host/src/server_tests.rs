use std::{
    fmt::Write as _,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use async_trait::async_trait;
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, duplex},
    sync::Notify,
    time::{Duration, timeout},
};

use super::*;
use crate::dispatcher::DispatchError;

struct Echo;

#[tokio::test]
async fn new_request_is_admitted_after_push_flood_has_started() {
    let (mut input, server_read) = duplex(4096);
    let (output, server_write) = duplex(1);
    let mut frames = FramedRead::new(output, LinesCodec::new());
    let (sender, receiver) = mpsc::channel(2);
    let host = HostServer::with_pushes(Arc::new(Echo), receiver, HostServerConfig::default());
    let task = tokio::spawn(async move { host.run(server_read, server_write).await });
    frames.next().await.unwrap().unwrap(); // ready
    let push = || PushFrame::new("iface:registry", serde_json::json!([])).unwrap();
    sender.send(push()).await.unwrap();
    sender.send(push()).await.unwrap();
    // Read one push first, proving the flood predates the request.
    frames.next().await.unwrap().unwrap();
    sender.send(push()).await.unwrap();
    input.write_all(format!("{{\"version\":{},\"id\":\"late\",\"operation\":\"iface:getAppearance\",\"payload\":{{}}}}\n",
        crate::protocol::PROTOCOL_VERSION).as_bytes()).await.unwrap();
    timeout(Duration::from_secs(2), async {
        loop {
            let line = frames.next().await.unwrap().unwrap();
            if let HostFrame::Response(response) = serde_json::from_str(&line).unwrap() {
                assert_eq!(response.id, "late");
                assert!(response.ok);
                break;
            }
            sender.send(push()).await.unwrap();
        }
    })
    .await
    .expect("new request must cross sustained pushes");
    drop(sender);
    input.shutdown().await.unwrap();
    while frames.next().await.is_some() {}
    task.await.unwrap().unwrap();
}

#[async_trait]
impl HostDispatcher for Echo {
    async fn dispatch(&self, operation: &str, payload: Value) -> Result<Value, DispatchError> {
        Ok(serde_json::json!({"operation": operation, "payload": payload}))
    }
}

struct Saturating {
    active: AtomicUsize,
    maximum: AtomicUsize,
    started: AtomicUsize,
    capacity_reached: Notify,
    released: AtomicBool,
    release: Notify,
}

struct PushBeforeReturn {
    sender: mpsc::Sender<PushFrame>,
}

struct HeldResponse {
    started: Notify,
    released: AtomicBool,
    release: Notify,
}

#[async_trait]
impl HostDispatcher for PushBeforeReturn {
    async fn dispatch(&self, _operation: &str, _payload: Value) -> Result<Value, DispatchError> {
        self.sender
            .send(PushFrame::new("iface:registry", serde_json::json!(["changed"])).unwrap())
            .await
            .unwrap();
        Ok(serde_json::json!({"accepted": true}))
    }
}

impl HeldResponse {
    fn new() -> Self {
        Self {
            started: Notify::new(),
            released: AtomicBool::new(false),
            release: Notify::new(),
        }
    }
}

#[async_trait]
impl HostDispatcher for HeldResponse {
    async fn dispatch(&self, _operation: &str, payload: Value) -> Result<Value, DispatchError> {
        self.started.notify_one();
        loop {
            let released = self.release.notified();
            if self.released.load(Ordering::SeqCst) {
                break;
            }
            released.await;
        }
        Ok(payload)
    }
}

struct OversizedThenSmall;

#[async_trait]
impl HostDispatcher for OversizedThenSmall {
    async fn dispatch(&self, operation: &str, _payload: Value) -> Result<Value, DispatchError> {
        if operation == "iface:getAppearance" {
            Ok(Value::String("x".repeat(2_048)))
        } else {
            Ok(serde_json::json!({"small": true}))
        }
    }
}

impl Saturating {
    fn new() -> Self {
        Self {
            active: AtomicUsize::new(0),
            maximum: AtomicUsize::new(0),
            started: AtomicUsize::new(0),
            capacity_reached: Notify::new(),
            released: AtomicBool::new(false),
            release: Notify::new(),
        }
    }
}

#[async_trait]
impl HostDispatcher for Saturating {
    async fn dispatch(&self, _operation: &str, payload: Value) -> Result<Value, DispatchError> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.maximum.fetch_max(active, Ordering::SeqCst);
        if self.started.fetch_add(1, Ordering::SeqCst) + 1 == 2 {
            self.capacity_reached.notify_one();
        }
        loop {
            let released = self.release.notified();
            if self.released.load(Ordering::SeqCst) {
                break;
            }
            released.await;
        }
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(payload)
    }
}

#[tokio::test]
async fn ready_and_correlated_response_cross_the_bounded_wire() {
    let (mut client, server) = duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(server);
    let host = HostServer::new(Arc::new(Echo), HostServerConfig::default());
    let task = tokio::spawn(async move { host.run(server_read, server_write).await });

    client
        .write_all(
            format!(
                "{{\"version\":{},\"id\":\"r1\",\"operation\":\"iface:getAppearance\",\"payload\":{{}}}}\n",
                crate::protocol::PROTOCOL_VERSION
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    client.shutdown().await.unwrap();
    let mut output = String::new();
    client.read_to_string(&mut output).await.unwrap();
    task.await.unwrap().unwrap();
    let frames = output
        .lines()
        .map(|line| serde_json::from_str::<HostFrame>(line).unwrap())
        .collect::<Vec<_>>();
    assert!(matches!(frames[0], HostFrame::Ready(_)));
    let HostFrame::Response(response) = &frames[1] else {
        panic!("second frame must be a response")
    };
    assert_eq!(response.id, "r1");
    assert_eq!(
        response.result.as_ref().unwrap()["operation"],
        "iface:getAppearance"
    );
}

#[tokio::test]
async fn the_ready_frame_reports_the_host_version_the_binary_configures() {
    let (mut client, server) = duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(server);
    let config = HostServerConfig {
        host_version: "0.3.140",
        ..HostServerConfig::default()
    };
    let host = HostServer::new(Arc::new(Echo), config);
    let task = tokio::spawn(async move { host.run(server_read, server_write).await });
    client.shutdown().await.unwrap();
    let mut output = String::new();
    client.read_to_string(&mut output).await.unwrap();
    task.await.unwrap().unwrap();

    let ready = serde_json::from_str::<HostFrame>(output.lines().next().unwrap()).unwrap();
    let HostFrame::Ready(ready) = ready else {
        panic!("the first frame must be ready")
    };
    assert_eq!(ready.host_version, "0.3.140");
    assert_eq!(
        HostServerConfig::default().host_version,
        env!("CARGO_PKG_VERSION")
    );
}

#[tokio::test]
async fn malformed_request_isolated_to_one_failure_frame() {
    let (mut client, server) = duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(server);
    let host = HostServer::new(Arc::new(Echo), HostServerConfig::default());
    let task = tokio::spawn(async move { host.run(server_read, server_write).await });
    client.write_all(b"not-json\n").await.unwrap();
    client.shutdown().await.unwrap();
    let mut output = String::new();
    client.read_to_string(&mut output).await.unwrap();
    task.await.unwrap().unwrap();
    let response = output
        .lines()
        .nth(1)
        .map(|line| serde_json::from_str::<HostFrame>(line).unwrap())
        .unwrap();
    let HostFrame::Response(response) = response else {
        panic!("malformed input must produce a response")
    };
    assert_eq!(response.error.unwrap().code, "invalid-json");
}

#[tokio::test]
async fn request_admission_never_exceeds_the_configured_bound() {
    let (mut client, server) = duplex(256 * 1024);
    let (server_read, server_write) = tokio::io::split(server);
    let dispatcher = Arc::new(Saturating::new());
    let config = HostServerConfig {
        max_concurrent_requests: 2,
        ..HostServerConfig::default()
    };
    let host = HostServer::new(Arc::clone(&dispatcher), config);
    let task = tokio::spawn(async move { host.run(server_read, server_write).await });
    let requests = (0..10).fold(String::new(), |mut requests, index| {
        writeln!(
            requests,
            "{{\"version\":{},\"id\":\"r{index}\",\"operation\":\"iface:getAppearance\",\"payload\":{{}}}}",
            crate::protocol::PROTOCOL_VERSION
        )
        .expect("write request fixture");
        requests
    });
    client.write_all(requests.as_bytes()).await.unwrap();

    timeout(
        Duration::from_secs(1),
        dispatcher.capacity_reached.notified(),
    )
    .await
    .expect("two requests admitted");
    tokio::task::yield_now().await;
    assert_eq!(dispatcher.started.load(Ordering::SeqCst), 2);
    assert_eq!(dispatcher.maximum.load(Ordering::SeqCst), 2);

    dispatcher.released.store(true, Ordering::SeqCst);
    dispatcher.release.notify_waiters();
    client.shutdown().await.unwrap();
    let mut output = String::new();
    client.read_to_string(&mut output).await.unwrap();
    task.await.unwrap().unwrap();
    assert_eq!(dispatcher.started.load(Ordering::SeqCst), 10);
    assert_eq!(dispatcher.maximum.load(Ordering::SeqCst), 2);
    assert_eq!(output.lines().count(), 11);
}

#[tokio::test]
async fn pushes_and_responses_share_the_single_bounded_writer() {
    let (mut client, server) = duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(server);
    let (push_sender, push_receiver) = mpsc::channel(1);
    let host = HostServer::with_pushes(Arc::new(Echo), push_receiver, HostServerConfig::default());
    let task = tokio::spawn(async move { host.run(server_read, server_write).await });
    push_sender
        .send(PushFrame::new("iface:registry", serde_json::json!([])).unwrap())
        .await
        .unwrap();
    drop(push_sender);
    client
        .write_all(
            format!(
                "{{\"version\":{},\"id\":\"r1\",\"operation\":\"iface:getAppearance\",\"payload\":{{}}}}\n",
                crate::protocol::PROTOCOL_VERSION
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    client.shutdown().await.unwrap();
    let mut output = String::new();
    client.read_to_string(&mut output).await.unwrap();
    task.await.unwrap().unwrap();

    let frames = output
        .lines()
        .map(|line| serde_json::from_str::<HostFrame>(line).unwrap())
        .collect::<Vec<_>>();
    assert!(frames.iter().any(|frame| matches!(
        frame,
        HostFrame::Push(push) if push.operation == "iface:registry" && push.payload == serde_json::json!([])
    )));
    assert!(frames.iter().any(|frame| matches!(
        frame,
        HostFrame::Response(response) if response.id == "r1" && response.ok
    )));
}

#[tokio::test]
async fn mutation_push_serializes_before_its_correlated_response() {
    let (mut client, server) = duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(server);
    let (push_sender, push_receiver) = mpsc::channel(1);
    let host = HostServer::with_pushes(
        Arc::new(PushBeforeReturn {
            sender: push_sender,
        }),
        push_receiver,
        HostServerConfig::default(),
    );
    let task = tokio::spawn(async move { host.run(server_read, server_write).await });
    client
        .write_all(
            format!(
                "{{\"version\":{},\"id\":\"r1\",\"operation\":\"iface:getAppearance\",\"payload\":{{}}}}\n",
                crate::protocol::PROTOCOL_VERSION
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    client.shutdown().await.unwrap();
    let mut output = String::new();
    client.read_to_string(&mut output).await.unwrap();
    task.await.unwrap().unwrap();

    let frames = output
        .lines()
        .map(|line| serde_json::from_str::<HostFrame>(line).unwrap())
        .collect::<Vec<_>>();
    assert!(matches!(
        &frames[1],
        HostFrame::Push(push)
            if push.operation == "iface:registry"
                && push.payload == serde_json::json!(["changed"])
    ));
    assert!(matches!(
        &frames[2],
        HostFrame::Response(response) if response.id == "r1" && response.ok
    ));
}

#[tokio::test]
async fn completed_response_crosses_sustained_push_backpressure_within_a_bound() {
    // A one-byte duplex forces every frame write to yield to this reader.
    // Replacing each observed push before yielding keeps the bounded push
    // channel continuously non-empty, deterministically reproducing the
    // starvation that a push-first select alone permits.
    let (client, server) = duplex(1);
    let (client_read, mut client_write) = tokio::io::split(client);
    let (server_read, server_write) = tokio::io::split(server);
    let mut frames = FramedRead::new(client_read, LinesCodec::new());
    let (push_sender, push_receiver) = mpsc::channel(2);
    let dispatcher = Arc::new(HeldResponse::new());
    let host = HostServer::with_pushes(
        Arc::clone(&dispatcher),
        push_receiver,
        HostServerConfig {
            outbound_capacity: 2,
            ..HostServerConfig::default()
        },
    );
    let task = tokio::spawn(async move { host.run(server_read, server_write).await });

    let ready_line = timeout(Duration::from_secs(1), frames.next())
        .await
        .expect("ready frame deadline")
        .expect("ready frame present")
        .expect("ready frame valid");
    assert!(matches!(
        serde_json::from_str::<HostFrame>(&ready_line).unwrap(),
        HostFrame::Ready(_)
    ));

    let started = dispatcher.started.notified();
    client_write
        .write_all(
            format!(
                "{{\"version\":{},\"id\":\"fair\",\"operation\":\"iface:getAppearance\",\"payload\":{{}}}}\n",
                crate::protocol::PROTOCOL_VERSION
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    timeout(Duration::from_secs(1), started)
        .await
        .expect("request admitted");
    for sequence in 0..2 {
        push_sender
            .try_send(PushFrame::new("iface:registry", serde_json::json!([sequence])).unwrap())
            .expect("initial push backlog fits its bound");
    }
    dispatcher.released.store(true, Ordering::SeqCst);
    dispatcher.release.notify_waiters();

    let mut response_within_bound = false;
    let mut replacement = 2;
    for _ in 0..6 {
        let line = timeout(Duration::from_secs(1), frames.next())
            .await
            .expect("host frame deadline")
            .expect("host frame present")
            .expect("host frame valid");
        match serde_json::from_str::<HostFrame>(&line).unwrap() {
            HostFrame::Push(_) => {
                push_sender
                    .try_send(
                        PushFrame::new("iface:registry", serde_json::json!([replacement])).unwrap(),
                    )
                    .expect("observed push releases one bounded channel slot");
                replacement += 1;
            }
            HostFrame::Response(response) if response.id == "fair" => {
                response_within_bound = true;
                break;
            }
            HostFrame::Response(_) | HostFrame::Ready(_) => {}
        }
    }

    drop(push_sender);
    client_write.shutdown().await.unwrap();
    while let Some(line) = frames.next().await {
        line.unwrap();
    }
    task.await.unwrap().unwrap();
    assert!(
        response_within_bound,
        "a completed response remained behind a continuously replenished push queue"
    );
}

#[tokio::test]
async fn oversized_response_is_correlated_without_terminating_the_host() {
    let (mut client, server) = duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(server);
    let host = HostServer::new(
        Arc::new(OversizedThenSmall),
        HostServerConfig {
            max_frame_bytes: 512,
            ..HostServerConfig::default()
        },
    );
    let task = tokio::spawn(async move { host.run(server_read, server_write).await });
    client
        .write_all(
            format!(
                "{{\"version\":{},\"id\":\"large\",\"operation\":\"iface:getAppearance\",\"payload\":{{}}}}\n\
{{\"version\":{},\"id\":\"small\",\"operation\":\"iface:getUpdateState\",\"payload\":{{}}}}\n",
                crate::protocol::PROTOCOL_VERSION,
                crate::protocol::PROTOCOL_VERSION
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    client.shutdown().await.unwrap();
    let mut output = String::new();
    client.read_to_string(&mut output).await.unwrap();
    task.await.unwrap().unwrap();

    let responses = output
        .lines()
        .skip(1)
        .map(|line| serde_json::from_str::<HostFrame>(line).unwrap())
        .filter_map(|frame| match frame {
            HostFrame::Response(response) => Some(response),
            HostFrame::Ready(_) | HostFrame::Push(_) => None,
        })
        .collect::<Vec<_>>();
    let large = responses
        .iter()
        .find(|response| response.id == "large")
        .unwrap();
    assert_eq!(
        large.error.as_ref().map(|error| error.code.as_str()),
        Some("response-too-large")
    );
    assert!(!large.error.as_ref().unwrap().replay_safe);
    let small = responses
        .iter()
        .find(|response| response.id == "small")
        .unwrap();
    assert!(small.ok);
    assert_eq!(small.result.as_ref().unwrap()["small"], true);
}

/// Counts the writes the server issues; the frames themselves pass through.
struct CountingWriter<W> {
    inner: W,
    writes: Arc<AtomicUsize>,
}

impl<W: AsyncWrite + Unpin> AsyncWrite for CountingWriter<W> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        self.writes.fetch_add(1, Ordering::Relaxed);
        std::pin::Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[tokio::test]
async fn pushes_queued_together_leave_in_order_in_one_write() {
    let (mut client, server) = duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(server);
    let writes = Arc::new(AtomicUsize::new(0));
    let server_write = CountingWriter {
        inner: server_write,
        writes: Arc::clone(&writes),
    };
    let (push_sender, push_receiver) = mpsc::channel(64);
    for sequence in 0..40 {
        push_sender
            .try_send(PushFrame::new("iface:registry", serde_json::json!([sequence])).unwrap())
            .unwrap();
    }
    drop(push_sender);
    let host = HostServer::with_pushes(Arc::new(Echo), push_receiver, HostServerConfig::default());
    let task = tokio::spawn(async move { host.run(server_read, server_write).await });
    client.shutdown().await.unwrap();
    let mut output = String::new();
    client.read_to_string(&mut output).await.unwrap();
    task.await.unwrap().unwrap();

    let sequences = output
        .lines()
        .filter_map(
            |line| match serde_json::from_str::<HostFrame>(line).unwrap() {
                HostFrame::Push(push) => Some(push.payload[0].as_u64().unwrap()),
                _ => None,
            },
        )
        .collect::<Vec<_>>();
    assert_eq!(sequences, (0..40).collect::<Vec<_>>());
    // One write for the ready frame, one for the whole batch of forty pushes.
    assert_eq!(writes.load(Ordering::Relaxed), 2);
}

#[test]
fn a_huge_request_line_releases_the_read_buffer_but_keeps_unread_bytes() {
    let mut buffer = BytesMut::with_capacity(8 * READ_BUFFER_RELEASE_BYTES);
    buffer.extend_from_slice(b"{\"next\":");
    release_read_buffer(&mut buffer);
    assert_eq!(&buffer[..], b"{\"next\":");
    assert!(buffer.capacity() <= READ_BUFFER_RELEASE_BYTES);

    let mut small = BytesMut::with_capacity(64 * 1024);
    small.extend_from_slice(b"partial");
    let before = small.capacity();
    release_read_buffer(&mut small);
    assert_eq!(&small[..], b"partial");
    assert_eq!(small.capacity(), before, "an ordinary buffer is kept");
}
