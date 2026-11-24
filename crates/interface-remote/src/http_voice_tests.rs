use super::*;
use interface_contracts::voice_control::Request;

struct Backend;
impl RemoteBackend for Backend {
    fn snapshot(&self) -> Result<TerminalSnapshot, RemoteError> {
        Ok(TerminalSnapshot {
            revision: 0,
            terminals: vec![],
        })
    }
    fn enqueue(&self, _: StreamKey, _: Vec<u8>) -> Result<(), RemoteError> {
        panic!("the retired control route must not reach terminal input")
    }
}

#[tokio::test]
async fn retired_control_route_still_requires_a_phone_grant_and_then_refuses() {
    let hub = Arc::new(RemoteHub::default());
    hub.activate("http://127.0.0.1:4321".into(), None).unwrap();
    let token = hub
        .enroll(&hub.pairing_code().unwrap(), "control fixture")
        .unwrap();
    let state = Arc::new(HttpState {
        hub,
        backend: Arc::new(Backend),
        assets: BTreeMap::new(),
        streams: Arc::new(Semaphore::new(8)),
        requests: Semaphore::new(32),
        histories: Semaphore::new(2),
    });
    let request = || {
        Json(Request {
            version: 1,
            command: interface_contracts::voice_control::Command::Inspect {},
        })
    };
    let unauthenticated = voice_control(State(state.clone()), HeaderMap::new(), request())
        .await
        .unwrap_err();
    assert_eq!(unauthenticated.code, "unauthorized");
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        format!("interface_remote={token}").parse().unwrap(),
    );
    let retired = voice_control(State(state), headers, request())
        .await
        .unwrap_err();
    assert_eq!(retired.code, "voice-control-unavailable");
    assert!(retired.replay_safe);
}
