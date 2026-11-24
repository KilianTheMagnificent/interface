use interface_collaboration::workers::{
    INSTRUCTIONS, MCP_SERVER_NAME, MCP_SERVER_TITLE, Service, catalogue,
};
use interface_storage::PrivateDirectory;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    future::Future,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::Path,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    process::Command,
    sync::{Mutex, Semaphore},
};

const FRAME_LIMIT: u64 = 512 * 1024;
#[derive(Clone)]
struct Binding {
    pid: u32,
    stamp: String,
    owner: String,
}
#[derive(Clone)]
struct Process {
    parent: u32,
    stamp: String,
}
type Bindings = Arc<Mutex<Vec<Binding>>>;
type Table = BTreeMap<u32, Process>;

pub(super) async fn serve(root: &Path, service: Arc<Service>) -> Result<(), String> {
    let directory = PrivateDirectory::open(root).map_err(|e| e.to_string())?;
    let path = root.join("broker.sock");
    if let Ok(metadata) = std::fs::symlink_metadata(&path) {
        if !metadata.file_type().is_socket() {
            return Err("worker socket path is not a socket".into());
        }
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    let listener = UnixListener::bind(&path).map_err(|e| e.to_string())?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())?;
    let uid = std::fs::metadata(directory.path())
        .map_err(|e| e.to_string())?
        .uid();
    accept_leads(
        &listener,
        uid,
        &service,
        process_table,
        tokio::signal::ctrl_c(),
    )
    .await;
    let _ = std::fs::remove_file(&path);
    Ok(())
}

/// Serves lead connections until `stop` resolves, then shuts the service down
/// before dropping the connections. One failed accept, or a peer that hung up
/// before its credentials could be read, costs only that connection: every
/// other lead keeps its bridge and active runs still get their cooperative
/// cancel at shutdown.
async fn accept_leads<P, F>(
    listener: &UnixListener,
    uid: u32,
    service: &Arc<Service>,
    probe: P,
    stop: impl Future,
) where
    P: Fn() -> F + Clone + Send + Sync + 'static,
    F: Future<Output = Result<Table, String>> + Send,
{
    let bindings = Arc::new(Mutex::new(Vec::new()));
    let tables = Arc::new(SharedProbe::new(probe));
    let probe = move || {
        let tables = Arc::clone(&tables);
        async move { tables.read().await }
    };
    let permits = Arc::new(Semaphore::new(32));
    let mut connections = tokio::task::JoinSet::new();
    tokio::pin!(stop);
    loop {
        tokio::select! {
            _ = &mut stop => break,
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else {
                    // An aborted peer or momentary descriptor pressure. Back
                    // off so a persistent failure cannot spin this loop.
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                };
                let Ok(credentials) = stream.peer_cred() else { continue; };
                if credentials.uid() != uid { continue; }
                let Some(pid) = credentials.pid().and_then(|p| u32::try_from(p).ok()) else { continue; };
                let Ok(permit) = permits.clone().try_acquire_owned() else { continue; };
                let service = service.clone(); let bindings = bindings.clone(); let probe = probe.clone();
                connections.spawn(async move { let _permit = permit; let _ = connection(stream, pid, service, bindings, probe).await; });
            }
            _ = connections.join_next(), if !connections.is_empty() => {}
        }
    }
    service.shutdown().await;
    connections.abort_all();
}

/// One process-table read shared by the requests that wait for it. A caller
/// only takes a table whose read began after it asked, so each request still
/// proves its lead against the process table as it was then; callers that
/// arrive while one read runs share the next, instead of each spawning `ps`.
struct SharedProbe<P> {
    probe: P,
    tickets: AtomicU64,
    /// The ticket drawn when the latest read began, and what it found. Held
    /// across the read, so reads never overlap.
    latest: Mutex<Option<(u64, Result<Table, String>)>>,
}

impl<P, F> SharedProbe<P>
where
    P: Fn() -> F,
    F: Future<Output = Result<Table, String>>,
{
    fn new(probe: P) -> Self {
        Self {
            probe,
            tickets: AtomicU64::new(0),
            latest: Mutex::new(None),
        }
    }

    async fn read(&self) -> Result<Table, String> {
        let asked = self.tickets.fetch_add(1, Ordering::SeqCst);
        let mut latest = self.latest.lock().await;
        if let Some((began, result)) = latest.as_ref()
            && *began > asked
        {
            return result.clone();
        }
        let began = self.tickets.fetch_add(1, Ordering::SeqCst);
        let result = (self.probe)().await;
        *latest = Some((began, result.clone()));
        result
    }
}

async fn process_table() -> Result<Table, String> {
    let mut command = Command::new("/bin/ps");
    command
        .args(["-A", "-o", "pid=,ppid=,lstart="])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let child = command
        .spawn()
        .map_err(|_| "caller identity probe failed")?;
    let output = tokio::time::timeout(Duration::from_secs(1), child.wait_with_output())
        .await
        .map_err(|_| "caller identity probe timed out")?
        .map_err(|_| "caller identity probe failed")?;
    if !output.status.success() || output.stdout.len() > 2 * 1024 * 1024 {
        return Err("caller identity unavailable".into());
    }
    let mut table = BTreeMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut fields = line.split_whitespace();
        let (Some(pid), Some(parent)) = (
            fields.next().and_then(|s| s.parse().ok()),
            fields.next().and_then(|s| s.parse().ok()),
        ) else {
            continue;
        };
        table.insert(
            pid,
            Process {
                parent,
                stamp: fields.collect::<Vec<_>>().join(" "),
            },
        );
    }
    Ok(table)
}
fn belongs(pid: u32, binding: &Binding, table: &Table) -> bool {
    if table
        .get(&binding.pid)
        .is_none_or(|p| p.stamp != binding.stamp)
    {
        return false;
    }
    let mut current = pid;
    for _ in 0..64 {
        if current == binding.pid {
            return true;
        }
        let Some(process) = table.get(&current) else {
            return false;
        };
        if process.parent == current || process.parent < 2 {
            return false;
        }
        current = process.parent;
    }
    false
}
async fn owner<F: Future<Output = Result<Table, String>>>(
    pid: u32,
    bindings: &Bindings,
    probe: impl Fn() -> F,
) -> Result<String, String> {
    let table = probe().await?;
    bindings
        .lock()
        .await
        .iter()
        .rev()
        .find(|b| belongs(pid, b, &table))
        .map(|b| b.owner.clone())
        .ok_or_else(|| "scopeDenied: launch this lead with iface workers launch".into())
}
async fn frame(
    reader: &mut BufReader<impl tokio::io::AsyncRead + Unpin>,
) -> Result<Option<Value>, String> {
    let mut bytes = Vec::new();
    let size = reader
        .take(FRAME_LIMIT + 1)
        .read_until(b'\n', &mut bytes)
        .await
        .map_err(|e| e.to_string())?;
    if size == 0 {
        return Ok(None);
    }
    if size as u64 > FRAME_LIMIT || bytes.last() != Some(&b'\n') {
        return Err("invalid or oversized worker frame".into());
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| "invalid worker JSON".into())
}
async fn write(
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    value: &Value,
) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await.map_err(|e| e.to_string())?;
    writer.flush().await.map_err(|e| e.to_string())
}
/// One lead connection. `probe` reads the process table that proves the
/// caller's identity; production passes [`process_table`].
async fn connection<P, F>(
    stream: UnixStream,
    pid: u32,
    service: Arc<Service>,
    bindings: Bindings,
    probe: P,
) -> Result<(), String>
where
    P: Fn() -> F,
    F: Future<Output = Result<Table, String>>,
{
    let (read, write_half) = stream.into_split();
    let writer = Arc::new(Mutex::new(write_half));
    let mut reader = BufReader::new(read);
    let first = tokio::time::timeout(Duration::from_secs(10), frame(&mut reader))
        .await
        .map_err(|_| "worker handshake timed out")??
        .ok_or("missing worker handshake")?;
    if first["method"] == "bind" {
        if !matches!(first["client"].as_str(), Some("claude" | "codex")) {
            return Err("invalid lead client".into());
        }
        let table = probe().await?;
        let process = table.get(&pid).ok_or("caller process missing")?;
        let mut values = bindings.lock().await;
        // Registration is a launcher control operation; a child cannot rebind
        // an existing lead or select someone else's owner reference.
        if values.iter().any(|b| belongs(pid, b, &table)) {
            return Err("lead already bound".into());
        }
        values.retain(|b| table.get(&b.pid).is_some_and(|p| p.stamp == b.stamp));
        if values.len() >= 64 {
            return Err("lead binding capacity exceeded".into());
        }
        let owner = uuid::Uuid::new_v4().to_string();
        values.push(Binding {
            pid,
            stamp: process.stamp.clone(),
            owner: owner.clone(),
        });
        drop(values);
        return write(
            &mut *writer.lock().await,
            &json!({"ownerConversationRef":owner}),
        )
        .await;
    }
    let bound_owner = owner(pid, &bindings, &probe).await?;
    if first["method"] == "notice" {
        return write(
            &mut *writer.lock().await,
            &service.notice(&bound_owner).await,
        )
        .await;
    }
    if first["method"] != "initialize" {
        return Err("MCP initialize required".into());
    }
    let requested = first["params"]["protocolVersion"]
        .as_str()
        .unwrap_or_default();
    let protocol = if matches!(
        requested,
        "2024-11-05" | "2025-03-26" | "2025-06-18" | "2025-11-25"
    ) {
        requested
    } else {
        "2025-11-25"
    };
    write(&mut *writer.lock().await, &json!({"jsonrpc":"2.0","id":first["id"],"result":{"protocolVersion":protocol,"capabilities":{"tools":{}},"serverInfo":{"name":MCP_SERVER_NAME,"title":MCP_SERVER_TITLE,"version":"1.0.0"},"instructions":INSTRUCTIONS}})).await?;
    let permits = Arc::new(Semaphore::new(8));
    let mut requests = tokio::task::JoinSet::new();
    while let Some(message) = frame(&mut reader).await? {
        // Notifications are no-ops; the next request still re-proves the lead.
        if message.get("id").is_none() {
            continue;
        }
        if owner(pid, &bindings, &probe).await? != bound_owner {
            return Err("scopeDenied: lead binding changed".into());
        }
        let Ok(permit) = permits.clone().try_acquire_owned() else {
            write(&mut *writer.lock().await, &json!({"jsonrpc":"2.0","id":message["id"],"error":{"code":-32000,"message":"request capacity exceeded"}})).await?;
            continue;
        };
        let writer = writer.clone();
        let service = service.clone();
        let owner = bound_owner.clone();
        requests.spawn(async move {
            let _permit = permit;
            let result = match message["method"].as_str() {
                Some("ping") => json!({"result":{}}),
                Some("tools/list") => json!({"result":{"tools":catalogue()}}),
                Some("tools/call") => {
                    let name = message["params"]["name"].as_str().unwrap_or_default();
                    match service.call(&owner, name, message["params"].get("arguments").cloned().unwrap_or_else(|| json!({}))).await {
                        Ok(value) => json!({"result":{"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":false}}),
                        Err(error) => json!({"result":{"content":[{"type":"text","text":error}],"isError":true}}),
                    }
                }
                _ => json!({"error":{"code":-32601,"message":"method not found"}}),
            };
            let mut response = result;
            response["jsonrpc"] = json!("2.0");
            response["id"] = message["id"].clone();
            let _ = write(&mut *writer.lock().await, &response).await;
        });
        while requests.try_join_next().is_some() {}
    }
    // Admission already persisted independently; disconnect cannot replay or
    // cancel a provider effect. Pending collect requests are disposable.
    requests.abort_all();
    Ok(())
}

pub(super) async fn request(root: &Path, value: Value) -> Result<Value, String> {
    PrivateDirectory::open(root).map_err(|e| e.to_string())?;
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut stream = UnixStream::connect(root.join("broker.sock"))
            .await
            .map_err(|e| e.to_string())?;
        write(&mut stream, &value).await?;
        frame(&mut BufReader::new(stream))
            .await?
            .ok_or_else(|| "worker broker refused request".into())
    })
    .await
    .map_err(|_| "worker broker request timed out".to_owned())?
}
pub(super) async fn bridge(root: &Path) -> Result<(), String> {
    PrivateDirectory::open(root).map_err(|e| e.to_string())?;
    let stream = UnixStream::connect(root.join("broker.sock"))
        .await
        .map_err(|e| e.to_string())?;
    let (mut read, mut write) = stream.into_split();
    let incoming = async {
        tokio::io::copy(&mut tokio::io::stdin(), &mut write).await?;
        write.shutdown().await
    };
    let outgoing = async {
        tokio::io::copy(&mut read, &mut tokio::io::stdout())
            .await
            .map(|_| ())
    };
    tokio::try_join!(incoming, outgoing).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;
