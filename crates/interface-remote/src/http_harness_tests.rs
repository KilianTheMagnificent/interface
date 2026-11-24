use super::*;
use interface_contracts::harness::{AttentionTarget, SharedDraft, TaskEvidence};
use std::sync::{
    Mutex as StdMutex,
    atomic::{AtomicBool, Ordering},
};
struct Backend {
    snapshot: StdMutex<TerminalSnapshot>,
    draft: StdMutex<SharedDraft>,
    change_generation: AtomicBool,
}
impl RemoteBackend for Backend {
    fn snapshot(&self) -> Result<TerminalSnapshot, RemoteError> {
        Ok(self.snapshot.lock().unwrap().clone())
    }
    fn enqueue(&self, _: StreamKey, _: Vec<u8>) -> Result<(), RemoteError> {
        panic!("Draft/result routes must never send input")
    }
    fn results(&self, _: StreamKey) -> Result<Vec<TaskEvidence>, RemoteError> {
        if self.change_generation.load(Ordering::SeqCst) {
            self.snapshot.lock().unwrap().terminals[0].generation =
                interface_contracts::terminal::TerminalGeneration(4);
        }
        Ok(vec![])
    }
    fn draft(&self, _: StreamKey, edit: Option<(u64, String)>) -> Result<SharedDraft, RemoteError> {
        let mut draft = self.draft.lock().unwrap();
        if let Some((revision, text)) = edit {
            if draft.revision != revision {
                return Err(error("draft-conflict", "Draft changed"));
            }
            draft.revision += 1;
            draft.text = text;
        }
        Ok(draft.clone())
    }
}
fn fixture() -> (Arc<HttpState>, Arc<Backend>, HeaderMap) {
    let hub = Arc::new(RemoteHub::default());
    hub.activate("http://127.0.0.1:4321".into(), None).unwrap();
    let token = hub
        .enroll(&hub.pairing_code().unwrap(), "Fixture phone")
        .unwrap();
    let terminal = crate::workspaces::tests::terminal();
    let backend = Arc::new(Backend {
        draft: StdMutex::new(SharedDraft {
            target: AttentionTarget::Terminal {
                terminal_id: terminal.id.to_string(),
                generation: terminal.generation.0,
            },
            revision: 1,
            text: "desktop".into(),
        }),
        snapshot: StdMutex::new(TerminalSnapshot {
            revision: 1,
            terminals: vec![terminal],
        }),
        change_generation: AtomicBool::new(false),
    });
    let state = Arc::new(HttpState {
        hub,
        backend: backend.clone(),
        assets: BTreeMap::new(),
        streams: Arc::new(Semaphore::new(8)),
        requests: Semaphore::new(32),
        histories: Semaphore::new(2),
    });
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        format!("interface_remote={token}").parse().unwrap(),
    );
    (state, backend, headers)
}
fn route(generation: u64) -> Path<(String, u64)> {
    Path((
        crate::workspaces::tests::terminal().id.to_string(),
        generation,
    ))
}
#[tokio::test]
async fn draft_and_results_require_pairing_generation_and_compare_and_set() {
    let (state, backend, headers) = fixture();
    assert!(
        read_draft(State(state.clone()), route(3), HeaderMap::new())
            .await
            .is_err()
    );
    assert!(
        results(State(state.clone()), route(2), headers.clone())
            .await
            .is_err()
    );
    let draft = read_draft(State(state.clone()), route(3), headers.clone())
        .await
        .unwrap()
        .0;
    assert_eq!(draft.text, "desktop");
    let _ = save_draft(
        State(state.clone()),
        route(3),
        headers.clone(),
        Json(DraftEdit {
            expected_revision: 1,
            text: "phone".into(),
        }),
    )
    .await
    .unwrap();
    assert!(
        save_draft(
            State(state.clone()),
            route(3),
            headers.clone(),
            Json(DraftEdit {
                expected_revision: 1,
                text: "stale desktop".into()
            })
        )
        .await
        .is_err()
    );
    assert_eq!(backend.draft.lock().unwrap().text, "phone");
    assert!(
        save_draft(
            State(state.clone()),
            route(2),
            headers.clone(),
            Json(DraftEdit {
                expected_revision: 2,
                text: "replacement".into()
            })
        )
        .await
        .is_err()
    );
    assert_eq!(backend.draft.lock().unwrap().revision, 2);
    backend.change_generation.store(true, Ordering::SeqCst);
    assert!(
        results(State(state.clone()), route(3), headers.clone())
            .await
            .is_err(),
        "a replaced generation returned an old result"
    );
    state.hub.disable();
    assert!(read_draft(State(state), route(4), headers).await.is_err());
}

async fn raw_get(address: std::net::SocketAddr, host: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    stream
        .write_all(
            format!("GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes(),
        )
        .await
        .unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).await.unwrap();
    reply
}

#[tokio::test]
async fn every_response_carries_the_fixed_security_headers_and_assets_keep_their_bytes() {
    let (state, _, _) = fixture();
    let mut state = Arc::into_inner(state).unwrap();
    state.assets.insert(
        "/",
        (
            "text/html; charset=utf-8",
            Bytes::from_static(b"<p>fixture</p>"),
        ),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, routes(Arc::new(state)))
            .await
            .unwrap();
    });
    let page = raw_get(address, "127.0.0.1:4321").await;
    let refused = raw_get(address, "attacker.example").await;
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(page.contains("content-type: text/html; charset=utf-8\r\n"));
    assert!(page.ends_with("\r\n\r\n<p>fixture</p>"), "{page}");
    assert!(refused.starts_with("HTTP/1.1 403"), "{refused}");
    for reply in [page, refused] {
        for header in [
            "cache-control: no-store",
            "content-security-policy: default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; font-src 'self' data:; manifest-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
            "x-content-type-options: nosniff",
            "referrer-policy: no-referrer",
        ] {
            assert!(
                reply.contains(&format!("{header}\r\n")),
                "{header} in {reply}"
            );
        }
    }
    server.abort();
}
