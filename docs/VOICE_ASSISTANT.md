# Voice assistant

A built-in assistant you talk to. Toggle the microphone button in the toolbar, speak, and it
answers out loud while it can see and operate Interface through its own tools. Everything about
audio runs on the Mac; the only network traffic is the text conversation with the model.

Design: `docs/superpowers/specs/2026-09-03-voice-assistant-design.md`.
Plan: `docs/superpowers/plans/2026-09-03-voice-assistant.md`.

## What ships today (the bones)

| Piece | Where | State |
|---|---|---|
| Listening and speaking | `apps/interface-voice` (Swift, Apple frameworks only) | On-device `SpeechAnalyzer` transcription with partials and end-of-utterance; `AVSpeechSynthesizer` playback behind a `SpeechBackend` seam. The system voice is a placeholder. |
| Brain | `SessionKind::Voice` session on `engine-claude` | Sonnet, low effort, spoken-style prompt, shown on the Chat surface like any session. |
| Tools | `apps/interface-host/src/voice/` (`iface_voice` MCP server, over the shared `mcp_transport.rs`) | `app_state`, `list_panes`, `start_pane`, `send_to_pane`, `read_pane`, `close_pane`, `list_sessions`, `read_session`, `send_to_session`, `notify`, `ui_snapshot`, `ui_act`, `ui_screenshot`, `open_project`, `focus_pane`. `read_pane` and `send_to_pane` are also the pane server's (`pane_mcp.rs`). `app_state` and `list_sessions` carry the 50 most recently active sessions and a count of all of them; `list_sessions` takes a `query` (title, folder or engine) to reach older ones. |
| Renderer | `electron/src/renderer/voice-ui.ts`, toolbar toggle, statusbar indicator | DOM snapshot/act for the UI tools; approval surfaces carry `data-voice-guard` so the assistant can never click its own Allow. |
| Safety | engine permission prompt | Read-only tools are pre-allowed; every acting tool and shell command asks first. |

Turn flow: final transcript → user turn → streamed reply chunked into sentences → spoken →
back to listening. Speaking over it interrupts. Toggle off stops the microphone and the helper.

## Plans, in order

1. **Live smoke and tuning.** First real run: microphone permission, end-of-utterance timing,
   barge-in feel, tool narration length.
2. **A natural voice, when wanted.** Drop a second `SpeechBackend` into the helper. Candidates
   researched 2026-09-03: Kokoro-82M (free, Apache-2.0, ~86 MB, ANE port via FluidAudio or
   `kokoro-js`), or a paid cloud voice behind a key in the Rust host (ElevenLabs ~$45/mo,
   xAI TTS ~$11/mo, OpenAI mini TTS ~$13/mo at 30 min/day). Declined for now: no paid option.
3. **Brain per session.** Let the voice session run on Codex or Grok as well as Claude; both
   CLIs take MCP servers, so the tool surface carries over.
4. **Spoken approvals.** Read permission prompts aloud and accept a spoken yes/no.
5. **Hands-free polish.** VAD tuning, spoken alerts when a pane is waiting on you.
6. **Beyond the app.** System-wide computer use from the helper (Accessibility and Screen
   Recording grants) using the same tool names.

## Not planned

Wake word (no free engine worth shipping), cloud speech-to-text, shipping subscription login to
other users.
