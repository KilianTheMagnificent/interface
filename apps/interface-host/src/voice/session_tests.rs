use std::{
    os::unix::fs::PermissionsExt as _,
    sync::atomic::{AtomicBool, Ordering},
};

use serde_json::Value;

use super::*;
use crate::dispatcher::DispatchError;

fn listening() -> Machine {
    let mut machine = Machine::new("/p".into());
    assert!(
        machine
            .handle(Input::SessionReady("claude#s1".into()))
            .is_empty()
    );
    let commands = machine.handle(Input::Helper(HelperToHost::State {
        listening: true,
        speaking: false,
    }));
    assert!(
        matches!(&commands[..], [Command::Push(event)] if event.state == VoiceState::Listening)
    );
    machine
}

fn delta(text: &str) -> Input {
    Input::Engine(EngineEvent {
        at: None,
        event: EngineEventKind::MessageDelta {
            text: text.into(),
            agent_key: None,
        },
    })
}

fn turn_end() -> Input {
    Input::Engine(EngineEvent {
        at: None,
        event: EngineEventKind::StatusChanged {
            status: SessionStatus::Idle,
        },
    })
}

fn speak_ids(commands: &[Command]) -> Vec<&str> {
    commands
        .iter()
        .filter_map(|command| match command {
            Command::Helper(HostToHelper::Speak { id, .. }) => Some(id.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn ready_starts_the_helper_and_listening_needs_both_helper_and_session() {
    let mut voiceless = Machine::new("/p".into());
    let commands = voiceless.handle(Input::Helper(HelperToHost::Ready {
        protocol: 1,
        voices: Vec::new(),
        default_voice: String::new(),
    }));
    assert!(matches!(
        &commands[..],
        [Command::Push(event)] if event.state == VoiceState::Error
            && event.message.as_deref() == Some(NO_VOICE)
    ));
    let mut machine = Machine::new("/p".into());
    let commands = machine.handle(Input::Helper(HelperToHost::Ready {
        protocol: 1,
        voices: Vec::new(),
        default_voice: "v".into(),
    }));
    assert!(matches!(
        &commands[..],
        [Command::Helper(HostToHelper::Start { end_silence_ms: 800, locale })] if locale == "en-US"
    ));
    assert!(
        machine
            .handle(Input::Helper(HelperToHost::State {
                listening: true,
                speaking: false,
            }))
            .is_empty()
    );
    assert_eq!(machine.state(), VoiceState::Starting);
    let commands = machine.handle(Input::SessionReady("claude#s1".into()));
    assert!(
        matches!(&commands[..], [Command::Push(event)] if event.state == VoiceState::Listening && event.session_key.as_deref() == Some("claude#s1"))
    );
}

#[test]
fn final_sends_the_turn_and_enters_thinking() {
    let mut machine = listening();
    let commands = machine.handle(Input::Helper(HelperToHost::Final {
        text: " hello there ".into(),
    }));
    assert!(matches!(
        &commands[..],
        [Command::SendTurn { session_key, text }, Command::Push(event)]
            if session_key == "claude#s1" && text == "hello there" && event.state == VoiceState::Thinking
    ));
    assert_eq!(machine.state(), VoiceState::Thinking);
}

#[test]
fn deltas_become_speak_commands_in_order_and_the_turn_returns_to_listening() {
    let mut machine = listening();
    machine.handle(Input::Helper(HelperToHost::Final { text: "hi".into() }));
    let first = machine.handle(delta("**Hello** there, how are you"));
    assert!(speak_ids(&first).is_empty());
    let second = machine.handle(delta(" today? I am fine. Ok"));
    // "I am fine." is under the 20-character minimum, so it waits.
    assert_eq!(speak_ids(&second), ["u1"]);
    assert!(matches!(
        &second[0],
        Command::Helper(HostToHelper::Speak { text, .. }) if text == "Hello there, how are you today?"
    ));
    let end = machine.handle(turn_end());
    assert!(matches!(
        &end[..],
        [Command::Helper(HostToHelper::Speak { id, text })] if id == "u2" && text == "I am fine. Ok"
    ));
    let started = machine.handle(Input::Helper(HelperToHost::SpeechStarted {
        id: "u1".into(),
    }));
    assert!(matches!(&started[..], [Command::Push(event)] if event.state == VoiceState::Speaking));
    assert!(
        machine
            .handle(Input::Helper(HelperToHost::SpeechFinished {
                id: "u1".into()
            }))
            .is_empty()
    );
    let last = machine.handle(Input::Helper(HelperToHost::SpeechFinished {
        id: "u2".into(),
    }));
    assert!(matches!(&last[..], [Command::Push(event)] if event.state == VoiceState::Listening));
}

#[test]
fn interrupted_clears_the_queue_and_a_final_while_busy_cancels_first() {
    let mut machine = listening();
    machine.handle(Input::Helper(HelperToHost::Final { text: "hi".into() }));
    let speak = machine.handle(delta("One longer sentence right here. Another one here."));
    assert_eq!(speak_ids(&speak), ["u1"]);
    let interrupted = machine.handle(Input::Helper(HelperToHost::Interrupted));
    assert!(matches!(
        &interrupted[..],
        [Command::InterruptSession { session_key }, Command::Push(event)]
            if session_key == "claude#s1" && event.state == VoiceState::Listening
    ));
    assert!(
        machine
            .handle(Input::Helper(HelperToHost::SpeechFinished {
                id: "u1".into()
            }))
            .is_empty()
    );
    assert_eq!(machine.state(), VoiceState::Listening);

    machine.handle(Input::Helper(HelperToHost::Final {
        text: "again".into(),
    }));
    let busy = machine.handle(Input::Helper(HelperToHost::Final {
        text: "no wait".into(),
    }));
    assert!(matches!(
        &busy[..],
        [
            Command::Helper(HostToHelper::Cancel),
            Command::InterruptSession { .. },
            Command::SendTurn { text, .. },
            Command::Push(_)
        ] if text == "no wait"
    ));
}

#[test]
fn a_final_while_thinking_swallows_only_the_interrupted_turns_result() {
    let mut machine = listening();
    machine.handle(Input::Helper(HelperToHost::Final {
        text: "first".into(),
    }));
    let replaced = machine.handle(Input::Helper(HelperToHost::Final {
        text: "second".into(),
    }));
    assert!(matches!(
        &replaced[..],
        [
            Command::Helper(HostToHelper::Cancel),
            Command::InterruptSession { .. },
            Command::SendTurn { .. },
            Command::Push(_)
        ]
    ));
    // The interrupted turn's cancelled result lands after the new turn
    // was sent: no state change, no message, and the new turn is spoken.
    let stale = machine.handle(Input::Engine(EngineEvent {
        at: None,
        event: EngineEventKind::Error {
            cause: None,
            message: "Request was aborted.".into(),
            retryable: false,
        },
    }));
    assert!(stale.is_empty());
    assert_eq!(machine.state(), VoiceState::Thinking);
    let spoken = machine.handle(delta("The second answer is here. And more"));
    assert_eq!(speak_ids(&spoken), ["u1"]);
    let started = machine.handle(Input::Helper(HelperToHost::SpeechStarted {
        id: "u1".into(),
    }));
    assert!(matches!(&started[..], [Command::Push(event)] if event.state == VoiceState::Speaking));
    // Only one terminal event is stale: the new turn's own end still counts.
    assert_eq!(speak_ids(&machine.handle(turn_end())), ["u2"]);
    for id in ["u1", "u2"] {
        machine.handle(Input::Helper(HelperToHost::SpeechFinished {
            id: id.into(),
        }));
    }
    assert_eq!(machine.state(), VoiceState::Listening);

    // When the CLI was already idle the interrupt yields no result at all;
    // the new turn's first delta lifts the guard so its own end is honoured.
    machine.handle(Input::Helper(HelperToHost::Final {
        text: "third".into(),
    }));
    machine.handle(Input::Helper(HelperToHost::Final {
        text: "fourth".into(),
    }));
    assert_eq!(
        speak_ids(&machine.handle(delta("The fourth answer is here. "))),
        ["u3"]
    );
    machine.handle(Input::Helper(HelperToHost::SpeechFinished {
        id: "u3".into(),
    }));
    assert!(
        matches!(&machine.handle(turn_end())[..], [Command::Push(event)] if event.state == VoiceState::Listening)
    );
}

#[test]
fn levels_carry_the_current_partial_and_a_final_clears_both() {
    let mut machine = listening();
    let partial = machine.handle(Input::Helper(HelperToHost::Partial { text: "hel".into() }));
    assert!(matches!(
        &partial[..],
        [Command::PushPartial(event)]
            if event.partial.as_deref() == Some("hel") && event.level.is_none()
    ));
    let level = machine.handle(Input::Helper(HelperToHost::Level { rms: 0.5 }));
    assert!(matches!(
        &level[..],
        [Command::PushLevel(event)]
            if event.partial.as_deref() == Some("hel") && event.level == Some(0.5)
    ));
    let sent = machine.handle(Input::Helper(HelperToHost::Final {
        text: "hello".into(),
    }));
    assert!(matches!(
        &sent[..],
        [Command::SendTurn { .. }, Command::Push(event)]
            if event.partial.is_none() && event.level.is_none()
    ));
}

const FAKE_HELPER: &str = r#"#!/usr/bin/env python3
import json, sys, threading, time
def emit(message):
    print(json.dumps(message), flush=True)
emit({"type": "ready", "protocol": 1, "voices": [], "defaultVoice": "v"})
emit({"type": "state", "listening": True, "speaking": False})
def later():
    time.sleep(0.1)
    emit({"type": "final", "text": "hello"})
threading.Thread(target=later, daemon=True).start()
for line in sys.stdin:
    if '"shutdown"' in line:
        break
"#;

/// `iface:send` takes half a second and records whether it got to finish.
#[derive(Default)]
struct SlowSessions {
    started: AtomicBool,
    settled: AtomicBool,
}

#[async_trait::async_trait]
impl HostDispatcher for SlowSessions {
    async fn dispatch(&self, operation: &str, _payload: Value) -> Result<Value, DispatchError> {
        if operation == CHANNELS.send {
            self.started.store(true, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(500)).await;
            self.settled.store(true, Ordering::SeqCst);
        }
        Ok(json!({}))
    }
}

#[tokio::test]
async fn disable_lets_an_in_flight_send_settle_before_stopping_the_effects_task() {
    let temporary = tempfile::tempdir().unwrap();
    let script = temporary.path().join("fake-interface-voice");
    std::fs::write(&script, FAKE_HELPER).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let (helper, helper_events) = helper::spawn(&script).expect("fake helper");
    let sessions = Arc::new(SlowSessions::default());
    let dispatcher = Arc::clone(&sessions) as Arc<dyn HostDispatcher>;
    let (inputs, input_receiver) = mpsc::channel(4);
    let (_engine, engine_receiver) = mpsc::channel(4);
    let (pushes, mut push_receiver) = mpsc::channel(64);
    let driver = tokio::spawn(run(
        Machine::new("/p".into()),
        helper,
        helper_events,
        input_receiver,
        engine_receiver,
        DriverIo {
            sessions: dispatcher,
            pushes,
            shared: Arc::new(VoiceShared::default()),
            inputs: inputs.clone(),
        },
    ));
    inputs
        .send(DriverInput::SessionReady("claude#s1".into()))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !sessions.started.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the final reached iface:send");
    inputs.send(DriverInput::Disable).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), driver)
        .await
        .expect("driver stopped")
        .unwrap();
    assert!(
        sessions.settled.load(Ordering::SeqCst),
        "disable aborted the in-flight send"
    );
    let mut last = None;
    while let Ok(frame) = push_receiver.try_recv() {
        last = Some(frame.payload["state"].clone());
    }
    assert_eq!(last, Some(json!("off")));
}

#[test]
fn denied_microphone_is_an_error_with_the_settings_hint() {
    let mut machine = listening();
    let commands = machine.handle(Input::Helper(HelperToHost::Permission {
        microphone: "denied".into(),
    }));
    assert!(matches!(
        &commands[..],
        [Command::Push(event)] if event.state == VoiceState::Error
            && event.message.as_deref() == Some(MICROPHONE_DENIED)
            && event.microphone.as_deref() == Some("denied")
    ));
    assert!(
        machine
            .handle(Input::Helper(HelperToHost::Final { text: "hi".into() }))
            .is_empty()
    );
}
