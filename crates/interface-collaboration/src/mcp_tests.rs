use std::{
    os::unix::fs::PermissionsExt,
    process::Stdio,
    sync::atomic::{AtomicBool, Ordering},
};

use tempfile::TempDir;
use tokio::{
    io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    net::unix::OwnedReadHalf,
    process::Command,
    time::timeout,
};

use super::*;

#[derive(Default)]
struct TestIngress {
    cancelled: AtomicBool,
    entered: AtomicBool,
    never_settles: AtomicBool,
}

#[async_trait]
impl CollaborationMcpRequestIngress for TestIngress {
    async fn submit(
        &self,
        _origin_connection_id: &str,
        _intent: CollaborationMcpIntent,
        cancellation: IngressCancellation,
    ) -> Result<CollaborationReceiptV1, McpIngressError> {
        self.entered.store(true, Ordering::Release);
        if self.never_settles.load(Ordering::Acquire) {
            // A consult that may already have committed: it never observes
            // cancellation and must not be aborted by shutdown.
            std::future::pending::<()>().await;
        }
        cancellation.cancelled().await;
        self.cancelled.store(true, Ordering::Release);
        Err(McpIngressError::new(
            CollaborationErrorCode::BrokerUnavailable,
        ))
    }

    async fn status(
        &self,
        _origin_connection_id: &str,
        _request: CollaborationMcpAccessRequest,
        _cancellation: IngressCancellation,
    ) -> Result<CollaborationReceiptV1, McpIngressError> {
        Err(McpIngressError::new(
            CollaborationErrorCode::BrokerUnavailable,
        ))
    }

    async fn cancel(
        &self,
        _origin_connection_id: &str,
        _request: CollaborationMcpAccessRequest,
        _cancellation: IngressCancellation,
    ) -> Result<CollaborationReceiptV1, McpIngressError> {
        Err(McpIngressError::new(
            CollaborationErrorCode::BrokerUnavailable,
        ))
    }
}

async fn socket(
    request_timeout: Duration,
) -> (TempDir, Arc<CollaborationMcpSocket>, Arc<TestIngress>) {
    socket_draining_within(
        request_timeout,
        Duration::from_millis(STOP_DRAIN_TIMEOUT_MS),
    )
    .await
}

async fn socket_draining_within(
    request_timeout: Duration,
    stop_drain_timeout: Duration,
) -> (TempDir, Arc<CollaborationMcpSocket>, Arc<TestIngress>) {
    let directory = tempfile::tempdir_in("/tmp").expect("temporary directory");
    let ingress = Arc::new(TestIngress::default());
    let mut options = CollaborationMcpSocketOptions::frozen(
        directory.path().join("private").join("broker.sock"),
        CollaborationEngineId::Claude,
        "out-of-band-capability".into(),
        Arc::clone(&ingress) as Arc<dyn CollaborationMcpRequestIngress>,
    );
    options.request_timeout = request_timeout;
    options.stop_drain_timeout = stop_drain_timeout;
    let socket = CollaborationMcpSocket::listen(options)
        .await
        .expect("listen");
    (directory, socket, ingress)
}

async fn write_json(writer: &mut (impl AsyncWrite + Unpin), value: Value) {
    let mut frame = serde_json::to_vec(&value).expect("serialize frame");
    frame.push(b'\n');
    writer.write_all(&frame).await.expect("write frame");
    writer.flush().await.expect("flush frame");
}

async fn read_json(reader: &mut BufReader<OwnedReadHalf>) -> Value {
    let mut line = String::new();
    timeout(Duration::from_secs(2), reader.read_line(&mut line))
        .await
        .expect("response timeout")
        .expect("read response");
    serde_json::from_str(&line).expect("JSON response")
}

async fn initialize(
    reader: &mut BufReader<OwnedReadHalf>,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
) {
    write_json(
        writer,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "transport-test", "version": "1"}
            }
        }),
    )
    .await;
    let initialized = read_json(reader).await;
    assert_eq!(
        initialized["result"]["protocolVersion"],
        MCP_PROTOCOL_VERSION
    );
    write_json(
        writer,
        json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }),
    )
    .await;
}

#[tokio::test]
async fn owner_private_socket_lists_exactly_the_four_collaboration_tools() {
    let (_directory, socket, _ingress) = socket(Duration::from_secs(1)).await;
    assert_eq!(
        std::fs::metadata(socket.socket_path())
            .expect("socket metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(socket.socket_path().parent().expect("parent"))
            .expect("parent metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );

    let stream = UnixStream::connect(socket.socket_path())
        .await
        .expect("connect");
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    initialize(&mut reader, &mut writer).await;
    write_json(
        &mut writer,
        json!({"jsonrpc":"2.0", "id":2, "method":"tools/list", "params":{}}),
    )
    .await;
    let response = read_json(&mut reader).await;
    let names = response["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name"))
        .collect::<Vec<_>>();
    assert_eq!(names, COLLABORATION_MCP_TOOL_NAMES[..4]);
    drop(writer);
    drop(reader);
    socket.stop().await.expect("stop");
    assert!(!socket.socket_path().exists());
}

#[tokio::test]
async fn orchestrator_tool_names_stay_callable_and_refuse() {
    let (_directory, socket, _ingress) = socket(Duration::from_secs(1)).await;
    let stream = UnixStream::connect(socket.socket_path())
        .await
        .expect("connect");
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    initialize(&mut reader, &mut writer).await;
    for (id, name, untrusted) in [(2, "iface_workers", true), (3, "iface_models", false)] {
        write_json(
            &mut writer,
            json!({
                "jsonrpc":"2.0",
                "id":id,
                "method":"tools/call",
                "params":{"name":name, "arguments":{}}
            }),
        )
        .await;
        let response = read_json(&mut reader).await;
        assert_eq!(response["result"]["isError"], true, "{name}");
        let structured = &response["result"]["structuredContent"];
        assert_eq!(structured["code"], "not_an_orchestrator", "{name}");
        assert_eq!(
            structured["untrusted"].as_bool(),
            untrusted.then_some(true),
            "{name}"
        );
        let text = response["result"]["content"][0]["text"]
            .as_str()
            .expect("text");
        assert_eq!(
            text.starts_with(UNTRUSTED_WORKER_PREFIX),
            untrusted,
            "{name}"
        );
        assert!(text.ends_with("not_an_orchestrator: Ask the human to enable an orchestrator policy for this session."), "{name}");
    }
    drop(writer);
    drop(reader);
    socket.stop().await.expect("stop");
}

#[tokio::test]
async fn deadline_cancels_host_work_but_still_returns_recovery_receipt() {
    let (_directory, socket, ingress) = socket(Duration::from_millis(20)).await;
    let stream = UnixStream::connect(socket.socket_path())
        .await
        .expect("connect");
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    initialize(&mut reader, &mut writer).await;
    write_json(
        &mut writer,
        json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{
                "name":"iface_consult",
                "arguments":{
                    "targetEngine":"codex",
                    "objective":"Review the current implementation.",
                    "clientRequestId":"deadline-test"
                }
            }
        }),
    )
    .await;
    let response = read_json(&mut reader).await;
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(
        response["result"]["structuredContent"]["code"],
        "RECOVERY_REQUIRED"
    );
    timeout(Duration::from_secs(1), async {
        while !ingress.cancelled.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("host cancellation");
    drop(writer);
    drop(reader);
    socket.stop().await.expect("stop");
}

#[tokio::test]
async fn stop_drains_within_its_budget_and_leaves_committed_calls_running() {
    let (_directory, socket, ingress) =
        socket_draining_within(Duration::from_millis(20), Duration::from_millis(50)).await;
    ingress.never_settles.store(true, Ordering::Release);
    let stream = UnixStream::connect(socket.socket_path())
        .await
        .expect("connect");
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    initialize(&mut reader, &mut writer).await;
    // A consult may already have committed a provider effect, so the socket
    // must stop without waiting for it.
    write_json(
        &mut writer,
        json!({
            "jsonrpc":"2.0",
            "id":4,
            "method":"tools/call",
            "params":{
                "name":"iface_consult",
                "arguments":{
                    "targetEngine":"codex",
                    "objective":"Review the current implementation.",
                    "clientRequestId":"bounded-drain-test"
                }
            }
        }),
    )
    .await;
    timeout(Duration::from_secs(2), async {
        while !ingress.entered.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("delegation call in flight");

    timeout(Duration::from_secs(2), socket.stop())
        .await
        .expect("stop must not wait for the delegation deadline")
        .expect("stop");
    assert!(!socket.socket_path().exists());
    drop(writer);
    drop(reader);
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn real_usr_bin_nc_reaches_the_env_free_unix_listener() {
    let (_directory, socket, _ingress) = socket(Duration::from_secs(1)).await;
    let mut child = Command::new("/usr/bin/nc")
        .arg("-U")
        .arg(socket.socket_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn nc");
    let mut stdin = child.stdin.take().expect("nc stdin");
    write_json(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 9,
            "method": "initialize",
            "params": {
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "real-nc", "version": "1"}
            }
        }),
    )
    .await;
    let mut stdout = BufReader::new(child.stdout.take().expect("nc stdout"));
    let mut line = String::new();
    timeout(Duration::from_secs(2), stdout.read_line(&mut line))
        .await
        .expect("nc response timeout")
        .expect("read nc response");
    let response: Value = serde_json::from_str(&line).expect("nc JSON response");
    assert_eq!(response["id"], 9);
    assert_eq!(response["result"]["protocolVersion"], MCP_PROTOCOL_VERSION);
    child.kill().await.expect("kill nc");
    let _ = child.wait().await;
    socket.stop().await.expect("stop");
}
