use super::*;
use std::sync::Mutex as StdMutex;

struct Backend {
    terminal: interface_contracts::terminal::TerminalSummary,
    inputs: StdMutex<Vec<Vec<u8>>>,
}
impl RemoteBackend for Backend {
    fn snapshot(&self) -> Result<TerminalSnapshot, RemoteError> {
        Ok(TerminalSnapshot {
            revision: 1,
            terminals: vec![self.terminal.clone()],
        })
    }
    fn enqueue(&self, _: StreamKey, bytes: Vec<u8>) -> Result<(), RemoteError> {
        self.inputs.lock().unwrap().push(bytes);
        Ok(())
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One ordered authority/receipt lifecycle.
async fn responses_require_trust_exact_generation_current_screen_and_single_use_authority() {
    let hub = Arc::new(RemoteHub::default());
    hub.activate("http://127.0.0.1:4321".into(), None).unwrap();
    let grant = hub.enroll(&hub.pairing_code().unwrap(), "Phone").unwrap();
    let device = hub.authenticate(&grant).unwrap();
    let mut terminal = crate::workspaces::tests::terminal();
    terminal.launch.as_mut().unwrap().profile_id =
        interface_contracts::terminal::TerminalLaunchProfileId::Codex;
    let key = (terminal.id.as_uuid(), terminal.generation.0);
    let backend = Arc::new(Backend {
        terminal,
        inputs: StdMutex::new(Vec::new()),
    });
    let state = Arc::new(HttpState {
        hub: hub.clone(),
        backend: backend.clone(),
        assets: BTreeMap::new(),
        streams: Arc::new(Semaphore::new(8)),
        requests: Semaphore::new(32),
        histories: Semaphore::new(2),
    });
    hub.replay.append(key, &[b'x'; 100]);
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../electron/src/fixtures/remote-approval-v4.json"
    ))
    .unwrap();
    let observation: ApprovalObservationV4 =
        serde_json::from_value(fixture["observation"].clone()).unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        format!("interface_remote={grant}").parse().unwrap(),
    );
    let path = || Path((key.0.to_string(), key.1));
    assert!(
        approval(
            State(state.clone()),
            path(),
            HeaderMap::new(),
            Json(observation.clone())
        )
        .await
        .is_err()
    );
    assert!(
        approval(
            State(state.clone()),
            Path((key.0.to_string(), 99)),
            headers.clone(),
            Json(observation.clone())
        )
        .await
        .is_err()
    );
    let view = approval(
        State(state.clone()),
        path(),
        headers.clone(),
        Json(observation.clone()),
    )
    .await
    .unwrap()
    .0
    .approval
    .unwrap();
    let mut response: ApprovalResponseV4 =
        serde_json::from_value(fixture["response"].clone()).unwrap();
    response.approval_id = view.id;
    headers.insert(
        "x-interface-control",
        hub.claim(device, key).unwrap().to_string().parse().unwrap(),
    );
    headers.insert(
        "x-interface-command",
        Uuid::new_v4().to_string().parse().unwrap(),
    );
    // The desktop taking control cancels the observed request, even without output.
    hub.desktop_input(key, || ());
    assert!(
        approval_response(
            State(state.clone()),
            path(),
            headers.clone(),
            Json(response.clone())
        )
        .await
        .is_err()
    );
    assert!(backend.inputs.lock().unwrap().is_empty());
    hub.replay.append(key, b"new");
    response.cursor = 103;
    response.approval_id = recognized_approval(&backend.terminal, key, 103, &response.screen)
        .unwrap()
        .view
        .id;
    headers.insert(
        "x-interface-control",
        hub.claim(device, key).unwrap().to_string().parse().unwrap(),
    );
    headers.insert(
        "x-interface-command",
        Uuid::new_v4().to_string().parse().unwrap(),
    );
    let mut old_response = response.clone();
    old_response.cursor = 100;
    assert!(
        approval_response(
            State(state.clone()),
            path(),
            headers.clone(),
            Json(old_response)
        )
        .await
        .is_err()
    );
    assert_eq!(
        approval_response(
            State(state.clone()),
            path(),
            headers.clone(),
            Json(response.clone())
        )
        .await
        .unwrap(),
        StatusCode::ACCEPTED
    );
    // A retry gets its receipt; a new command cannot reuse the same displayed request.
    assert_eq!(
        approval_response(
            State(state.clone()),
            path(),
            headers.clone(),
            Json(response.clone())
        )
        .await
        .unwrap(),
        StatusCode::ACCEPTED
    );
    headers.insert(
        "x-interface-command",
        Uuid::new_v4().to_string().parse().unwrap(),
    );
    assert!(
        approval_response(
            State(state.clone()),
            path(),
            headers.clone(),
            Json(response)
        )
        .await
        .is_err()
    );
    assert_eq!(*backend.inputs.lock().unwrap(), vec![b"y".to_vec()]);
    hub.revoke(device).unwrap();
    assert!(
        approval(State(state), path(), headers, Json(observation))
            .await
            .is_err()
    );
}
