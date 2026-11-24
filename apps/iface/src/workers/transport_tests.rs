use super::*;
use interface_collaboration::workers::{Policy, empty_notice};
use interface_host::report_workers::OpenCodeRunner;
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
use tokio::{
    net::unix::{OwnedReadHalf, OwnedWriteHalf},
    task::JoinHandle,
};

fn process(parent: u32, stamp: &str) -> Process {
    Process {
        parent,
        stamp: stamp.into(),
    }
}

/// A process table a test edits between frames, counting the gate's reads.
#[derive(Clone)]
struct FakeTable {
    table: Arc<std::sync::Mutex<Table>>,
    reads: Arc<AtomicUsize>,
}

impl FakeTable {
    fn new(rows: impl IntoIterator<Item = (u32, Process)>) -> Self {
        Self {
            table: Arc::new(std::sync::Mutex::new(rows.into_iter().collect())),
            reads: Arc::default(),
        }
    }
    fn probe(
        &self,
    ) -> impl Fn() -> std::future::Ready<Result<Table, String>> + Clone + Send + Sync + 'static
    {
        let fake = self.clone();
        move || {
            fake.reads.fetch_add(1, Ordering::SeqCst);
            std::future::ready(Ok(fake.table.lock().unwrap().clone()))
        }
    }
    fn set(&self, pid: u32, process: Process) {
        self.table.lock().unwrap().insert(pid, process);
    }
    fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }
}

fn service(root: &Path) -> Arc<Service> {
    Service::open(
        &root.join("workers"),
        Policy {
            source: root.into(),
            files: BTreeSet::new(),
            contributor_data_allowed: false,
        },
        Arc::new(OpenCodeRunner {
            executable: PathBuf::from("/unused"),
        }),
    )
    .unwrap()
}

/// The client end of one broker connection made by process `pid`.
struct Peer {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    task: JoinHandle<Result<(), String>>,
}

impl Peer {
    fn connect(pid: u32, service: &Arc<Service>, bindings: &Bindings, table: &FakeTable) -> Self {
        let (client, server) = UnixStream::pair().unwrap();
        let task = tokio::spawn(connection(
            server,
            pid,
            service.clone(),
            bindings.clone(),
            table.probe(),
        ));
        let (read, writer) = client.into_split();
        Self {
            reader: BufReader::new(read),
            writer,
            task,
        }
    }
    async fn send(&mut self, value: Value) {
        write(&mut self.writer, &value).await.unwrap();
    }
    /// The next reply, or `None` once the broker closed the connection.
    async fn reply(&mut self) -> Option<Value> {
        frame(&mut self.reader).await.unwrap()
    }
    async fn call(&mut self, value: Value) -> Option<Value> {
        self.send(value).await;
        self.reply().await
    }
    async fn end(self) -> Result<(), String> {
        drop(self.writer);
        self.task.await.unwrap()
    }
}

async fn bind(
    pid: u32,
    service: &Arc<Service>,
    bindings: &Bindings,
    table: &FakeTable,
) -> Result<String, String> {
    let mut peer = Peer::connect(pid, service, bindings, table);
    let reply = peer.call(json!({"method":"bind","client":"claude"})).await;
    peer.end().await?;
    Ok(reply.unwrap()["ownerConversationRef"]
        .as_str()
        .unwrap()
        .to_owned())
}

fn initialize() -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}})
}

/// 99 is the shell that launched the lead 100, whose MCP bridge is 101; 50 is
/// an unrelated process of the same user.
fn lead_table() -> FakeTable {
    FakeTable::new([
        (99, process(1, "shell")),
        (100, process(99, "lead")),
        (101, process(100, "bridge")),
        (50, process(1, "stranger")),
    ])
}

#[test]
fn identity_uses_ancestry_and_start_stamp() {
    let table = BTreeMap::from([
        (
            10,
            Process {
                parent: 1,
                stamp: "first".into(),
            },
        ),
        (
            11,
            Process {
                parent: 10,
                stamp: "child".into(),
            },
        ),
        (
            12,
            Process {
                parent: 1,
                stamp: "stranger".into(),
            },
        ),
    ]);
    let binding = Binding {
        pid: 10,
        stamp: "first".into(),
        owner: "lead".into(),
    };
    assert!(belongs(11, &binding, &table));
    assert!(!belongs(12, &binding, &table));
    assert!(!belongs(
        11,
        &Binding {
            stamp: "reused".into(),
            ..binding
        },
        &table
    ));
}

#[tokio::test]
async fn a_bound_lead_is_served_until_its_process_is_replaced() {
    let root = tempfile::tempdir_in("/private/tmp").unwrap();
    let service = service(root.path());
    let bindings = Bindings::default();
    let table = lead_table();
    bind(100, &service, &bindings, &table).await.unwrap();

    // Neither the lead nor anything it launched can bind again.
    for pid in [100, 101] {
        let mut again = Peer::connect(pid, &service, &bindings, &table);
        assert_eq!(
            again.call(json!({"method":"bind","client":"codex"})).await,
            None
        );
        assert_eq!(again.end().await, Err("lead already bound".into()));
    }
    let mut unknown = Peer::connect(50, &service, &bindings, &table);
    assert_eq!(
        unknown.call(json!({"method":"bind","client":"grok"})).await,
        None
    );
    assert_eq!(unknown.end().await, Err("invalid lead client".into()));

    // A process outside the lead gets no MCP session and no notice.
    for first in [initialize(), json!({"method":"notice"})] {
        let mut stranger = Peer::connect(50, &service, &bindings, &table);
        assert_eq!(stranger.call(first).await, None);
        assert_eq!(
            stranger.end().await,
            Err("scopeDenied: launch this lead with iface workers launch".into())
        );
    }

    let mut notice = Peer::connect(101, &service, &bindings, &table);
    assert_eq!(
        notice.call(json!({"method":"notice"})).await,
        Some(empty_notice())
    );
    assert_eq!(notice.end().await, Ok(()));

    let mut session = Peer::connect(101, &service, &bindings, &table);
    let initialized = session.call(initialize()).await.unwrap();
    assert_eq!(initialized["id"], 1);
    assert_eq!(initialized["result"]["protocolVersion"], "2025-06-18");
    let tools = session
        .call(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))
        .await
        .unwrap();
    assert_eq!(tools["id"], 2);
    assert_eq!(tools["result"]["tools"], json!(catalogue()));

    // The lead exits and its pid is reused. A notification is still a no-op
    // that reads nothing; the next request re-proves the lead and is refused.
    table.set(100, process(99, "reused"));
    let reads = table.reads();
    session
        .send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
        .await;
    session
        .send(json!({"jsonrpc":"2.0","id":3,"method":"ping"}))
        .await;
    assert_eq!(session.reply().await, None);
    assert_eq!(table.reads(), reads + 1);
    assert_eq!(
        session.end().await,
        Err("scopeDenied: launch this lead with iface workers launch".into())
    );
}

#[tokio::test]
async fn a_newer_binding_over_the_lead_ends_its_open_session() {
    let root = tempfile::tempdir_in("/private/tmp").unwrap();
    let service = service(root.path());
    let bindings = Bindings::default();
    let table = lead_table();
    let lead = bind(100, &service, &bindings, &table).await.unwrap();
    let mut session = Peer::connect(101, &service, &bindings, &table);
    assert!(session.call(initialize()).await.is_some());
    assert!(
        session
            .call(json!({"jsonrpc":"2.0","id":2,"method":"ping"}))
            .await
            .is_some()
    );

    // The shell above the lead is not inside any binding, so it may bind; the
    // bridge now resolves to that newer owner and its session is refused.
    let shell = bind(99, &service, &bindings, &table).await.unwrap();
    assert_ne!(shell, lead);
    assert_eq!(
        session
            .call(json!({"jsonrpc":"2.0","id":3,"method":"ping"}))
            .await,
        None
    );
    assert_eq!(
        session.end().await,
        Err("scopeDenied: lead binding changed".into())
    );
}

#[tokio::test]
async fn binding_capacity_is_bounded_and_reclaims_replaced_leads() {
    let root = tempfile::tempdir_in("/private/tmp").unwrap();
    let service = service(root.path());
    let bindings = Bindings::default();
    let table = FakeTable::new((1000..=1064).map(|pid| (pid, process(1, &format!("lead-{pid}")))));
    for pid in 1000..1064 {
        bind(pid, &service, &bindings, &table).await.unwrap();
    }
    assert_eq!(
        bind(1064, &service, &bindings, &table).await,
        Err("lead binding capacity exceeded".into())
    );
    table.set(1000, process(1, "replaced"));
    bind(1064, &service, &bindings, &table).await.unwrap();
}

#[tokio::test]
async fn a_peer_that_hangs_up_before_accept_does_not_stop_the_broker() {
    let root = tempfile::tempdir_in("/private/tmp").unwrap();
    let service = service(root.path());
    let path = root.path().join("broker.sock");
    let listener = UnixListener::bind(&path).unwrap();
    // Queued before the loop runs and gone by the time it is accepted, so its
    // credentials can no longer be read.
    drop(UnixStream::connect(&path).await.unwrap());
    let uid = std::fs::metadata(root.path()).unwrap().uid();
    let table = FakeTable::new([(std::process::id(), process(1, "test"))]);
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let broker = tokio::spawn({
        let service = service.clone();
        let probe = table.probe();
        async move { accept_leads(&listener, uid, &service, probe, stopped).await }
    });

    let (read, mut writer) = UnixStream::connect(&path).await.unwrap().into_split();
    write(&mut writer, &json!({"method":"bind","client":"claude"}))
        .await
        .unwrap();
    let reply = frame(&mut BufReader::new(read)).await.unwrap();
    assert!(reply.expect("the broker still serves")["ownerConversationRef"].is_string());
    stop.send(()).unwrap();
    broker.await.unwrap();
}

#[tokio::test]
async fn concurrent_requests_share_one_process_read_taken_after_they_asked() {
    let table = lead_table();
    let gate = Arc::new(tokio::sync::Notify::new());
    let shared = SharedProbe::new({
        let (table, gate) = (table.clone(), gate.clone());
        move || {
            let (table, gate) = (table.clone(), gate.clone());
            async move {
                // The first read is still running when every caller asks.
                if table.reads() == 0 {
                    gate.notified().await;
                }
                table.probe()().await
            }
        }
    });
    let release = async {
        tokio::task::yield_now().await;
        gate.notify_one();
    };
    let (first, second, third, fourth, fifth, sixth, ()) = tokio::join!(
        shared.read(),
        shared.read(),
        shared.read(),
        shared.read(),
        shared.read(),
        shared.read(),
        release
    );
    for result in [first, second, third, fourth, fifth, sixth] {
        assert_eq!(result.unwrap()[&100].stamp, "lead");
    }
    // The read in flight when the others asked, then one they all share.
    assert_eq!(table.reads(), 2);

    // Nothing is reused once read: the lead's replacement is seen at once.
    table.set(100, process(99, "reused"));
    assert_eq!(shared.read().await.unwrap()[&100].stamp, "reused");
    assert_eq!(table.reads(), 3);
}
