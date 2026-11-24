# Apple speech helper

Read root `AGENTS.md`, this directory's `README.md`, and `docs/VOICE_ASSISTANT.md`.
Navigation: `cargo xtask navigate voice-helper` from the repository root.

Swift adapts Apple's speech/audio APIs; Rust owns protocol policy and turn logic
in `crates/interface-voice` and `apps/interface-host/src/voice`. Keep NDJSON stdout
machine-readable, diagnostics on stderr, audio buffers bounded and shutdown orderly.

Run `xcrun swift test --package-path apps/interface-voice` from the repository
root and the required `cargo xtask check`. The audio self-test speaks aloud;
microphone, permissions and speech recognition need separate runtime evidence.
