//! The voice assistant owner: three frozen channels, the helper-driven turn
//! loop, the per-session `iface_voice` MCP server and the renderer UI bridge.
//! The fourth channel, `iface:voiceControl`, belonged to the Control assistant
//! retired on 2026-10-14 (docs/archive/control-assistant-20261014): it stays
//! served as wire protocol and answers `voice-control-unavailable`.

mod helper;
pub(crate) mod mcp;
mod session;
pub(crate) mod tools;
mod ui_bridge;

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use engine_claude::{McpServer, VOICE_MCP_SERVER_NAME, VOICE_READ_ONLY_TOOLS, VoiceMcpProvider};
use interface_common::sync::lock;
use interface_contracts::{
    engine::{EngineEvent, EngineId, LaunchOptions, SessionKind, SessionSummary, session_key},
    ipc::CHANNELS,
    voice::{
        VoiceEvent, VoiceSetEnabledRequest, VoiceSetEnabledResponse, VoiceState,
        VoiceUiResultRequest,
    },
};
use interface_desktop_services::DesktopService;
use interface_runtime::SessionRuntime;
use serde_json::{Value, json};
use tokio::sync::mpsc;

pub use mcp::DeferredVoiceMcp;

use crate::{
    dispatcher::{
        DispatchError, HostDispatcher, OPERATION_NOT_MIGRATED_CODE, RuntimeCommandHandler,
        SessionEventObserver,
    },
    native::{decode_empty, decode_payload},
    protocol::PushFrame,
    terminal::TerminalHostDispatcher,
};

pub const VOICE_OPERATIONS: [&str; 4] = [
    CHANNELS.voice_set_enabled,
    CHANNELS.voice_interrupt,
    CHANNELS.voice_ui_result,
    CHANNELS.voice_control,
];

pub(crate) const VOICE_SESSION_TITLE: &str = "Voice assistant";
/// The retired Control assistant's coordinators were Voice-kind sessions with
/// this title. Voice never resumes one as its own session: it was launched
/// with the Control tools and prompt, not the voice assistant's.
const RETIRED_CONTROL_TITLE: &str = "Control assistant";
const RETIRED_CONTROL_MESSAGE: &str = "The Control assistant is no longer available.";
const VOICE_MODEL: &str = "sonnet";
const VOICE_EFFORT: &str = "low";
const INPUT_CAPACITY: usize = 64;
const ENGINE_EVENT_CAPACITY: usize = 1024;

/// The last pushed voice event, minus partials and levels, so tools and the
/// enable response can report the current state without asking the driver.
#[derive(Default)]
pub(crate) struct VoiceShared {
    last: Mutex<Option<VoiceEvent>>,
}

impl VoiceShared {
    pub(crate) fn snapshot(&self) -> VoiceEvent {
        lock(&self.last).clone().unwrap_or_else(off_event)
    }

    fn set(&self, event: &VoiceEvent) {
        let mut stored = event.clone();
        stored.partial = None;
        stored.level = None;
        *lock(&self.last) = Some(stored);
    }
}

fn off_event() -> VoiceEvent {
    VoiceEvent {
        state: VoiceState::Off,
        session_key: None,
        cwd: None,
        partial: None,
        level: None,
        message: None,
        microphone: None,
        asset: None,
    }
}

pub(crate) async fn push_voice_event(
    pushes: &mpsc::Sender<PushFrame>,
    shared: &VoiceShared,
    event: VoiceEvent,
) {
    shared.set(&event);
    push_voice_frame(pushes, &event).await;
}

/// Pushes without recording the event as the current state: for one-shot
/// notifications, which must not resurface in the next enable response.
pub(crate) async fn push_voice_frame(pushes: &mpsc::Sender<PushFrame>, event: &VoiceEvent) {
    let frame = serde_json::to_value(event)
        .map_err(|error| error.to_string())
        .and_then(|payload| PushFrame::new(CHANNELS.voice, payload).map_err(|error| error.message));
    match frame {
        Ok(frame) => {
            if pushes.send(frame).await.is_err() {
                tracing::warn!("voice push receiver closed");
            }
        }
        Err(error) => tracing::error!(%error, "could not encode a voice event"),
    }
}

type WatchedSession = Option<(String, mpsc::Sender<EngineEvent>)>;

/// Taps the runtime event pump for the one session the voice service owns.
/// Installed on the runtime dispatcher before the service exists.
#[derive(Clone, Default)]
pub struct VoiceEventTap {
    watched: Arc<Mutex<WatchedSession>>,
}

impl VoiceEventTap {
    fn watch(&self, session_key: String, sender: mpsc::Sender<EngineEvent>) {
        *lock(&self.watched) = Some((session_key, sender));
    }

    fn clear(&self) {
        *lock(&self.watched) = None;
    }
}

impl SessionEventObserver for VoiceEventTap {
    fn observe_session_event(&self, session_key: &str, event: &EngineEvent) {
        let watched = lock(&self.watched);
        let Some((key, sender)) = watched.as_ref() else {
            return;
        };
        if key == session_key && sender.try_send(event.clone()).is_err() {
            tracing::warn!("voice session event queue is full or closed; dropped one event");
        }
    }
}

pub struct VoiceDeps {
    pub runtime: Arc<SessionRuntime>,
    /// The runtime-bridge owner: `iface:createSession`, `iface:send`, ...
    pub sessions: Arc<dyn HostDispatcher>,
    pub terminal: Arc<TerminalHostDispatcher>,
    pub desktop: Arc<DesktopService>,
    pub pushes: mpsc::Sender<PushFrame>,
    pub data_root: PathBuf,
    pub events: VoiceEventTap,
}

struct ActiveVoice {
    inputs: mpsc::Sender<session::DriverInput>,
    task: tokio::task::JoinHandle<()>,
}

pub struct VoiceService {
    runtime: Arc<SessionRuntime>,
    sessions: Arc<dyn HostDispatcher>,
    pushes: mpsc::Sender<PushFrame>,
    tap: VoiceEventTap,
    ui: Arc<ui_bridge::UiBridge>,
    mcp: Arc<mcp::VoiceMcpServers>,
    shared: Arc<VoiceShared>,
    active: tokio::sync::Mutex<Option<ActiveVoice>>,
}

impl VoiceService {
    /// Must be called inside a Tokio runtime.
    #[must_use]
    pub fn new(deps: VoiceDeps) -> Arc<Self> {
        let shared = Arc::new(VoiceShared::default());
        let ui = Arc::new(ui_bridge::UiBridge::new(deps.pushes.clone()));
        let tools = Arc::new(tools::VoiceTools {
            runtime: Arc::clone(&deps.runtime),
            sessions: Arc::clone(&deps.sessions),
            terminal: deps.terminal,
            desktop: deps.desktop,
            pushes: deps.pushes.clone(),
            ui: Arc::clone(&ui),
            shared: Arc::clone(&shared),
        });
        let mcp = Arc::new(mcp::VoiceMcpServers::new(&deps.data_root, tools));
        Arc::new(Self {
            runtime: deps.runtime,
            sessions: deps.sessions,
            pushes: deps.pushes,
            tap: deps.events,
            ui,
            mcp,
            shared,
            active: tokio::sync::Mutex::new(None),
        })
    }

    pub async fn set_enabled(&self, request: VoiceSetEnabledRequest) -> VoiceSetEnabledResponse {
        let mut active = self.active.lock().await;
        if !request.enabled {
            self.stop(&mut active).await;
            return self.response();
        }
        if active.is_some() {
            // A failed run stays up so its message stays visible; enabling
            // again is the renderer's only way out, so it restarts from scratch.
            if self.shared.snapshot().state != VoiceState::Error {
                return self.response();
            }
            self.stop(&mut active).await;
        }
        let (helper, helper_events) = match helper::spawn(&helper::helper_path()) {
            Ok(spawned) => spawned,
            Err(error) => {
                let mut event = off_event();
                event.state = VoiceState::Error;
                event.cwd = Some(request.cwd);
                event.message = Some(format!("The voice helper could not start: {error}"));
                push_voice_event(&self.pushes, &self.shared, event).await;
                return self.response();
            }
        };
        let machine = session::Machine::new(request.cwd.clone());
        push_voice_event(&self.pushes, &self.shared, machine.snapshot()).await;
        let (inputs, input_receiver) = mpsc::channel(INPUT_CAPACITY);
        let (engine_events, engine_receiver) = mpsc::channel(ENGINE_EVENT_CAPACITY);
        let task = tokio::spawn(session::run(
            machine,
            helper,
            helper_events,
            input_receiver,
            engine_receiver,
            session::DriverIo {
                sessions: Arc::clone(&self.sessions),
                pushes: self.pushes.clone(),
                shared: Arc::clone(&self.shared),
                inputs: inputs.clone(),
            },
        ));
        *active = Some(ActiveVoice {
            inputs: inputs.clone(),
            task,
        });
        // ponytail: the enable lock is held across the session launch, so a
        // disable arriving mid-launch waits for the provider to come up.
        let session = self.ensure_session(&request.cwd).await;
        match &session {
            Ok(key) => {
                self.tap.watch(key.clone(), engine_events);
                let _ = inputs
                    .send(session::DriverInput::SessionReady(key.clone()))
                    .await;
            }
            Err(message) => {
                let _ = inputs
                    .send(session::DriverInput::SessionFailed(message.clone()))
                    .await;
            }
        }
        // Let the driver push the state change before the response reports it.
        tokio::task::yield_now().await;
        let mut response = self.response();
        match session {
            Ok(key) => response.session_key = Some(key),
            Err(message) => {
                response.state = VoiceState::Error;
                response.message = Some(message);
            }
        }
        response
    }

    async fn stop(&self, active: &mut Option<ActiveVoice>) {
        if let Some(active) = active.take() {
            let _ = active.inputs.send(session::DriverInput::Disable).await;
            let _ = active.task.await;
        }
        self.tap.clear();
    }

    pub async fn interrupt(&self) {
        if let Some(active) = self.active.lock().await.as_ref() {
            let _ = active.inputs.send(session::DriverInput::Interrupt).await;
        }
    }

    pub fn ui_result(&self, result: VoiceUiResultRequest) {
        if !self.ui.resolve(result) {
            tracing::debug!("voice UI result for an unknown or expired call");
        }
    }

    pub async fn shutdown(&self) {
        self.set_enabled(VoiceSetEnabledRequest {
            enabled: false,
            cwd: String::new(),
        })
        .await;
        self.mcp.stop_all();
    }

    fn response(&self) -> VoiceSetEnabledResponse {
        let snapshot = self.shared.snapshot();
        VoiceSetEnabledResponse {
            state: snapshot.state,
            session_key: snapshot.session_key,
            message: snapshot.message,
        }
    }

    /// Reuses the most recent voice session of this project (resuming it
    /// through the frozen `iface:resumeSession` path when cold), else creates
    /// one through `iface:createSession` exactly as the renderer would.
    async fn ensure_session(&self, cwd: &str) -> Result<String, String> {
        let summaries = RuntimeCommandHandler::session_summaries(self.runtime.as_ref())
            .map_err(|error| error.message)?;
        if let Some(existing) = pick_voice_session(&summaries, cwd) {
            let key = session_key(&existing.engine, &existing.session_id);
            if self.runtime.session_is_live(&key).unwrap_or(false) {
                return Ok(key);
            }
            self.sessions
                .dispatch(CHANNELS.resume_session, json!({"sessionKey": key}))
                .await
                .map_err(|error| error.message)?;
            return Ok(key);
        }
        let engine = EngineId::new("claude").map_err(|error| error.to_string())?;
        let options = LaunchOptions {
            cwd: cwd.to_owned(),
            kind: Some(SessionKind::Voice),
            model: Some(VOICE_MODEL.to_owned()),
            effort: Some(VOICE_EFFORT.to_owned()),
            name: Some(VOICE_SESSION_TITLE.to_owned()),
            allowed_tools: Some(
                VOICE_READ_ONLY_TOOLS
                    .iter()
                    .map(|tool| format!("mcp__{VOICE_MCP_SERVER_NAME}__{tool}"))
                    .collect(),
            ),
            ..LaunchOptions::default()
        };
        let created = self
            .sessions
            .dispatch(
                CHANNELS.create_session,
                json!({"engine": engine, "options": options}),
            )
            .await
            .map_err(|error| error.message)?;
        created
            .get("key")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| "the voice session started without a key".to_owned())
    }
}

fn pick_voice_session<'a>(
    summaries: &'a [SessionSummary],
    cwd: &str,
) -> Option<&'a SessionSummary> {
    summaries
        .iter()
        .filter(|summary| {
            summary.engine.as_str() == "claude"
                && !summary.archived
                && summary.kind == Some(SessionKind::Voice)
                && summary.title != RETIRED_CONTROL_TITLE
                && (summary.project_root.as_deref() == Some(cwd) || summary.cwd == cwd)
        })
        .max_by(|left, right| left.last_activity.cmp(&right.last_activity))
}

impl VoiceMcpProvider for VoiceService {
    fn descriptor_for(&self, session_key: &str, _cwd: &Path) -> Option<McpServer> {
        self.mcp.descriptor_for(session_key)
    }
}

#[async_trait]
impl HostDispatcher for VoiceService {
    async fn dispatch(&self, operation: &str, payload: Value) -> Result<Value, DispatchError> {
        if operation == CHANNELS.voice_control {
            // Retired: a well-formed request is refused before any effect.
            decode_payload::<interface_contracts::voice_control::Request>(operation, payload)?;
            return Err(DispatchError::new(
                "voice-control-unavailable",
                RETIRED_CONTROL_MESSAGE,
                false,
                true,
            ));
        }
        if operation == CHANNELS.voice_set_enabled {
            let request = decode_payload::<VoiceSetEnabledRequest>(operation, payload)?;
            return serde_json::to_value(self.set_enabled(request).await).map_err(|error| {
                DispatchError::new("voice-encode-failed", error.to_string(), false, true)
            });
        }
        if operation == CHANNELS.voice_interrupt {
            decode_empty(operation, &payload)?;
            self.interrupt().await;
            return Ok(json!({}));
        }
        if operation == CHANNELS.voice_ui_result {
            let result = decode_payload::<VoiceUiResultRequest>(operation, payload)?;
            self.ui_result(result);
            return Ok(json!({}));
        }
        Err(DispatchError::new(
            OPERATION_NOT_MIGRATED_CODE,
            "This Interface operation has not migrated to the Rust host.",
            false,
            true,
        ))
    }
}

#[cfg(test)]
mod tests {
    use interface_contracts::engine::SessionStatus;

    use super::*;

    fn summary(kind: Option<SessionKind>, title: &str, root: &str, at: &str) -> SessionSummary {
        SessionSummary {
            engine: EngineId::new("claude").unwrap(),
            session_id: format!("{title}-{at}"),
            title: title.into(),
            cwd: root.into(),
            status: SessionStatus::Idle,
            cloud: false,
            model: None,
            effort: None,
            last_activity: at.into(),
            first_seen: at.into(),
            archived: false,
            size: None,
            title_from_summary: None,
            origin: None,
            project_root: Some(root.into()),
            worktree_home: None,
            worktree_path: None,
            setting_sources: None,
            disallowed_tools: None,
            kind,
            permission_mode: None,
        }
    }

    #[test]
    fn reuse_prefers_the_newest_voice_session_of_the_same_project() {
        let summaries = vec![
            summary(
                Some(SessionKind::Voice),
                "old",
                "/p",
                "2026-01-01T00:00:00.000Z",
            ),
            summary(
                Some(SessionKind::Voice),
                "new",
                "/p",
                "2026-02-01T00:00:00.000Z",
            ),
            summary(
                Some(SessionKind::Voice),
                "other",
                "/q",
                "2026-03-01T00:00:00.000Z",
            ),
            summary(None, VOICE_SESSION_TITLE, "/p", "2026-04-01T00:00:00.000Z"),
            summary(
                Some(SessionKind::Code),
                VOICE_SESSION_TITLE,
                "/p",
                "2026-05-01T00:00:00.000Z",
            ),
            summary(
                Some(SessionKind::Voice),
                RETIRED_CONTROL_TITLE,
                "/p",
                "2026-06-01T00:00:00.000Z",
            ),
        ];
        // The newer rows that merely carry the title are Code sessions and
        // would resume without the voice prompt or tools; the newest is a
        // retired Control coordinator.
        assert_eq!(
            pick_voice_session(&summaries, "/p").map(|s| s.session_id.as_str()),
            Some("new-2026-02-01T00:00:00.000Z")
        );
        assert!(pick_voice_session(&summaries, "/none").is_none());
        assert_eq!(VOICE_OPERATIONS.len(), 4);
    }

    #[tokio::test]
    async fn a_notification_push_is_not_recorded_as_the_current_state() {
        let shared = VoiceShared::default();
        let (pushes, mut receiver) = mpsc::channel(1);
        let mut event = off_event();
        event.message = Some("Build: done".into());
        push_voice_frame(&pushes, &event).await;
        assert_eq!(
            receiver.try_recv().unwrap().payload["message"],
            "Build: done"
        );
        assert!(shared.snapshot().message.is_none());
    }

    #[test]
    fn the_tap_forwards_only_the_watched_session() {
        let tap = VoiceEventTap::default();
        let (sender, mut receiver) = mpsc::channel(4);
        let event = EngineEvent {
            at: None,
            event: interface_contracts::engine::EngineEventKind::SessionClosed,
        };
        tap.observe_session_event("claude#a", &event);
        tap.watch("claude#a".into(), sender);
        tap.observe_session_event("claude#b", &event);
        tap.observe_session_event("claude#a", &event);
        assert!(receiver.try_recv().is_ok());
        assert!(receiver.try_recv().is_err());
        tap.clear();
        tap.observe_session_event("claude#a", &event);
        assert!(receiver.try_recv().is_err());
    }
}
