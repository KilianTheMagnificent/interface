use interface_collaboration::broker::{BrokerError, BrokerRuntime};
use interface_collaboration::state::{CollaborationEngineId, CollaborationErrorCode};
use interface_collaboration::{
    CollaborationBrokerRuntime, CollaborationBrokerRuntimeOptions, McpIngressError,
};

const CODEX_CAPABILITY: &str = "codex-capability-0000000000000001";
const CLAUDE_CAPABILITY: &str = "claude-capability-00000000000001";
const GROK_CAPABILITY: &str = "grok-capability-0000000000000001";

#[test]
fn source_lineage_is_stable_and_domain_separated() {
    assert_eq!(
        interface_collaboration::broker::collaboration_source_lineage_id("codex#thread-1"),
        "73e023c4ebd3da3e35ffcaf617eab096b68a671f22e60d241802ceca6365eef4"
    );
}

#[test]
fn claude_binding_has_no_authority_until_exact_source_is_confirmed() {
    let mut broker = BrokerRuntime::new(CODEX_CAPABILITY).expect("broker");
    let binding = broker
        .create_session_binding(
            CollaborationEngineId::Claude,
            "session-1",
            false,
            CLAUDE_CAPABILITY,
        )
        .expect("binding");
    assert_eq!(
        broker
            .resolve_model_authority(CLAUDE_CAPABILITY)
            .expect("authority")
            .source_session_keys,
        Vec::<String>::new()
    );
    assert_eq!(
        broker.confirm_source_session_key(binding, "claude#different"),
        Err(BrokerError::SourceMismatch)
    );
    broker
        .confirm_source_session_key(binding, "claude#session-1")
        .expect("confirm");
    broker
        .confirm_source_session_key(binding, "claude#session-1")
        .expect("idempotent");
    let authority = broker
        .resolve_model_authority(CLAUDE_CAPABILITY)
        .expect("authority");
    assert_eq!(authority.engine, CollaborationEngineId::Claude);
    assert_eq!(authority.source_session_keys, ["claude#session-1"]);
    assert_eq!(authority.depth, 0);
    assert_eq!(authority.source_lineage_ids.len(), 1);

    broker.dispose_session_binding(binding).expect("dispose");
    assert!(broker.resolve_model_authority(CLAUDE_CAPABILITY).is_none());
    // A disposed binding is dropped outright; every reader still treats it as
    // revoked, and its capability is never reissued.
    assert_eq!(
        broker.confirm_source_session_key(binding, "claude#session-1"),
        Err(BrokerError::BindingInactive)
    );
    broker
        .dispose_session_binding(binding)
        .expect("second dispose");
    assert_eq!(
        broker
            .create_session_binding(
                CollaborationEngineId::Claude,
                "session-1",
                false,
                CLAUDE_CAPABILITY,
            )
            .err(),
        Some(BrokerError::DuplicateCapability)
    );
}

#[test]
fn grok_new_session_can_confirm_final_id_but_resume_is_exact() {
    let mut broker = BrokerRuntime::new(CODEX_CAPABILITY).expect("broker");
    let new_binding = broker
        .create_session_binding(
            CollaborationEngineId::AcpGrok,
            "provisional-1",
            false,
            GROK_CAPABILITY,
        )
        .expect("new binding");
    broker
        .confirm_source_session_key(new_binding, "acp:grok#final-1")
        .expect("new final ID");

    let resumed_capability = "grok-capability-0000000000000002";
    let resumed = broker
        .create_session_binding(
            CollaborationEngineId::AcpGrok,
            "resume-1",
            true,
            resumed_capability,
        )
        .expect("resume binding");
    assert_eq!(
        broker.confirm_source_session_key(resumed, "acp:grok#other"),
        Err(BrokerError::SourceMismatch)
    );
    broker
        .confirm_source_session_key(resumed, "acp:grok#resume-1")
        .expect("exact resume");
}

#[test]
fn codex_sources_are_independent_and_depth_fence_revokes_all_authority() {
    let mut broker = BrokerRuntime::new(CODEX_CAPABILITY).expect("broker");
    let first = broker
        .register_codex_source("codex#thread-1")
        .expect("source one");
    let second = broker
        .register_codex_source("codex#thread-2")
        .expect("source two");
    let authority = broker
        .resolve_model_authority(CODEX_CAPABILITY)
        .expect("authority");
    assert_eq!(
        authority.source_session_keys,
        ["codex#thread-1", "codex#thread-2"]
    );

    let fence = broker.enter_codex_depth_fence().expect("fence");
    assert!(
        broker
            .resolve_model_authority(CODEX_CAPABILITY)
            .expect("fenced authority")
            .source_session_keys
            .is_empty()
    );
    broker.leave_codex_depth_fence(fence);
    broker.dispose_codex_source(first).expect("dispose first");
    assert_eq!(
        broker
            .resolve_model_authority(CODEX_CAPABILITY)
            .expect("authority")
            .source_session_keys,
        ["codex#thread-2"]
    );
    broker.dispose_codex_source(second).expect("dispose second");
}

#[test]
fn duplicate_capabilities_and_duplicate_source_owners_fail_closed() {
    let mut broker = BrokerRuntime::new(CODEX_CAPABILITY).expect("broker");
    assert_eq!(
        broker.create_session_binding(
            CollaborationEngineId::Claude,
            "session-1",
            false,
            CODEX_CAPABILITY,
        ),
        Err(BrokerError::DuplicateCapability)
    );
    let binding = broker
        .create_session_binding(
            CollaborationEngineId::Claude,
            "session-1",
            false,
            CLAUDE_CAPABILITY,
        )
        .expect("binding");
    broker
        .confirm_source_session_key(binding, "claude#session-1")
        .expect("confirm");
    assert_eq!(
        broker.create_session_binding(
            CollaborationEngineId::Codex,
            "thread-1",
            false,
            "another-capability-00000000000001",
        ),
        Err(BrokerError::InvalidSessionRequest)
    );
}

#[test]
fn stop_revokes_authority_synchronously_and_is_idempotent() {
    let mut broker = BrokerRuntime::new(CODEX_CAPABILITY).expect("broker");
    broker
        .register_codex_source("codex#thread-1")
        .expect("source");
    broker.stop();
    assert!(broker.resolve_model_authority(CODEX_CAPABILITY).is_none());
    assert_eq!(
        broker.register_codex_source("codex#thread-2"),
        Err(BrokerError::Stopping)
    );
    broker.stop();
}

struct RefusingIngress;

#[async_trait::async_trait]
impl interface_collaboration::CollaborationModelIngress for RefusingIngress {
    async fn submit(
        &self,
        _intent: interface_collaboration::CollaborationMcpIntent,
        _authority: interface_collaboration::broker::CollaborationModelAuthority,
        _cancellation: interface_collaboration::IngressCancellation,
    ) -> Result<interface_collaboration::state::CollaborationReceiptV1, McpIngressError> {
        Err(McpIngressError::new(
            CollaborationErrorCode::BrokerUnavailable,
        ))
    }

    async fn status(
        &self,
        _request: interface_collaboration::CollaborationMcpAccessRequest,
        _authority: interface_collaboration::broker::CollaborationModelAuthority,
        _cancellation: interface_collaboration::IngressCancellation,
    ) -> Result<interface_collaboration::state::CollaborationReceiptV1, McpIngressError> {
        Err(McpIngressError::new(
            CollaborationErrorCode::BrokerUnavailable,
        ))
    }

    async fn cancel(
        &self,
        _request: interface_collaboration::CollaborationMcpAccessRequest,
        _authority: interface_collaboration::broker::CollaborationModelAuthority,
        _cancellation: interface_collaboration::IngressCancellation,
    ) -> Result<interface_collaboration::state::CollaborationReceiptV1, McpIngressError> {
        Err(McpIngressError::new(
            CollaborationErrorCode::BrokerUnavailable,
        ))
    }
}

#[tokio::test]
async fn start_removes_dead_sockets_and_leaves_live_ones_and_other_files() {
    use std::os::unix::{fs::PermissionsExt, net::UnixListener};

    let private = |path: &std::path::Path, mode: u32| {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    };
    let present = |path: &std::path::Path| std::fs::symlink_metadata(path).is_ok();
    // A short path: socket addresses are limited to 104 bytes.
    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let root = temp.path().join("collaboration");
    let sockets = root.join("sockets");
    std::fs::create_dir_all(&sockets).unwrap();
    private(&sockets, 0o700);
    // A host that was killed left its socket behind; nothing listens on it.
    let dead = sockets.join("deadsocket000001.sock");
    drop(UnixListener::bind(&dead).unwrap());
    private(&dead, 0o600);
    // A listener that still accepts is never removed.
    let live = sockets.join("livesocket000001.sock");
    let listener = UnixListener::bind(&live).unwrap();
    private(&live, 0o600);
    // Neither is anything that is not exactly a broker socket.
    let regular = sockets.join("regularfile00001.sock");
    std::fs::write(&regular, b"not a socket").unwrap();
    private(&regular, 0o600);
    let shared = sockets.join("sharedmode000001.sock");
    drop(UnixListener::bind(&shared).unwrap());
    private(&shared, 0o644);
    let unnamed = sockets.join("short.sock");
    drop(UnixListener::bind(&unnamed).unwrap());
    private(&unnamed, 0o600);

    let broker = CollaborationBrokerRuntime::start(CollaborationBrokerRuntimeOptions::frozen(
        root.clone(),
        std::sync::Arc::new(RefusingIngress),
    ))
    .await
    .expect("broker");

    assert!(!present(&dead));
    for kept in [&live, &regular, &shared, &unnamed] {
        assert!(present(kept), "{} was removed", kept.display());
    }
    assert!(std::os::unix::net::UnixStream::connect(&live).is_ok());
    broker.stop().await.expect("stop");
    drop(listener);
}
