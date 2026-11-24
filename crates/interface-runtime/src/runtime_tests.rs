use super::*;
use interface_contracts::engine::EngineSessionListing;
use std::{
    collections::HashMap,
    os::unix::fs::PermissionsExt,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

struct ClosingHandle {
    engine: EngineId,
    closes: Arc<AtomicUsize>,
    refuse_close: bool,
}

#[async_trait::async_trait]
impl SessionHandle for ClosingHandle {
    fn engine(&self) -> &EngineId {
        &self.engine
    }

    fn session_id(&self) -> String {
        "overflow".into()
    }

    async fn ready(&self) -> Result<interface_engine::ReadySession, EngineError> {
        Err(EngineError::definitely_unsent(
            EngineErrorKind::Lifecycle,
            "not used by this fixture",
            false,
        ))
    }

    fn take_events(&mut self) -> Result<BoundedEventStream<EngineEvent>, EngineError> {
        Err(EngineError::definitely_unsent(
            EngineErrorKind::Lifecycle,
            "not used by this fixture",
            false,
        ))
    }

    async fn send(&self, _text: String) -> DeliveryOutcome<()> {
        DeliveryOutcome::Confirmed(())
    }

    async fn interrupt(&self) -> DeliveryOutcome<()> {
        DeliveryOutcome::Confirmed(())
    }

    async fn close(&self) -> Result<(), EngineError> {
        self.closes.fetch_add(1, Ordering::AcqRel);
        if self.refuse_close {
            return Err(EngineError::definitely_unsent(
                EngineErrorKind::Lifecycle,
                "close refused by the fixture",
                false,
            ));
        }
        Ok(())
    }

    async fn respond_to_approval(
        &self,
        _approval_id: String,
        _verdict: ApprovalVerdict,
        _response: ApprovalResponse,
    ) -> DeliveryOutcome<()> {
        DeliveryOutcome::Confirmed(())
    }
}

struct StubAdapter {
    id: EngineId,
}

#[async_trait::async_trait]
impl EngineAdapter for StubAdapter {
    fn id(&self) -> &EngineId {
        &self.id
    }

    async fn probe(&self) -> Result<EngineInfo, EngineError> {
        Err(EngineError::definitely_unsent(
            EngineErrorKind::Lifecycle,
            "not used by this fixture",
            false,
        ))
    }

    async fn list_sessions(&self) -> Result<EngineSessionListing, EngineError> {
        Err(EngineError::definitely_unsent(
            EngineErrorKind::Lifecycle,
            "not used by this fixture",
            false,
        ))
    }

    fn create_session(
        &self,
        _options: LaunchOptions,
    ) -> Result<Box<dyn SessionHandle>, EngineError> {
        Err(EngineError::definitely_unsent(
            EngineErrorKind::Lifecycle,
            "not used by this fixture",
            false,
        ))
    }

    fn resume_session(
        &self,
        _session_id: String,
        _cwd: String,
        _options: ResumeOptions,
    ) -> Result<Box<dyn SessionHandle>, EngineError> {
        Err(EngineError::definitely_unsent(
            EngineErrorKind::Lifecycle,
            "not used by this fixture",
            false,
        ))
    }

    fn approval_facts(
        &self,
        tool: &str,
        _input: &serde_json::Value,
    ) -> interface_contracts::agents::ApprovalFacts {
        interface_contracts::agents::ApprovalFacts::Shell {
            command: tool.to_owned(),
            cwd: None,
        }
    }
}

/// A runtime over a private temporary registry and history; `register`
/// adds the test's adapters before it is built. Keep the directory alive.
fn test_runtime(
    register: impl FnOnce(&mut SessionRuntimeBuilder),
) -> (tempfile::TempDir, SessionRuntime) {
    let temporary = tempfile::tempdir().expect("temporary runtime directory");
    std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private runtime directory");
    let registry = SessionRegistry::open(temporary.path()).expect("session registry");
    let history = EngineHistoryStore::open(
        temporary.path().join("history"),
        crate::HistoryLimits::default(),
    )
    .expect("engine history");
    let mut builder = SessionRuntime::builder(RuntimeServices::new(registry, history));
    register(&mut builder);
    (temporary, builder.build().expect("runtime"))
}

fn closing_handle(closes: Arc<AtomicUsize>, refuse_close: bool) -> ClosingHandle {
    ClosingHandle {
        engine: EngineId::new("codex").expect("engine"),
        closes,
        refuse_close,
    }
}

/// Registers `handle` as the live attachment for `key` in `state` and
/// returns it with its subscriber's event stream.
async fn attach_live(
    runtime: &SessionRuntime,
    key: &str,
    handle: ClosingHandle,
    state: u8,
) -> (Arc<LiveSession>, BoundedEventStream<RuntimeEventFrame>) {
    let (subscriber, events) = bounded_event_stream(4);
    let live = Arc::new(LiveSession::new(
        RuntimeSession {
            key: key.into(),
            engine: handle.engine.clone(),
            session_id: "overflow".into(),
            ready_at: "2026-08-27T00:00:00.000Z".into(),
        },
        Arc::new(handle),
        subscriber,
        None,
        state,
    ));
    runtime
        .inner
        .live
        .write()
        .await
        .insert(key.to_owned(), Arc::clone(&live));
    (live, events)
}

#[test]
fn approval_facts_delegates_to_the_session_engine_adapter_and_defaults_to_unknown() {
    let (_temporary, runtime) = test_runtime(|builder| {
        builder
            .register_adapter(StubAdapter {
                id: EngineId::new("codex").expect("engine"),
            })
            .expect("register stub adapter");
    });

    let facts = runtime.approval_facts("codex#one", "run-tests", &serde_json::Value::Null);
    assert_eq!(
        facts,
        interface_contracts::agents::ApprovalFacts::Shell {
            command: "run-tests".into(),
            cwd: None,
        }
    );

    let unregistered =
        runtime.approval_facts("acp:grok#one", "run-tests", &serde_json::Value::Null);
    assert_eq!(
        unregistered,
        interface_contracts::agents::ApprovalFacts::Unknown {
            tool: "run-tests".into(),
        }
    );

    let malformed_key =
        runtime.approval_facts("no-separator", "run-tests", &serde_json::Value::Null);
    assert_eq!(
        malformed_key,
        interface_contracts::agents::ApprovalFacts::Unknown {
            tool: "run-tests".into(),
        }
    );
}

#[test]
fn completed_generation_cannot_evict_a_replacement() {
    let completed = Arc::new(());
    let replacement = Arc::new(());
    let key = String::from("codex#same");
    let mut attachments = HashMap::from([(key.clone(), replacement.clone())]);

    assert!(!remove_if_current(&mut attachments, &key, &completed));
    assert!(Arc::ptr_eq(
        attachments.get(&key).expect("replacement retained"),
        &replacement
    ));
    assert!(remove_if_current(&mut attachments, &key, &replacement));
    assert!(attachments.is_empty());
}

#[tokio::test]
async fn provider_overflow_closes_and_evicts_before_terminal() {
    let (_temporary, runtime) = test_runtime(|_| {});
    let closes = Arc::new(AtomicUsize::new(0));
    let (live, mut events) = attach_live(
        &runtime,
        "codex#overflow",
        closing_handle(Arc::clone(&closes), false),
        LIVE_ACTIVE,
    )
    .await;
    let (sender, ingress) = mpsc::channel(1);
    sender
        .send(ProviderIngress::Overflow { capacity: 1 })
        .await
        .expect("overflow ingress");
    drop(sender);

    run_event_pump(
        runtime.inner.services.clone(),
        Arc::clone(&runtime.inner.project_roots),
        Arc::new(SystemRuntimeClock),
        Arc::downgrade(&runtime.inner),
        live,
        ingress,
    )
    .await;

    assert_eq!(closes.load(Ordering::Acquire), 1);
    assert!(
        !runtime
            .inner
            .live
            .read()
            .await
            .contains_key("codex#overflow")
    );
    assert!(matches!(
        events.recv().await,
        Ok(Some(RuntimeEventFrame::Terminal {
            reason: RuntimeEventTermination::ProviderOverflow,
            ..
        }))
    ));
}

#[tokio::test]
async fn a_refused_close_keeps_the_quarantined_state() {
    let (_temporary, runtime) = test_runtime(|_| {});
    let (live, _events) = attach_live(
        &runtime,
        "codex#quarantined",
        closing_handle(Arc::new(AtomicUsize::new(0)), true),
        LIVE_OUTCOME_UNKNOWN,
    )
    .await;

    let delivery = runtime.close_session("codex#quarantined").await;

    assert!(!delivery.outcome.is_confirmed());
    assert_eq!(live.state.load(Ordering::Acquire), LIVE_OUTCOME_UNKNOWN);
}

#[test]
fn the_client_version_reaches_codex_and_every_acp_adapter() {
    let grok = AcpAdapterConfig::builtin(
        engine_acp::BuiltinAcpProvider::Grok,
        PathBuf::from("/tmp/fake-grok"),
    )
    .expect("Grok configuration");
    let qwen = AcpAdapterConfig::builtin(
        engine_acp::BuiltinAcpProvider::Qwen,
        PathBuf::from("/tmp/fake-qwen"),
    )
    .expect("Qwen configuration");
    let configured = NativeAdapterConfiguration {
        codex: Some(CodexAdapterConfig::new("/tmp/fake-codex")),
        claude_executable: None,
        acp: vec![grok, qwen],
    }
    .with_client_version("0.3.140");

    assert_eq!(configured.codex.unwrap().client_version, "0.3.140");
    assert_eq!(
        configured
            .acp
            .iter()
            .map(|config| config.client_version.as_str())
            .collect::<Vec<_>>(),
        ["0.3.140", "0.3.140"]
    );
    // Without a host's version the engines report their own crate version.
    assert_eq!(
        CodexAdapterConfig::new("/tmp/fake-codex").client_version,
        env!("CARGO_PKG_VERSION")
    );
}

mod buffer_policy_tests {
    use super::*;
    use crate::runtime::pump::PROVIDER_INGRESS_BYTES;
    use interface_contracts::engine::{EngineEventKind, NoticeTone};

    #[tokio::test]
    async fn ingress_budget_holds_until_the_consumer_releases_payloads() {
        let event = || EngineEvent {
            at: None,
            event: EngineEventKind::MessageDelta {
                text: "x".repeat(7 * 1024 * 1024),
                agent_key: None,
            },
        };
        let (sink, stream) = bounded_event_stream(8);
        for _ in 0..5 {
            sink.emit(event()).unwrap();
        }
        sink.close();
        let (sender, mut receiver) = mpsc::channel(16);
        let forward = tokio::spawn(forward_provider_events(stream, sender));
        let mut held = Vec::new();
        for _ in 0..4 {
            held.push(receiver.recv().await.unwrap());
        }
        tokio::task::yield_now().await;
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        drop(held.pop());
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), receiver.recv())
                .await
                .unwrap(),
            Some(ProviderIngress::Event(..))
        ));
        assert!(matches!(
            receiver.recv().await,
            Some(ProviderIngress::Closed)
        ));
        forward.await.unwrap();
    }

    #[tokio::test]
    async fn ingress_refuses_one_oversize_event_explicitly() {
        let (sink, stream) = bounded_event_stream(2);
        sink.emit(EngineEvent {
            at: None,
            event: EngineEventKind::MessageDelta {
                text: "x".repeat(PROVIDER_INGRESS_BYTES + 1),
                agent_key: None,
            },
        })
        .unwrap();
        let (sender, mut receiver) = mpsc::channel(2);
        forward_provider_events(stream, sender).await;
        assert!(matches!(
            receiver.recv().await,
            Some(ProviderIngress::ByteBudget {
                bytes: PROVIDER_INGRESS_BYTES
            })
        ));
        assert!(receiver.recv().await.is_none());
    }

    #[tokio::test]
    async fn subscriber_omission_keeps_session_sequence_and_terminal_receipt() {
        let frame = |sequence, bytes| RuntimeEventFrame::Event {
            value: RuntimeEvent {
                session_key: "codex#buffer".into(),
                sequence,
                received_at: "2026-09-24T00:00:00.000Z".into(),
                event: EngineEvent {
                    at: None,
                    event: EngineEventKind::MessageDelta {
                        text: "x".repeat(bytes),
                        agent_key: None,
                    },
                },
            },
        };
        let (sink, mut stream) = bounded_event_stream_with_budget(2000, 128 * 1024);
        for sequence in 1..=100 {
            sink.emit(frame(sequence, 64 * 1024)).unwrap();
        }
        let terminal = RuntimeEventFrame::Terminal {
            session_key: "codex#buffer".into(),
            reason: RuntimeEventTermination::Closed,
            message: None,
        };
        sink.emit(terminal.clone()).unwrap();
        sink.close();
        let RuntimeEventFrame::Event { value } = stream.recv().await.unwrap().unwrap() else {
            panic!("gap marker");
        };
        assert_eq!(value.sequence, 1);
        assert_eq!(value.session_key, "codex#buffer");
        assert!(matches!(
            value.event.event,
            EngineEventKind::Notice {
                tone: NoticeTone::Warning,
                ..
            }
        ));
        assert_eq!(stream.recv().await.unwrap(), Some(frame(100, 64 * 1024)));
        assert_eq!(stream.recv().await.unwrap(), Some(terminal));
        assert_eq!(stream.recv().await.unwrap(), None);
    }
}
