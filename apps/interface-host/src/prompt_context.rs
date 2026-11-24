use std::{path::PathBuf, sync::Arc};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use interface_contracts::{
    ipc::{CHANNELS, PROMPT_CONTEXT_VALIDATION_LIMITS, PromptImageRejectReason},
    prompt_images::PROMPT_IMAGE_LIMITS,
};
use interface_desktop_services::DesktopService;
use interface_prompt_context::{
    PastedImageInput, PromptContextRuntimeView, PromptContextService, PromptContextServiceConfig,
    PromptContextServiceError,
};
use interface_runtime::SessionRuntime;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    dispatcher::{DispatchError, HostDispatcher},
    native::NativeRuntimeView,
};

pub use interface_prompt_context::PROMPT_CONTEXT_OPERATIONS;

const PASTED_IMAGE_WIRE_ENCODING: &str = "base64-v1";
const MAX_PASTED_IMAGE_BASE64_CHARS: usize = PROMPT_IMAGE_LIMITS.max_source_bytes.div_ceil(3) * 4;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PastedImageWirePayload {
    // Dispatch already admitted `base64-v1`; keep the key so
    // `deny_unknown_fields` still requires the frozen wire shape.
    #[serde(rename = "encoding")]
    _encoding: String,
    images: Vec<PastedImageWireItem>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum PastedImageWireItem {
    Base64 { data: String },
    InvalidRequest,
    SizeLimit,
}

/// Exact prompt-context authority projection from the current desktop/runtime
/// graph. It recomputes roots on every call, matching the legacy bridge:
/// current picker grants, visible session roots, and archived roots whose
/// provider effect is still live.
pub struct HostPromptContextRuntimeView {
    runtime: Arc<SessionRuntime>,
    desktop: Arc<DesktopService>,
}

impl HostPromptContextRuntimeView {
    #[must_use]
    pub const fn new(runtime: Arc<SessionRuntime>, desktop: Arc<DesktopService>) -> Self {
        Self { runtime, desktop }
    }
}

impl PromptContextRuntimeView for HostPromptContextRuntimeView {
    fn prompt_retention_ready(&self) -> bool {
        self.desktop.prompt_retention_ready()
    }
    fn retained_prompt_paths(&self) -> Vec<PathBuf> {
        self.desktop.retained_prompt_paths()
    }
    fn session_root(&self, session_key: &str) -> Option<PathBuf> {
        crate::registry_access::read(&self.runtime, |registry| {
            registry
                .get(session_key)
                .filter(|summary| !summary.cwd.is_empty())
                .map(|summary| PathBuf::from(&summary.cwd))
        })
        .ok()
        .flatten()
    }

    fn authorized_roots(&self) -> Vec<PathBuf> {
        let mut roots = self.desktop.workspace_roots();
        if let Ok(session_roots) = NativeRuntimeView::approved_session_roots(self.runtime.as_ref())
        {
            roots.extend(session_roots);
        }
        roots
    }
}

pub struct PromptContextHostDispatcher {
    service: Arc<PromptContextService>,
}

impl PromptContextHostDispatcher {
    #[must_use]
    pub const fn new(service: Arc<PromptContextService>) -> Self {
        Self { service }
    }

    #[must_use]
    pub fn service(&self) -> Arc<PromptContextService> {
        Arc::clone(&self.service)
    }

    #[must_use]
    pub fn system(
        runtime: Arc<SessionRuntime>,
        desktop: Arc<DesktopService>,
        pasted_image_directory: Option<PathBuf>,
    ) -> Self {
        let view: Arc<dyn PromptContextRuntimeView> =
            Arc::new(HostPromptContextRuntimeView::new(runtime, desktop));
        Self::new(Arc::new(PromptContextService::new(
            PromptContextServiceConfig::system(view, pasted_image_directory),
        )))
    }
}

#[async_trait]
impl HostDispatcher for PromptContextHostDispatcher {
    /// The service does blocking work (git review for a diff item, image
    /// decode and resize, filesystem reads), so every call runs on the
    /// blocking pool and never pins an async worker that terminal output and
    /// other requests share.
    async fn dispatch(&self, operation: &str, payload: Value) -> Result<Value, DispatchError> {
        let service = Arc::clone(&self.service);
        // Only a paste writes files; its outcome is unknown if the worker dies.
        let replay_safe = operation != CHANNELS.save_pasted_images;
        let operation = operation.to_owned();
        tokio::task::spawn_blocking(move || {
            if operation == CHANNELS.save_pasted_images
                && payload.get("encoding").and_then(Value::as_str)
                    == Some(PASTED_IMAGE_WIRE_ENCODING)
            {
                let inputs = decode_pasted_image_wire(payload);
                return serde_json::to_value(service.save_pasted_image_inputs(inputs.as_deref()))
                    .map_err(|_| {
                        prompt_context_error(&PromptContextServiceError::ResponseEncoding)
                    });
            }
            service
                .dispatch(&operation, &payload)
                .map_err(|error| prompt_context_error(&error))
        })
        .await
        .map_err(|error| {
            DispatchError::new(
                "prompt-context-worker-failed",
                format!("The prompt-context worker failed: {error}"),
                false,
                replay_safe,
            )
        })?
    }
}

fn decode_pasted_image_wire(payload: Value) -> Option<Vec<PastedImageInput>> {
    let payload = serde_json::from_value::<PastedImageWirePayload>(payload).ok()?;
    if payload.images.len() > PROMPT_CONTEXT_VALIDATION_LIMITS.max_items {
        return Some(
            (0..=PROMPT_CONTEXT_VALIDATION_LIMITS.max_items)
                .map(|_| PastedImageInput::Rejected(PromptImageRejectReason::InvalidRequest))
                .collect(),
        );
    }
    Some(
        payload
            .images
            .into_iter()
            .map(|image| match image {
                PastedImageWireItem::InvalidRequest => {
                    PastedImageInput::Rejected(PromptImageRejectReason::InvalidRequest)
                }
                PastedImageWireItem::SizeLimit => {
                    PastedImageInput::Rejected(PromptImageRejectReason::SizeLimit)
                }
                PastedImageWireItem::Base64 { data }
                    if data.len() > MAX_PASTED_IMAGE_BASE64_CHARS =>
                {
                    PastedImageInput::Rejected(PromptImageRejectReason::SizeLimit)
                }
                PastedImageWireItem::Base64 { data } => BASE64.decode(data.as_bytes()).map_or_else(
                    |_| PastedImageInput::Rejected(PromptImageRejectReason::InvalidRequest),
                    PastedImageInput::Bytes,
                ),
            })
            .collect(),
    )
}

fn prompt_context_error(error: &PromptContextServiceError) -> DispatchError {
    match error {
        PromptContextServiceError::OperationNotOwned(_) => DispatchError::new(
            "operation-not-owned",
            "That operation is not owned by the Rust prompt-context service.",
            false,
            true,
        ),
        PromptContextServiceError::ResponseEncoding => DispatchError::new(
            "response-encoding",
            "The Rust prompt-context result could not be encoded.",
            false,
            false,
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::Path};

    use interface_contracts::ipc::CHANNELS;
    use interface_prompt_context::PromptContextRuntimeView;
    use serde_json::json;

    use super::*;

    struct FakeRuntimeView {
        sessions: BTreeMap<String, PathBuf>,
        roots: Vec<PathBuf>,
    }

    impl PromptContextRuntimeView for FakeRuntimeView {
        fn session_root(&self, session_key: &str) -> Option<PathBuf> {
            self.sessions.get(session_key).cloned()
        }

        fn authorized_roots(&self) -> Vec<PathBuf> {
            self.roots.clone()
        }
    }

    fn dispatcher(root: &Path) -> PromptContextHostDispatcher {
        let view: Arc<dyn PromptContextRuntimeView> = Arc::new(FakeRuntimeView {
            sessions: BTreeMap::from([("codex#one".to_owned(), root.to_path_buf())]),
            roots: vec![root.to_path_buf()],
        });
        PromptContextHostDispatcher::new(Arc::new(PromptContextService::new(
            PromptContextServiceConfig::system(view, Some(root.join("pasted-images"))),
        )))
    }

    #[tokio::test]
    async fn routes_all_four_frozen_operations_through_the_rust_service() {
        let root = tempfile::tempdir().expect("root");
        let file = root.path().join("notes.txt");
        std::fs::write(&file, b"hello").expect("file");
        let file = std::fs::canonicalize(file).expect("canonical file");
        let file_text = file.to_string_lossy().into_owned();
        let dispatcher = dispatcher(root.path());

        let granted = dispatcher
            .dispatch(
                CHANNELS.grant_prompt_context_paths,
                json!({"paths":[file_text]}),
            )
            .await
            .expect("grant");
        assert_eq!(granted["granted"].as_array().map(Vec::len), Some(1));

        let pasted = dispatcher
            .dispatch(CHANNELS.save_pasted_images, json!({"images":[]}))
            .await
            .expect("paste");
        assert_eq!(pasted, json!({"saved":[],"rejected":[]}));

        let thumbnail = dispatcher
            .dispatch(
                CHANNELS.read_prompt_context_thumbnail,
                json!({"path":file_text}),
            )
            .await
            .expect("thumbnail");
        assert!(thumbnail.is_null());

        let validation = dispatcher
            .dispatch(
                CHANNELS.validate_prompt_context,
                json!({"target":"codex#one","items":[]}),
            )
            .await
            .expect("validation");
        assert_eq!(validation, json!({"ok":true,"items":[],"invalid":[]}));
    }

    #[tokio::test]
    async fn malformed_payloads_keep_structured_legacy_refusals() {
        let root = tempfile::tempdir().expect("root");
        let dispatcher = dispatcher(root.path());
        let granted = dispatcher
            .dispatch(CHANNELS.grant_prompt_context_paths, Value::Null)
            .await
            .expect("structured grant refusal");
        assert_eq!(granted["rejected"][0]["reason"], "invalid-request");
        let validation = dispatcher
            .dispatch(CHANNELS.validate_prompt_context, Value::Null)
            .await
            .expect("structured validation refusal");
        assert_eq!(validation["invalid"][0]["reason"], "invalid-request");
    }

    #[tokio::test]
    async fn binary_wire_preserves_per_image_refusals_without_json_byte_arrays() {
        let root = tempfile::tempdir().expect("root");
        let dispatcher = dispatcher(root.path());
        let result = dispatcher
            .dispatch(
                CHANNELS.save_pasted_images,
                json!({
                    "encoding": "base64-v1",
                    "images": [
                        {"kind": "base64", "data": ""},
                        {"kind": "base64", "data": "!"},
                        {"kind": "invalid-request"},
                        {"kind": "size-limit"}
                    ]
                }),
            )
            .await
            .expect("wire result");
        assert_eq!(
            result["rejected"]
                .as_array()
                .expect("rejections")
                .iter()
                .map(|rejection| rejection["reason"].as_str().expect("reason"))
                .collect::<Vec<_>>(),
            vec![
                "size-limit",
                "invalid-request",
                "invalid-request",
                "size-limit"
            ]
        );
        assert_eq!(result["saved"], json!([]));
    }

    #[tokio::test]
    async fn binary_wire_checks_item_count_before_decoding_items() {
        let root = tempfile::tempdir().expect("root");
        let dispatcher = dispatcher(root.path());
        let images = (0..=PROMPT_CONTEXT_VALIDATION_LIMITS.max_items)
            .map(|_| json!({"kind": "base64", "data": "!"}))
            .collect::<Vec<_>>();
        let result = dispatcher
            .dispatch(
                CHANNELS.save_pasted_images,
                json!({"encoding": "base64-v1", "images": images}),
            )
            .await
            .expect("wire result");
        assert_eq!(
            result["rejected"],
            json!([{
                "reason": "item-limit",
                "message": "At most 8 images can be attached at once."
            }])
        );
    }

    /// Waits (bounded) for another task on the same runtime to make progress.
    struct ProgressProbeView {
        root: PathBuf,
        progress: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
        observed: std::sync::atomic::AtomicBool,
    }

    impl PromptContextRuntimeView for ProgressProbeView {
        fn session_root(&self, _session_key: &str) -> Option<PathBuf> {
            None
        }

        fn authorized_roots(&self) -> Vec<PathBuf> {
            let progressed = self
                .progress
                .lock()
                .expect("progress receiver")
                .recv_timeout(std::time::Duration::from_secs(2))
                .is_ok();
            self.observed
                .fetch_or(progressed, std::sync::atomic::Ordering::SeqCst);
            vec![self.root.clone()]
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn service_work_runs_off_the_async_worker() {
        let root = tempfile::tempdir().expect("root");
        let file = root.path().join("notes.txt");
        std::fs::write(&file, b"hello").expect("file");
        let (progress, receiver) = std::sync::mpsc::channel();
        let view = Arc::new(ProgressProbeView {
            root: root.path().to_path_buf(),
            progress: std::sync::Mutex::new(receiver),
            observed: std::sync::atomic::AtomicBool::new(false),
        });
        let dispatcher = PromptContextHostDispatcher::new(Arc::new(PromptContextService::new(
            PromptContextServiceConfig::system(view.clone(), None),
        )));
        // On a single-threaded runtime this task can only run while dispatch
        // is parked on the blocking pool, not while it blocks the worker.
        let other_task = tokio::spawn(async move {
            let _ = progress.send(());
        });
        dispatcher
            .dispatch(
                CHANNELS.read_prompt_context_thumbnail,
                json!({"path":file.to_string_lossy()}),
            )
            .await
            .expect("thumbnail");
        other_task.await.expect("other task");
        assert!(
            view.observed.load(std::sync::atomic::Ordering::SeqCst),
            "prompt-context work blocked the async worker"
        );
    }

    #[tokio::test]
    async fn unknown_operation_has_no_fallback_owner() {
        let root = tempfile::tempdir().expect("root");
        let error = dispatcher(root.path())
            .dispatch("iface:notPromptContext", Value::Null)
            .await
            .expect_err("unknown operation");
        assert_eq!(error.code, "operation-not-owned");
        assert!(error.replay_safe);
    }
}
