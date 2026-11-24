# Voice assistant — design

Date: 2026-09-03. Branch: `feat/voice-assistant`. Status: approved in chat by the owner; implementation in progress. Held off `dev` until the owner says so.

## Goal

A voice assistant built into Interface. The owner toggles it on with a toolbar button, talks, and it answers out loud in its own voice while it can see and operate the app through tools, the way computer use operates a screen. Everything about audio runs on the Mac; the only network traffic is the text conversation with Claude on the owner's existing subscription. No dependency on the owner's separate dictation app (FlowClone).

## Non-goals

Wake word, system-wide computer use over other apps, cloud speech services, shipping to other users (subscription login is personal use only), a custom neural voice (can be added later inside the same helper).

## Architecture

Four pieces, three of them already mostly present in the app.

| Piece | Where | Role |
|---|---|---|
| `interface-voice` helper | `apps/interface-voice/` (Swift, Apple frameworks only) | Mic capture with echo cancellation, on-device `SpeechAnalyzer`/`SpeechTranscriber` transcription with partials and end-of-utterance, `AVSpeechSynthesizer` playback, barge-in. Bundled as the third binary in `Contents/Resources/bin`. |
| Voice service | `crates/interface-voice` (pure parts, unit-tested) + `apps/interface-host/src/voice.rs` (wiring) | Spawns and supervises the helper over NDJSON stdio, owns the voice session, chunks streamed assistant text into sentences and speaks them, exposes app-control tools over MCP, pushes state to the renderer. |
| Voice session | existing `engine-claude` session in the sessions store | Title "Voice assistant", one per project cwd, reused and resumable. `--model sonnet`, `--effort low`, appended spoken-style system prompt, voice MCP server attached, permissions via the existing `--permission-prompt-tool stdio` approval path. Shows in the sidebar like any session, so the normal transcript view is the voice transcript. |
| Renderer | `electron/src/renderer/App.tsx` and friends | Toolbar toggle button, statusbar indicator (off / starting / listening / thinking / speaking / error, with a level meter while listening), auto-selects the voice session when enabled. Executes UI-bridge tool calls (phase 2). |

Process tree at runtime: Electron main → `interface-host` → { `interface-voice` helper, `claude` (voice session), PTYs... }. The helper is a child of the host exactly like a provider CLI. Nothing runs when the toggle is off.

## Turn flow

1. Toggle on → host spawns helper (or reuses), sends `start`, creates or resumes the voice session for the active project cwd, pushes state `starting` then `listening`. Renderer selects the session.
2. Helper emits `partial` lines (shown in the indicator) and one `final` line per utterance.
3. Host sends the final text as a user turn to the session; state `thinking`.
4. Assistant text deltas stream back. A sentence chunker splits them at sentence punctuation or newline (minimum ~20 characters unless turn end), strips markdown markers, replaces fenced code with "code omitted", and sends each chunk as `speak`. First `speechStarted` → state `speaking`.
5. Turn end flushes the remainder. When the last `speechFinished` arrives, state returns to `listening`.
6. Barge-in: if the user speaks while the assistant is speaking, the helper cancels playback and emits `interrupted`; the host calls the session's `interrupt`, drops queued chunks, and treats the new `final` as the next turn.
7. Toggle off → `stop` + `shutdown` to the helper, session left resumable, state `off`.

Tool calls made by the session produce no speech; the indicator shows `thinking`. Permission requests raised by the engine follow the existing approval UI in phase 1; phase 2 speaks them and accepts a spoken yes/no.

## Helper wire protocol (NDJSON, frozen, `protocol: 1`)

host→helper: `start{endSilenceMs,locale}`, `stop`, `speak{id,text}`, `cancel`, `setVoice{voice}`, `shutdown`.
helper→host: `ready{protocol,voices[],defaultVoice}`, `permission{microphone}`, `asset{status,progress}`, `state{listening,speaking}`, `partial{text}`, `final{text}`, `speechStarted{id}`, `speechFinished{id}`, `speechCancelled{id}`, `interrupted`, `error{message,fatal}`, `level{rms}`.
Unknown fields ignored on both sides. Full table in `apps/interface-voice/README.md`.

## Host ↔ renderer contract

One push frame and two request channels in phase 1, one push and one request more in phase 2. Each addition is the repo's atomic multi-file channel edit with golden JSON tests on both sides.

- Push `iface:voice` → `VoiceEvent { state, sessionId?, cwd?, partial?, level?, message?, microphone?, asset? }`.
- Request `voice.setEnabled { enabled: bool, cwd: string }` → `{ state, sessionId? }`.
- Request `voice.interrupt {}` → `{}`.
- Phase 2: push `iface:voiceUiCall { callId, tool, input }`; request `voice.uiResult { callId, ok, output }`.

## Tools exposed to the voice session (MCP)

Attached only to the voice session. Read tools are allow-listed; act tools go through the existing permission prompt.

Phase 1, host-native:
- `app_state` → projects (workspace store + session cwds), sessions (id, title, engine, cwd, status), panes (id, profile, cwd, title, waiting flag), active project.
- `list_panes`, `start_pane { profile: shell|claude|codex|grok|qwen|antigravity, cwd, title? }`, `send_to_pane { paneId, text, enter: bool }`, `read_pane { paneId, lines? }` (tail of a per-pane 64 KiB ring buffer tapped in the host output pump, ANSI stripped), `close_pane { paneId }`.
- `list_sessions`, `read_session { sessionId, lastN }`, `send_to_session { sessionId, text }`.
- `notify { title, body }` (existing OS notification path).

Phase 2, via the renderer UI bridge:
- `ui_snapshot` → compact accessibility-style tree of the live DOM: element ids, roles, labels, states, plus visible text of each terminal from its xterm buffer.
- `ui_act { id, action: click|type|key|focus, text? }` via Electron input synthesis.
- `ui_screenshot` → PNG from `webContents.capturePage`.
- `open_project { path }`, `focus_pane { paneId }` (renderer selection changes).

## Error handling

Helper exit or protocol error → state `error` with message, one automatic restart, then stays `error` until toggled. Mic denied → state `error` "Microphone access denied" with the System Settings hint. Speech asset installing → state `starting` with progress. Session launch failure → state `error`. Every error is visible in the indicator and logged by the host; nothing is silently swallowed. Toggle off always tears down cleanly.

## Safety

Voice-committed turns never auto-approve anything. Act tools and shell commands use the engine's permission flow. The MCP server is bound to the voice session id; other sessions cannot see the voice tools. The helper never receives anything but text to speak.

## Build and packaging

- `apps/interface-voice/Package.swift`, built with `xcrun swift build -c release`; `cargo xtask package-mac` builds it and copies it to `Contents/Resources/bin/interface-voice`; `EXPECTED_BINARIES` becomes three entries and the digest check covers it.
- `electron/scripts/dev.mjs` builds a debug helper next to the debug host so the Dev app finds it the same way.
- `NSMicrophoneUsageDescription` added to `electron-builder.yml` `extendInfo` and to the Dev app plist patch in `dev-app.mjs`. Hardened runtime stays off; no entitlements file needed.
- `cargo xtask check` stays the gate. The Swift build is added to the check only if it is fast and deterministic on this machine; otherwise `cargo xtask check-voice` runs it and the plan says so.

## Testing

- Rust unit tests: sentence chunker, markdown stripping, ANSI stripping, ring buffer, helper protocol decode/encode, VoiceEvent golden JSON.
- Contract tests: new push/request frames in `child_protocol.rs`, `ipc-payload.test.ts`, `rust-host-client.test.ts`.
- Host integration: `child_protocol.rs` case with a fake `interface-voice` executable that emits `ready`, `final`, `speechFinished`, driven through `voice.setEnabled`.
- Swift: `interface-voice --selftest` synthesizes a sentence to WAV, transcribes it back, and fails if fewer than 3 of 4 words are heard. Manual, not in the gate.
- Manual smoke in the Dev app: toggle, speak, hear the reply, barge-in, toggle off, and `start_pane` + `read_pane` through the assistant.

## Phases on this branch

1. Helper; voice service; session; chunker; push/channels; toolbar toggle and indicator; app-native tools; build and plist. Shippable.
2. UI bridge tools, spoken permission prompts, `open_project` / `focus_pane`.
3. Later, separate: nicer voice (Kokoro inside the helper), hands-free tuning, spoken alerts when a pane is waiting.
