//! Round-trips a UI tool call to the renderer: one `iface:voiceUiCall` push,
//! one `iface:voiceUiResult` request back, matched by `call_id`.

use std::{
    collections::HashMap,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use interface_contracts::{
    ipc::CHANNELS,
    voice::{VoiceUiCall, VoiceUiResultRequest},
};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use crate::protocol::PushFrame;

const UI_CALL_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) struct UiBridge {
    pushes: mpsc::Sender<PushFrame>,
    pending: Mutex<HashMap<String, oneshot::Sender<VoiceUiResultRequest>>>,
    next_id: AtomicU64,
}

impl UiBridge {
    pub(crate) fn new(pushes: mpsc::Sender<PushFrame>) -> Self {
        Self {
            pushes,
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        }
    }

    /// Pushes the call and waits for the renderer's answer.
    ///
    /// # Errors
    ///
    /// The renderer's own error text, or a host message when the push could
    /// not be delivered or no answer arrived within 10 seconds.
    pub(crate) async fn call(&self, tool: &str, input: Value) -> Result<String, String> {
        self.call_with_timeout(tool, input, UI_CALL_TIMEOUT).await
    }

    async fn call_with_timeout(
        &self,
        tool: &str,
        input: Value,
        wait: Duration,
    ) -> Result<String, String> {
        let call_id = format!("c{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let (sender, receiver) = oneshot::channel();
        self.pending().insert(call_id.clone(), sender);
        let frame = serde_json::to_value(VoiceUiCall {
            call_id: call_id.clone(),
            tool: tool.to_owned(),
            input,
        })
        .map_err(|error| error.to_string())
        .and_then(|payload| {
            PushFrame::new(CHANNELS.voice_ui_call, payload).map_err(|error| error.message)
        });
        let frame = match frame {
            Ok(frame) => frame,
            Err(message) => {
                self.pending().remove(&call_id);
                return Err(message);
            }
        };
        if self.pushes.send(frame).await.is_err() {
            self.pending().remove(&call_id);
            return Err("the Interface window is not available".to_owned());
        }
        match tokio::time::timeout(wait, receiver).await {
            Ok(Ok(result)) if result.ok => Ok(result.output),
            Ok(Ok(result)) => Err(result.output),
            Ok(Err(_)) => Err(format!("{tool} was dropped before the renderer answered")),
            Err(_) => {
                self.pending().remove(&call_id);
                Err(format!("{tool} timed out waiting for the Interface window"))
            }
        }
    }

    /// Delivers a renderer answer; returns `false` for an unknown or expired call.
    pub(crate) fn resolve(&self, result: VoiceUiResultRequest) -> bool {
        let Some(sender) = self.pending().remove(&result.call_id) else {
            return false;
        };
        sender.send(result).is_ok()
    }

    fn pending(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<String, oneshot::Sender<VoiceUiResultRequest>>> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn call_pushes_a_ui_call_and_resolves_on_the_matching_result() {
        let (pushes, mut frames) = mpsc::channel(4);
        let bridge = std::sync::Arc::new(UiBridge::new(pushes));
        let caller = std::sync::Arc::clone(&bridge);
        let call = tokio::spawn(async move { caller.call("ui_snapshot", json!({})).await });
        let frame = frames.recv().await.expect("ui call push");
        assert_eq!(frame.operation, "iface:voiceUiCall");
        assert_eq!(frame.payload["tool"], "ui_snapshot");
        let call_id = frame.payload["call_id"].as_str().unwrap().to_owned();
        assert!(!bridge.resolve(VoiceUiResultRequest {
            call_id: "nope".into(),
            ok: true,
            output: String::new(),
        }));
        assert!(bridge.resolve(VoiceUiResultRequest {
            call_id,
            ok: true,
            output: "[v1] button \"Sessions\"".into(),
        }));
        assert_eq!(
            call.await.unwrap().as_deref(),
            Ok("[v1] button \"Sessions\"")
        );
    }

    #[tokio::test]
    async fn call_times_out_when_the_renderer_never_answers() {
        let (pushes, _frames) = mpsc::channel(4);
        let bridge = UiBridge::new(pushes);
        let result = bridge
            .call_with_timeout("ui_act", json!({"id": "v1"}), Duration::from_millis(20))
            .await;
        assert_eq!(
            result.unwrap_err(),
            "ui_act timed out waiting for the Interface window"
        );
        assert!(bridge.pending().is_empty());
    }
}
