//! The voice turn state machine (pure) and the task that drives it against
//! the helper, the session dispatcher and the renderer pushes.

use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};

use interface_contracts::{
    engine::{EngineEvent, EngineEventKind, SessionStatus},
    ipc::CHANNELS,
    voice::{VoiceAsset, VoiceEvent, VoiceState},
};
use interface_voice::{HelperToHost, HostToHelper, SentenceChunker, speakable};
use serde_json::json;
use tokio::sync::mpsc;

use super::{
    VoiceShared,
    helper::{self, Helper, HelperEvent},
    push_voice_event,
};
use crate::{dispatcher::HostDispatcher, protocol::PushFrame};

const MIN_CHUNK_CHARS: usize = 20;
const END_SILENCE_MS: u32 = 800;
const LOCALE: &str = "en-US";
const PUSH_THROTTLE: Duration = Duration::from_millis(100);
/// How long `Disable` lets an in-flight session effect settle before aborting it.
const EFFECTS_GRACE: Duration = Duration::from_secs(5);
pub(crate) const MICROPHONE_DENIED: &str = "Microphone access denied. Allow Interface in System Settings → Privacy & Security → Microphone.";
const NO_VOICE: &str =
    "No speech voice installed. Add one in System Settings → Accessibility → Spoken Content.";

pub(crate) enum Input {
    Helper(HelperToHost),
    Engine(EngineEvent),
    SessionReady(String),
    SessionFailed(String),
    TurnFailed(String),
    Interrupt,
    HelperRestarting,
    HelperFailed(String),
}

#[derive(Debug)]
pub(crate) enum Command {
    Helper(HostToHelper),
    SendTurn {
        session_key: String,
        text: String,
    },
    InterruptSession {
        session_key: String,
    },
    Push(VoiceEvent),
    /// Levels: the driver drops these above 10 Hz.
    PushLevel(VoiceEvent),
    /// Partials: throttled to 10 Hz on their own clock, so a burst of level
    /// frames can never starve the text.
    PushPartial(VoiceEvent),
}

/// Off → Starting → Listening → Thinking → Speaking, plus Error.
pub(crate) struct Machine {
    state: VoiceState,
    cwd: String,
    session_key: Option<String>,
    microphone: Option<String>,
    asset: Option<VoiceAsset>,
    helper_listening: bool,
    /// The live utterance text and mic level, carried by every push while
    /// listening so neither blanks the other.
    partial: Option<String>,
    level: Option<f64>,
    chunker: SentenceChunker,
    /// Utterance ids handed to the helper and not yet finished or cancelled.
    queue: VecDeque<String>,
    next_utterance: u64,
    turn_done: bool,
    /// A `Final` while busy interrupted the previous turn; its terminal event
    /// still arrives and must not end the turn that replaced it.
    interrupt_outstanding: bool,
    pub(crate) restarts: u8,
}

impl Machine {
    pub(crate) fn new(cwd: String) -> Self {
        Self {
            state: VoiceState::Starting,
            cwd,
            session_key: None,
            microphone: None,
            asset: None,
            helper_listening: false,
            partial: None,
            level: None,
            chunker: SentenceChunker::new(MIN_CHUNK_CHARS),
            queue: VecDeque::new(),
            next_utterance: 1,
            turn_done: true,
            interrupt_outstanding: false,
            restarts: 0,
        }
    }

    #[cfg(test)]
    pub(crate) fn state(&self) -> VoiceState {
        self.state
    }

    pub(crate) fn snapshot(&self) -> VoiceEvent {
        VoiceEvent {
            state: self.state,
            session_key: self.session_key.clone(),
            cwd: Some(self.cwd.clone()),
            partial: self.partial.clone(),
            level: self.level,
            message: None,
            microphone: self.microphone.clone(),
            asset: self.asset.clone(),
        }
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) fn handle(&mut self, input: Input) -> Vec<Command> {
        match input {
            Input::Helper(message) => self.handle_helper(message),
            Input::Engine(event) => self.handle_engine(event),
            Input::SessionReady(key) => {
                self.session_key = Some(key);
                self.try_listen()
            }
            Input::SessionFailed(message) | Input::HelperFailed(message) => self.fail(message),
            Input::TurnFailed(message) => {
                if self.busy() {
                    self.reset_turn();
                    vec![
                        self.set_state(VoiceState::Listening),
                        Command::Push(self.message(message)),
                    ]
                } else {
                    vec![Command::Push(self.message(message))]
                }
            }
            Input::Interrupt => {
                if !self.busy() {
                    return Vec::new();
                }
                let mut commands = vec![Command::Helper(HostToHelper::Cancel)];
                commands.extend(self.interrupt_session());
                commands.push(self.set_state(VoiceState::Listening));
                commands
            }
            Input::HelperRestarting => {
                self.helper_listening = false;
                self.reset_turn();
                vec![self.set_state(VoiceState::Starting)]
            }
        }
    }

    fn handle_helper(&mut self, message: HelperToHost) -> Vec<Command> {
        match message {
            HelperToHost::Ready { default_voice, .. } => {
                if default_voice.is_empty() {
                    return self.fail(NO_VOICE.to_owned());
                }
                vec![Command::Helper(HostToHelper::Start {
                    end_silence_ms: END_SILENCE_MS,
                    locale: LOCALE.to_owned(),
                })]
            }
            HelperToHost::Permission { microphone } => {
                let denied = microphone == "denied";
                self.microphone = Some(microphone);
                if denied {
                    self.fail(MICROPHONE_DENIED.to_owned())
                } else {
                    vec![Command::Push(self.snapshot())]
                }
            }
            HelperToHost::Asset { status, progress } => {
                let failed = status == "failed";
                self.asset = Some(VoiceAsset { status, progress });
                if failed {
                    self.fail("Speech assets failed to install.".to_owned())
                } else {
                    vec![Command::Push(self.snapshot())]
                }
            }
            HelperToHost::State { listening, .. } => {
                self.helper_listening = listening;
                self.try_listen()
            }
            HelperToHost::Partial { text } if self.state == VoiceState::Listening => {
                self.partial = Some(text);
                vec![Command::PushPartial(self.snapshot())]
            }
            HelperToHost::Level { rms } if self.state == VoiceState::Listening => {
                self.level = Some(rms);
                vec![Command::PushLevel(self.snapshot())]
            }
            HelperToHost::Final { text } => {
                self.partial = None;
                self.level = None;
                let text = text.trim().to_owned();
                let Some(session_key) = self.session_key.clone() else {
                    return Vec::new();
                };
                if text.is_empty() || matches!(self.state, VoiceState::Off | VoiceState::Error) {
                    return Vec::new();
                }
                let mut commands = Vec::new();
                if self.busy() {
                    commands.push(Command::Helper(HostToHelper::Cancel));
                    commands.extend(self.interrupt_session());
                    self.interrupt_outstanding = true;
                }
                self.turn_done = false;
                commands.push(Command::SendTurn { session_key, text });
                commands.push(self.set_state(VoiceState::Thinking));
                commands
            }
            HelperToHost::SpeechStarted { .. } if self.state == VoiceState::Thinking => {
                vec![self.set_state(VoiceState::Speaking)]
            }
            HelperToHost::SpeechFinished { id } | HelperToHost::SpeechCancelled { id } => {
                self.queue.retain(|queued| queued != &id);
                self.after_speech()
            }
            HelperToHost::Interrupted => {
                if !self.busy() {
                    return Vec::new();
                }
                let mut commands = self.interrupt_session();
                commands.push(self.set_state(VoiceState::Listening));
                commands
            }
            HelperToHost::Error { message, fatal } => {
                if fatal {
                    self.fail(message)
                } else {
                    vec![Command::Push(self.message(message))]
                }
            }
            HelperToHost::Partial { .. }
            | HelperToHost::Level { .. }
            | HelperToHost::SpeechStarted { .. }
            | HelperToHost::Unknown => Vec::new(),
        }
    }

    fn handle_engine(&mut self, event: EngineEvent) -> Vec<Command> {
        if !self.busy() {
            return Vec::new();
        }
        match event.event {
            EngineEventKind::MessageDelta {
                text,
                agent_key: None,
            } => {
                // The replacing turn is streaming, so the interrupt produced no
                // terminal event of its own (the CLI was already idle).
                self.interrupt_outstanding = false;
                let chunks = self.chunker.push(&text);
                chunks
                    .into_iter()
                    .filter_map(|chunk| self.speak(&chunk))
                    .collect()
            }
            EngineEventKind::StatusChanged {
                status: SessionStatus::Idle,
            } => {
                if self.take_stale_terminal() {
                    return Vec::new();
                }
                self.turn_done = true;
                let mut commands = Vec::new();
                if let Some(rest) = self.chunker.flush() {
                    commands.extend(self.speak(&rest));
                }
                commands.extend(self.after_speech());
                commands
            }
            EngineEventKind::Error { message, .. } => {
                if self.take_stale_terminal() {
                    return Vec::new();
                }
                self.turn_done = true;
                let mut commands = vec![Command::Push(self.message(message))];
                if let Some(rest) = self.chunker.flush() {
                    commands.extend(self.speak(&rest));
                }
                commands.extend(self.after_speech());
                commands
            }
            _ => Vec::new(),
        }
    }

    fn busy(&self) -> bool {
        matches!(self.state, VoiceState::Thinking | VoiceState::Speaking)
    }

    /// True once for the interrupted turn's terminal event, which lands after
    /// the replacing turn was sent and must neither end it nor surface its text.
    fn take_stale_terminal(&mut self) -> bool {
        std::mem::take(&mut self.interrupt_outstanding)
    }

    fn speak(&mut self, chunk: &str) -> Option<Command> {
        let text = speakable(chunk);
        if text.is_empty() {
            return None;
        }
        let id = format!("u{}", self.next_utterance);
        self.next_utterance += 1;
        self.queue.push_back(id.clone());
        Some(Command::Helper(HostToHelper::Speak { id, text }))
    }

    fn after_speech(&mut self) -> Vec<Command> {
        if !self.queue.is_empty() {
            return Vec::new();
        }
        if self.turn_done && self.busy() {
            vec![self.set_state(VoiceState::Listening)]
        } else if self.state == VoiceState::Speaking {
            vec![self.set_state(VoiceState::Thinking)]
        } else {
            Vec::new()
        }
    }

    fn try_listen(&mut self) -> Vec<Command> {
        if self.state == VoiceState::Starting && self.helper_listening && self.session_key.is_some()
        {
            vec![self.set_state(VoiceState::Listening)]
        } else {
            Vec::new()
        }
    }

    fn interrupt_session(&mut self) -> Vec<Command> {
        self.reset_turn();
        self.session_key
            .clone()
            .map(|session_key| Command::InterruptSession { session_key })
            .into_iter()
            .collect()
    }

    fn reset_turn(&mut self) {
        self.queue.clear();
        self.chunker = SentenceChunker::new(MIN_CHUNK_CHARS);
        self.turn_done = true;
        self.interrupt_outstanding = false;
        self.partial = None;
        self.level = None;
    }

    fn fail(&mut self, message: String) -> Vec<Command> {
        self.reset_turn();
        self.state = VoiceState::Error;
        vec![Command::Push(self.message(message))]
    }

    fn set_state(&mut self, state: VoiceState) -> Command {
        self.state = state;
        self.partial = None;
        self.level = None;
        Command::Push(self.snapshot())
    }

    fn message(&self, message: String) -> VoiceEvent {
        let mut event = self.snapshot();
        event.message = Some(message);
        event
    }
}

pub(crate) enum DriverInput {
    SessionReady(String),
    SessionFailed(String),
    TurnFailed(String),
    Interrupt,
    Disable,
}

pub(crate) struct DriverIo {
    pub(crate) sessions: Arc<dyn HostDispatcher>,
    pub(crate) pushes: mpsc::Sender<PushFrame>,
    pub(crate) shared: Arc<VoiceShared>,
    pub(crate) inputs: mpsc::Sender<DriverInput>,
}

enum Effect {
    Send { session_key: String, text: String },
    Interrupt { session_key: String },
}

/// Drives one enabled voice session until `Disable`, then tears the helper
/// down and pushes `off`.
pub(crate) async fn run(
    mut machine: Machine,
    helper: Helper,
    helper_events: mpsc::Receiver<HelperEvent>,
    mut inputs: mpsc::Receiver<DriverInput>,
    mut engine: mpsc::Receiver<EngineEvent>,
    io: DriverIo,
) {
    let mut helper = Some(helper);
    let mut helper_events = helper_events;
    let mut helper_alive = true;
    let (effects, effect_receiver) = mpsc::unbounded_channel();
    let mut effects_task = tokio::spawn(run_effects(
        effect_receiver,
        Arc::clone(&io.sessions),
        io.inputs.clone(),
    ));
    let mut last_level = Instant::now()
        .checked_sub(PUSH_THROTTLE)
        .unwrap_or_else(Instant::now);
    let mut last_partial = last_level;
    loop {
        let commands = tokio::select! {
            event = helper_events.recv(), if helper_alive => match event {
                Some(HelperEvent::Message(message)) => machine.handle(Input::Helper(message)),
                Some(HelperEvent::Exited) | None => {
                    helper_alive = false;
                    helper = None;
                    if machine.restarts == 0 {
                        machine.restarts += 1;
                        match helper::spawn(&helper::helper_path()) {
                            Ok((restarted, events)) => {
                                helper = Some(restarted);
                                helper_events = events;
                                helper_alive = true;
                                machine.handle(Input::HelperRestarting)
                            }
                            Err(error) => machine.handle(Input::HelperFailed(format!(
                                "The voice helper could not be restarted: {error}"
                            ))),
                        }
                    } else {
                        machine.handle(Input::HelperFailed(
                            "The voice helper stopped unexpectedly.".to_owned(),
                        ))
                    }
                }
            },
            Some(event) = engine.recv() => machine.handle(Input::Engine(event)),
            input = inputs.recv() => match input {
                Some(DriverInput::SessionReady(key)) => machine.handle(Input::SessionReady(key)),
                Some(DriverInput::SessionFailed(message)) => {
                    machine.handle(Input::SessionFailed(message))
                }
                Some(DriverInput::TurnFailed(message)) => machine.handle(Input::TurnFailed(message)),
                Some(DriverInput::Interrupt) => machine.handle(Input::Interrupt),
                Some(DriverInput::Disable) | None => break,
            },
        };
        for command in commands {
            match command {
                Command::Helper(message) => {
                    if let Some(helper) = helper.as_mut()
                        && let Err(error) = helper.send(&message).await
                    {
                        tracing::warn!(%error, "voice helper write failed");
                    }
                }
                Command::SendTurn { session_key, text } => {
                    let _ = effects.send(Effect::Send { session_key, text });
                }
                Command::InterruptSession { session_key } => {
                    let _ = effects.send(Effect::Interrupt { session_key });
                }
                Command::Push(event) => push_voice_event(&io.pushes, &io.shared, event).await,
                Command::PushLevel(event) => {
                    if last_level.elapsed() >= PUSH_THROTTLE {
                        last_level = Instant::now();
                        push_voice_event(&io.pushes, &io.shared, event).await;
                    }
                }
                Command::PushPartial(event) => {
                    if last_partial.elapsed() >= PUSH_THROTTLE {
                        last_partial = Instant::now();
                        push_voice_event(&io.pushes, &io.shared, event).await;
                    }
                }
            }
        }
    }
    drop(effects);
    if let Some(mut helper) = helper.take() {
        let _ = helper.send(&HostToHelper::Stop).await;
        let _ = helper.send(&HostToHelper::Shutdown).await;
        helper.stop().await;
    }
    // A turn already handed to `iface:send` settles on its own: aborting it
    // mid-dispatch would leave the collaboration send started but never settled.
    if tokio::time::timeout(EFFECTS_GRACE, &mut effects_task)
        .await
        .is_err()
    {
        effects_task.abort();
    }
    let mut off = machine.snapshot();
    off.state = VoiceState::Off;
    push_voice_event(&io.pushes, &io.shared, off).await;
}

/// Session effects run in order on one task so a barge-in interrupt always
/// lands before the turn that follows it.
async fn run_effects(
    mut effects: mpsc::UnboundedReceiver<Effect>,
    sessions: Arc<dyn HostDispatcher>,
    inputs: mpsc::Sender<DriverInput>,
) {
    while let Some(effect) = effects.recv().await {
        let (operation, payload) = match effect {
            Effect::Send { session_key, text } => (
                CHANNELS.send,
                json!({"sessionKey": session_key, "text": text}),
            ),
            Effect::Interrupt { session_key } => {
                (CHANNELS.interrupt, json!({"sessionKey": session_key}))
            }
        };
        if let Err(error) = sessions.dispatch(operation, payload).await {
            tracing::warn!(operation, code = %error.code, "voice session effect failed");
            if operation == CHANNELS.send {
                let _ = inputs
                    .send(DriverInput::TurnFailed(format!(
                        "The voice session could not take that turn: {}",
                        error.message
                    )))
                    .await;
            }
        }
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
