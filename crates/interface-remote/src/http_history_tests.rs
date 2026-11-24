use super::*;
use std::sync::{
    Mutex as StdMutex,
    atomic::{AtomicU8, Ordering},
};

struct Backend {
    snapshot: StdMutex<TerminalSnapshot>,
    reads: StdMutex<Vec<(String, u64)>>,
    effect: AtomicU8,
    hub: Arc<RemoteHub>,
}
impl RemoteBackend for Backend {
    fn snapshot(&self) -> Result<TerminalSnapshot, RemoteError> {
        Ok(self.snapshot.lock().unwrap().clone())
    }
    fn enqueue(&self, _: StreamKey, _: Vec<u8>) -> Result<(), RemoteError> {
        panic!("read-only route must never enqueue input")
    }
    fn history(&self, key: String, limit: u64) -> HistoryFuture<'_> {
        Box::pin(async move {
            self.reads.lock().unwrap().push((key, limit));
            match self.effect.load(Ordering::SeqCst) {
                1 => self.hub.disable(),
                2 => {
                    self.snapshot.lock().unwrap().terminals[0]
                        .launch_session
                        .as_mut()
                        .unwrap()
                        .id = Some("different-chat".into());
                }
                3 => return Ok(unavailable_history("x".repeat(2 * 1024 * 1024))),
                4 => return Ok(page_just_under_the_body_limit()),
                _ => (),
            }
            Ok(HistoryPage::empty())
        })
    }
}
/// A page whose own JSON fits the 2 MiB body limit while its envelope does not.
fn page_just_under_the_body_limit() -> HistoryPage {
    let overhead = serde_json::to_vec(&unavailable_history(String::new()))
        .unwrap()
        .len();
    let page = unavailable_history("x".repeat(PHONE_BODY_LIMIT - 10 - overhead));
    assert_eq!(
        serde_json::to_vec(&page).unwrap().len(),
        PHONE_BODY_LIMIT - 10
    );
    page
}
fn fixture() -> (Arc<HttpState>, Arc<Backend>, HeaderMap) {
    let hub = Arc::new(RemoteHub::default());
    hub.activate("http://127.0.0.1:4321".into(), None).unwrap();
    let token = hub
        .enroll(&hub.pairing_code().unwrap(), "Test phone")
        .unwrap();
    let backend = Arc::new(Backend {
        snapshot: StdMutex::new(TerminalSnapshot {
            revision: 1,
            terminals: vec![crate::workspaces::tests::terminal()],
        }),
        reads: StdMutex::new(Vec::new()),
        effect: AtomicU8::new(0),
        hub: hub.clone(),
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
async fn read(
    state: Arc<HttpState>,
    headers: HeaderMap,
    generation: u64,
    limit: u64,
) -> Result<RemoteHistoryV2, RemoteError> {
    let response = history(
        State(state),
        Path((
            crate::workspaces::tests::terminal().id.to_string(),
            generation,
        )),
        Query(HistoryQuery { limit: Some(limit) }),
        headers,
    )
    .await?;
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(body.len() <= PHONE_BODY_LIMIT);
    Ok(serde_json::from_slice(&body).unwrap())
}
#[tokio::test]
async fn history_requires_pairing_current_generation_and_confirmed_binding() {
    let (state, backend, headers) = fixture();
    assert!(read(state.clone(), HeaderMap::new(), 3, 500).await.is_err());
    assert!(
        workspaces(State(state.clone()), HeaderMap::new())
            .await
            .is_err()
    );
    assert!(read(state.clone(), headers.clone(), 2, 500).await.is_err());
    assert!(backend.reads.lock().unwrap().is_empty());
    let page = read(state.clone(), headers.clone(), 3, 1).await.unwrap();
    assert_eq!(page.version, 2);
    assert_eq!(
        backend.reads.lock().unwrap().as_slice(),
        &[("claude#confirmed-provider-chat".into(), 500)]
    );
    let _ = read(state.clone(), headers.clone(), 3, u64::MAX)
        .await
        .unwrap();
    assert_eq!(backend.reads.lock().unwrap()[1].1, 5000);
    backend.snapshot.lock().unwrap().terminals[0].launch_session = None;
    let result = read(state, headers, 3, 500).await.unwrap();
    assert_eq!(
        result.history.availability,
        Some(interface_contracts::ipc::HistoryAvailability::Unavailable)
    );
    assert_eq!(
        backend.reads.lock().unwrap().len(),
        2,
        "never fall back to the workspace session key"
    );
}
#[tokio::test]
async fn history_rechecks_grant_and_binding_after_read_and_caps_large_responses() {
    // Effect 4: the page alone fits, but the envelope the tunnel forwards
    // would not, so the phone gets the limit page instead of a failure.
    for effect in [1, 2, 3, 4] {
        let (state, backend, headers) = fixture();
        backend.effect.store(effect, Ordering::SeqCst);
        let result = read(state, headers, 3, 500).await;
        if effect >= 3 {
            let page = result.unwrap();
            assert!(page.history.reason.unwrap().contains("phone limit"));
        } else {
            assert!(result.is_err());
        }
    }
}
#[tokio::test]
async fn history_admission_is_bounded_before_provider_io() {
    let (state, backend, headers) = fixture();
    let permits = state.histories.acquire_many(2).await.unwrap();
    assert!(read(state.clone(), headers, 3, 500).await.is_err());
    assert!(backend.reads.lock().unwrap().is_empty());
    drop(permits);
}
