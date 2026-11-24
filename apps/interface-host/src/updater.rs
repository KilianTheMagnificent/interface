use std::sync::Arc;

use async_trait::async_trait;
use interface_contracts::ipc::CHANNELS;
use interface_contracts::update_sessions::{
    UpdateSessionsCommand, UpdateSessionsPhase, UpdateSessionsView,
};
use interface_updater::{
    NativeUpdaterConfigurationInput, NativeUpdaterEvent, ReleaseChannel, RemoteFetcher,
    UpdaterClock, UpdaterService, UpdaterVerifier,
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::{sync::mpsc, task::JoinHandle};

use crate::{
    dispatcher::{DispatchError, HostDispatcher, INVALID_PAYLOAD_CODE},
    protocol::PushFrame,
};

pub const UPDATER_OPERATIONS: [&str; 4] = [
    CHANNELS.get_update_state,
    CHANNELS.check_for_updates,
    CHANNELS.restart_to_update,
    CHANNELS.update_sessions,
];
pub const NATIVE_UPDATER_CONFIGURE: &str = "iface:nativeUpdaterConfigure";
pub const NATIVE_UPDATER_EVENT: &str = "iface:nativeUpdaterEvent";
pub const INTERNAL_UPDATER_OPERATIONS: [&str; 2] = [NATIVE_UPDATER_CONFIGURE, NATIVE_UPDATER_EVENT];

/// Builds the host's update service.
///
/// A build with a public release channel takes its feed from the binary and
/// nowhere else: `INTERFACE_UPDATE_URL`, which lets a build without a channel
/// read an unsigned HTTPS feed, is then ignored, so nothing in a process's
/// environment can route a published app around the signed manifest.
#[must_use]
pub fn updater_service(
    data_root: std::path::PathBuf,
    channel: Option<ReleaseChannel>,
    environment_feed: Option<String>,
    fetcher: Arc<dyn RemoteFetcher>,
    verifier: Arc<dyn UpdaterVerifier>,
    clock: Arc<dyn UpdaterClock>,
) -> Arc<UpdaterService> {
    let unsigned_feed = if channel.is_some() {
        None
    } else {
        environment_feed
    };
    Arc::new(
        UpdaterService::new(data_root, unsigned_feed, verifier, clock)
            .with_release_channel(channel, fetcher),
    )
}

pub struct UpdaterHostDispatcher {
    service: Arc<UpdaterService>,
}

impl UpdaterHostDispatcher {
    #[must_use]
    pub const fn new(service: Arc<UpdaterService>) -> Self {
        Self { service }
    }

    pub async fn close(&self) {
        self.service.close().await;
    }
}

#[async_trait]
impl HostDispatcher for UpdaterHostDispatcher {
    async fn dispatch(&self, operation: &str, payload: Value) -> Result<Value, DispatchError> {
        match operation {
            operation if operation == CHANNELS.get_update_state => {
                empty_payload(operation, &payload)?;
                encode(self.service.state().await)
            }
            operation if operation == CHANNELS.check_for_updates => {
                empty_payload(operation, &payload)?;
                encode(self.service.begin_check().await)
            }
            operation if operation == CHANNELS.restart_to_update => {
                empty_payload(operation, &payload)?;
                encode(self.service.request_install().await)
            }
            operation if operation == CHANNELS.update_sessions => {
                // Retired 2026-10-21 with "Halt all and update". The channel
                // stays served: a read reports nothing paused, and nothing is
                // ever asked to stop or continue.
                match decode::<UpdateSessionsCommand>(operation, payload)? {
                    UpdateSessionsCommand::Read => encode(UpdateSessionsView {
                        version: 1,
                        phase: UpdateSessionsPhase::Idle,
                        sessions: Vec::new(),
                        detail: String::new(),
                    }),
                    _ => Err(DispatchError::new(
                        "update-checkpoint",
                        "Update checkpoints were removed. Restart to update installs directly.",
                        false,
                        true,
                    )),
                }
            }
            NATIVE_UPDATER_CONFIGURE => {
                let input = decode::<NativeUpdaterConfigurationInput>(operation, payload)?;
                encode(self.service.configure(input).await)
            }
            NATIVE_UPDATER_EVENT => {
                let event = decode::<NativeUpdaterEvent>(operation, payload)?;
                encode(self.service.native_event(event).await)
            }
            _ => Err(DispatchError::new(
                "operation-not-owned",
                "That operation is not owned by the Rust updater service.",
                false,
                true,
            )),
        }
    }
}

/// Forwards every Rust updater transition, including the intermediate
/// signature-verification state, through the frozen renderer push channel.
#[must_use]
pub fn spawn_update_state_forwarder(
    service: &Arc<UpdaterService>,
    pushes: mpsc::Sender<PushFrame>,
) -> JoinHandle<()> {
    let mut states = service.subscribe();
    tokio::spawn(async move {
        while let Some(state) = states.recv().await {
            let Ok(payload) = serde_json::to_value(state) else {
                break;
            };
            let Ok(frame) = PushFrame::new(CHANNELS.update_state, payload) else {
                break;
            };
            if pushes.send(frame).await.is_err() {
                break;
            }
        }
    })
}

fn empty_payload(operation: &str, payload: &Value) -> Result<(), DispatchError> {
    if payload.as_object().is_some_and(serde_json::Map::is_empty) {
        return Ok(());
    }
    Err(invalid_payload(operation))
}

fn decode<T: DeserializeOwned>(operation: &str, payload: Value) -> Result<T, DispatchError> {
    serde_json::from_value(payload).map_err(|_| invalid_payload(operation))
}

fn encode(value: impl serde::Serialize) -> Result<Value, DispatchError> {
    serde_json::to_value(value).map_err(|_| {
        DispatchError::new(
            "response-encoding",
            "The Rust updater result could not be encoded.",
            false,
            false,
        )
    })
}

fn invalid_payload(operation: &str) -> DispatchError {
    DispatchError::new(
        INVALID_PAYLOAD_CODE,
        format!("{operation} received an invalid payload."),
        false,
        true,
    )
}

#[cfg(test)]
mod tests {
    use std::{path::Path, sync::Mutex};

    use async_trait::async_trait;
    use interface_updater::{
        UpdaterClock, UpdaterVerifier, UreqFetcher, VerificationError, generate_signing_key,
    };
    use serde_json::json;

    use super::*;

    struct FixedClock;

    impl UpdaterClock for FixedClock {
        fn now_iso(&self) -> String {
            "2026-08-27T12:00:00.000Z".to_owned()
        }
    }

    struct AcceptingVerifier {
        artifacts: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl UpdaterVerifier for AcceptingVerifier {
        async fn running_requirement(
            &self,
            _packaged: bool,
            _app_bundle: Option<&Path>,
        ) -> Result<Option<String>, VerificationError> {
            Ok(Some("anchor apple generic".to_owned()))
        }

        async fn verify_artifact(
            &self,
            artifact: &Path,
            _requirement: &str,
        ) -> Result<(), VerificationError> {
            self.artifacts
                .lock()
                .expect("artifacts")
                .push(artifact.to_string_lossy().into_owned());
            Ok(())
        }
    }

    fn dispatcher() -> (
        tempfile::TempDir,
        Arc<UpdaterService>,
        UpdaterHostDispatcher,
    ) {
        let root = tempfile::tempdir().expect("root");
        let service = Arc::new(UpdaterService::new(
            root.path(),
            Some("https://updates.example.test/interface/".to_owned()),
            Arc::new(AcceptingVerifier {
                artifacts: Mutex::new(Vec::new()),
            }),
            Arc::new(FixedClock),
        ));
        let dispatcher = UpdaterHostDispatcher::new(service.clone());
        (root, service, dispatcher)
    }

    fn configured_feed(
        channel: Option<ReleaseChannel>,
        environment_feed: Option<&str>,
    ) -> impl std::future::Future<Output = Option<String>> {
        let root = tempfile::tempdir().expect("root");
        let service = updater_service(
            root.path().to_path_buf(),
            channel,
            environment_feed.map(str::to_owned),
            // Configuring never touches the network.
            Arc::new(UreqFetcher::new()),
            Arc::new(AcceptingVerifier {
                artifacts: Mutex::new(Vec::new()),
            }),
            Arc::new(FixedClock),
        );
        async move {
            let _root = root;
            service
                .configure(NativeUpdaterConfigurationInput {
                    is_packaged: true,
                    current_version: "0.2.0".to_owned(),
                    app_bundle_path: Some("/Applications/Interface.app".to_owned()),
                })
                .await
                .feed_url
        }
    }

    #[tokio::test]
    async fn a_build_with_a_public_channel_ignores_a_feed_from_the_environment() {
        let elsewhere = "https://updates.elsewhere.test/interface/";
        // Without a channel the environment's feed is what the installer reads.
        assert_eq!(
            configured_feed(None, Some(elsewhere)).await.as_deref(),
            Some(elsewhere)
        );
        let (_, key) = generate_signing_key().expect("key");
        let channel = ReleaseChannel::loopback(
            "com.example.interface",
            "stable",
            &["http://127.0.0.1:9/interface-update.json"],
            &[key],
        )
        .expect("channel");
        let feed = configured_feed(Some(channel), Some(elsewhere))
            .await
            .expect("feed");
        assert!(feed.starts_with("http://127.0.0.1:"), "{feed}");
    }

    #[tokio::test]
    async fn restart_to_update_installs_without_a_session_checkpoint() {
        let (_root, _service, dispatcher) = dispatcher();
        let configured = dispatcher
            .dispatch(
                NATIVE_UPDATER_CONFIGURE,
                json!({
                    "isPackaged": true,
                    "currentVersion": "0.2.0",
                    "appBundlePath": "/Applications/Interface.app"
                }),
            )
            .await
            .expect("configure");
        assert_eq!(configured["enabled"], true);

        let checking = dispatcher
            .dispatch(CHANNELS.check_for_updates, json!({}))
            .await
            .expect("check");
        assert_eq!(checking["phase"], "checking");
        let ready = dispatcher
            .dispatch(
                NATIVE_UPDATER_EVENT,
                json!({
                    "kind":"downloaded",
                    "version":"0.3.0",
                    "downloadedFile":"/private/tmp/Interface.zip"
                }),
            )
            .await
            .expect("downloaded");
        assert_eq!(ready["phase"], "ready");
        // The retired checkpoint channel reports nothing paused and pauses nothing.
        assert_eq!(
            dispatcher
                .dispatch(CHANNELS.update_sessions, json!({"command":"read"}))
                .await
                .expect("read"),
            json!({"version":1,"phase":"idle","sessions":[],"detail":""})
        );
        let refused = dispatcher
            .dispatch(CHANNELS.update_sessions, json!({"command":"prepare"}))
            .await
            .expect_err("nothing is asked to stop");
        assert!(refused.replay_safe);
        assert_eq!(
            dispatcher
                .dispatch(CHANNELS.restart_to_update, json!({}))
                .await
                .expect("restart"),
            Value::Bool(true)
        );
        let state = dispatcher
            .dispatch(CHANNELS.get_update_state, json!({}))
            .await
            .expect("state");
        assert_eq!(state["phase"], "installing");
    }

    #[tokio::test]
    async fn forwards_every_state_transition_over_the_existing_push_channel() {
        let (_root, service, dispatcher) = dispatcher();
        let (pushes, mut receiver) = mpsc::channel(16);
        let forwarder = spawn_update_state_forwarder(&service, pushes);
        dispatcher
            .dispatch(
                NATIVE_UPDATER_CONFIGURE,
                json!({
                    "isPackaged": true,
                    "currentVersion": "0.2.0",
                    "appBundlePath": "/Applications/Interface.app"
                }),
            )
            .await
            .expect("configure");
        let idle = receiver.recv().await.expect("idle push");
        assert_eq!(idle.operation, CHANNELS.update_state);
        assert_eq!(idle.payload["phase"], "idle");

        dispatcher
            .dispatch(CHANNELS.check_for_updates, json!({}))
            .await
            .expect("check");
        let checking = receiver.recv().await.expect("checking push");
        assert_eq!(checking.payload["phase"], "checking");

        dispatcher
            .dispatch(
                NATIVE_UPDATER_EVENT,
                json!({
                    "kind":"downloaded",
                    "version":"0.3.0",
                    "downloadedFile":"/private/tmp/Interface.zip"
                }),
            )
            .await
            .expect("downloaded");
        let verifying = receiver.recv().await.expect("verification push");
        assert_eq!(verifying.payload["phase"], "downloading");
        assert_eq!(verifying.payload["progress"], 100.0);
        let ready = receiver.recv().await.expect("ready push");
        assert_eq!(ready.payload["phase"], "ready");
        forwarder.abort();
    }

    #[tokio::test]
    async fn malformed_or_extra_native_fields_fail_before_state_mutation() {
        let (_root, _service, dispatcher) = dispatcher();
        let error = dispatcher
            .dispatch(
                NATIVE_UPDATER_EVENT,
                json!({"kind":"error", "message":"private native detail"}),
            )
            .await
            .expect_err("private error detail refused");
        assert_eq!(error.code, INVALID_PAYLOAD_CODE);
        let disabled = dispatcher
            .dispatch(
                NATIVE_UPDATER_EVENT,
                json!({"kind":"downloaded", "downloadedFile":"relative.zip"}),
            )
            .await
            .expect("well-shaped event");
        assert_eq!(disabled["phase"], "disabled");
        let error = dispatcher
            .dispatch(CHANNELS.get_update_state, Value::Null)
            .await
            .expect_err("payload refused");
        assert_eq!(error.code, INVALID_PAYLOAD_CODE);
    }

    #[test]
    fn operation_sets_remain_disjoint_and_frozen() {
        assert_eq!(UPDATER_OPERATIONS.len(), 4);
        assert_eq!(INTERNAL_UPDATER_OPERATIONS.len(), 2);
        assert!(
            UPDATER_OPERATIONS
                .iter()
                .all(|operation| { !INTERNAL_UPDATER_OPERATIONS.contains(operation) })
        );
    }
}
