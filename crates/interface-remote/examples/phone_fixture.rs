//! Disposable Simulator fixture. Uses only synthetic conversation data and the
//! public test private scalar 1. Never connects to a provider or opens a user store.
use interface_contracts::{
    ipc::HistoryPage,
    remote::{RemoteHistoryV2, RemoteWorkspaceSnapshotV2},
    remote_connection::{
        ConnectionAction, ConnectionAdminRequest, ConnectionCommand, ConnectionConfigurationV1,
        MacProfile, PAIRING_PREFIX_V6, PairingActionV6, PairingRequestV6,
    },
    remote_threads::{RemoteThreadDetailV1, RemoteThreadsV1},
    remote_trust::{RemoteTrustData, TrustCommand},
    terminal::{TerminalAttentionV1, TerminalSnapshot},
};
use interface_remote::{
    ConnectionPersistence, HistoryFuture, InboxFuture, RemoteBackend, RemoteError, RemoteHub,
    RemoteServer, ThreadAnswerFuture, ThreadDetailFuture, ThreadsFuture, TrustPersistence,
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Default)]
struct MemoryTrust(Mutex<RemoteTrustData>);
impl TrustPersistence for MemoryTrust {
    fn load(&self) -> Result<RemoteTrustData, RemoteError> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn save(&self, value: &RemoteTrustData) -> Result<(), RemoteError> {
        *self.0.lock().unwrap() = value.clone();
        Ok(())
    }
}
struct MemoryConnection(
    Mutex<Option<ConnectionConfigurationV1>>,
    Mutex<Option<interface_contracts::remote_connection::ConnectionListenerV1>>,
);
impl ConnectionPersistence for MemoryConnection {
    fn load_listener(
        &self,
    ) -> Result<Option<interface_contracts::remote_connection::ConnectionListenerV1>, RemoteError>
    {
        Ok(self.1.lock().unwrap().clone())
    }
    fn save_listener(
        &self,
        value: &interface_contracts::remote_connection::ConnectionListenerV1,
    ) -> Result<(), RemoteError> {
        *self.1.lock().unwrap() = Some(value.clone());
        Ok(())
    }
    fn load(&self) -> Result<Option<ConnectionConfigurationV1>, RemoteError> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn save(&self, value: &ConnectionConfigurationV1) -> Result<(), RemoteError> {
        *self.0.lock().unwrap() = Some(value.clone());
        Ok(())
    }
}
struct Backend {
    snapshot: TerminalSnapshot,
    history: HistoryPage,
    /// Phone Threads v1, from the shared `remote-phone-v4.json`.
    threads: RemoteThreadsV1,
    detail: RemoteThreadDetailV1,
}
impl RemoteBackend for Backend {
    fn snapshot(&self) -> Result<TerminalSnapshot, RemoteError> {
        Ok(self.snapshot.clone())
    }
    fn history(&self, _: String, _: u64) -> HistoryFuture<'_> {
        Box::pin(async { Ok(self.history.clone()) })
    }
    fn enqueue(
        &self,
        _: interface_remote::replay::StreamKey,
        _: Vec<u8>,
    ) -> Result<(), RemoteError> {
        Ok(())
    }
    fn threads(&self) -> ThreadsFuture<'_> {
        Box::pin(async { Ok(self.threads.clone()) })
    }
    fn inbox(&self) -> InboxFuture<'_> {
        Box::pin(async {
            Ok(serde_json::from_str(include_str!(
                "../../../electron/src/fixtures/remote-inbox-v2.json"
            ))
            .unwrap())
        })
    }
    fn thread_detail(&self, key: String) -> ThreadDetailFuture<'_> {
        Box::pin(async move {
            if key == self.detail.thread.key {
                Ok(self.detail.clone())
            } else {
                Err(RemoteError::new(
                    "thread-not-listed",
                    "Not a fixture thread.",
                    true,
                ))
            }
        })
    }
    /// Takes a follow-up only for the fixture thread at rest, as the Mac does.
    fn send_to_thread(&self, key: String, _: String) -> ThreadAnswerFuture<'_> {
        Box::pin(async move {
            let at_rest = self.threads.threads.iter().any(|thread| {
                thread.key == key
                    && thread.status
                        == interface_contracts::remote_threads::RemoteThreadStatus::Idle
            });
            if at_rest {
                Ok(())
            } else {
                Err(RemoteError::new(
                    "thread-busy",
                    "This thread is busy.",
                    true,
                ))
            }
        })
    }
    /// Takes only the fixture's own request; nothing reaches any agent.
    fn answer_thread(&self, key: String, approval_id: String, _: bool) -> ThreadAnswerFuture<'_> {
        Box::pin(async move {
            let current = self
                .detail
                .approval
                .as_ref()
                .map(|approval| approval.id.as_str());
            if key == self.detail.thread.key && current == Some(approval_id.as_str()) {
                Ok(())
            } else {
                Err(RemoteError::new(
                    "approval-changed",
                    "This request has changed.",
                    true,
                ))
            }
        })
    }
}
fn command(command: ConnectionCommand) -> ConnectionAdminRequest {
    ConnectionAdminRequest {
        action: ConnectionAction::Connection,
        version: 5,
        command,
    }
}

#[tokio::main]
async fn main() {
    let descriptor = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("Pass a new temporary fixture descriptor path."),
    );
    assert!(
        !descriptor.exists(),
        "Refusing to overwrite an existing descriptor."
    );
    let data: serde_json::Value = serde_json::from_str(include_str!(
        "../../../electron/src/fixtures/remote-companion-v2.json"
    ))
    .unwrap();
    let snapshot: RemoteWorkspaceSnapshotV2 =
        serde_json::from_value(data["snapshot"].clone()).unwrap();
    let history: RemoteHistoryV2 = serde_json::from_value(data["history"].clone()).unwrap();
    let mut snapshot = TerminalSnapshot {
        revision: 1,
        terminals: snapshot
            .workspaces
            .into_iter()
            .flat_map(|w| w.terminals)
            .collect(),
    };
    // Roster v4: the fixture session needs you.
    snapshot.terminals[0].attention = Some(TerminalAttentionV1 {
        version: 1,
        since: "2026-10-14T09:12:00.000Z".into(),
    });
    let phone: serde_json::Value = serde_json::from_str(include_str!(
        "../../../electron/src/fixtures/remote-phone-v4.json"
    ))
    .unwrap();
    let hub = Arc::new(RemoteHub::default());
    hub.configure_trust(Arc::new(MemoryTrust::default()))
        .unwrap();
    let screens = snapshot
        .terminals
        .iter()
        .map(|terminal| (terminal.id.as_uuid(), terminal.generation.0))
        .collect::<Vec<_>>();
    let config = ConnectionConfigurationV1 {
        version: 1,
        enabled: false,
        private_key: format!("{:064x}", 1),
        relay_origin: None,
    };
    let server = RemoteServer::new(
        Arc::clone(&hub),
        Arc::new(Backend {
            snapshot,
            history: history.history,
            threads: serde_json::from_value(phone["threads"].clone()).unwrap(),
            detail: serde_json::from_value(phone["detail"].clone()).unwrap(),
        }),
        PathBuf::from("electron/dist-remote"),
    )
    .with_connection_store(
        Arc::new(MemoryConnection(Mutex::new(Some(config)), Mutex::new(None))),
        MacProfile::Development,
    )
    .unwrap();
    server
        .manage_connection(command(ConnectionCommand::Enable { relay_origin: None }))
        .await
        .unwrap();
    // Output is kept for phones only while access is on.
    for key in screens {
        hub.replay
            .append(key, "\u{1b}[2J\u{1b}[HClaude Code\r\n\r\n❯ ".as_bytes());
    }
    let pairing = server
        .pair_direct(PairingRequestV6 {
            action: PairingActionV6::PairDirect,
            version: 6,
        })
        .await
        .unwrap();
    let invitation = serde_json::to_value(pairing.invitation).unwrap();
    std::fs::write(
        &descriptor,
        serde_json::to_vec(
            &serde_json::json!({"fixture":true,"qr":format!("{PAIRING_PREFIX_V6}{invitation}"),"rows":pairing.qr_rows}),
        )
        .unwrap(),
    )
    .unwrap();
    println!("Synthetic phone fixture ready. No production sessions or credentials are used.");
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = tick.tick() => {
                // Deliberate test-only approval. This fixture has no provider or real session authority.
                let pending = server.manage_trust(3, TrustCommand::Status {}).unwrap().pending;
                for device in pending { server.manage_trust(3, TrustCommand::Approve { id:device.id }).unwrap(); }
            }
        }
    }
    server.stop().await;
    let _ = std::fs::remove_file(descriptor);
}
