# interface-voice

A macOS 26 command-line helper the Rust host spawns and drives over NDJSON (one
JSON object per line) on stdin/stdout. It listens with Apple's on-device
`SpeechAnalyzer` + `SpeechTranscriber` and speaks with `AVSpeechSynthesizer`;
Apple frameworks only, no third-party dependencies.

## Build

```sh
cd apps/interface-voice && xcrun swift build -c release
```

`Package.swift` embeds `Sources/interface-voice/Info.plist` into `__TEXT,__info_plist`
via a link path SwiftPM resolves against the package dir; TCC needs it to open the mic.

## Self-test

```sh
.build/release/interface-voice --selftest [--wav /path/out.wav]
```

Speaks "Interface voice helper ready" to the speakers, synthesises the same
phrase to a WAV with `AVSpeechSynthesizer.write(_:toBufferCallback:)`, then
transcribes it through the same `SpeechAnalyzer` pipeline. Prints one `selftest`
line; exits non-zero if fewer than 3 of the 4 words come back. No mic needed.

## Protocol (version 1)

| host -> helper | fields | effect |
| --- | --- | --- |
| `start` | `endSilenceMs` (800), `locale` ("en-US") | request mic permission, install assets, begin listening; idempotent |
| `stop` | | microphone off, analyzer finished |
| `speak` | `id`, `text` | enqueue an utterance |
| `cancel` | | stop speaking, drop the queue |
| `setVoice` | `voice` (identifier) | select a voice |
| `shutdown` | | stop listening, let queued speech finish, exit 0 |

| helper -> host | fields |
| --- | --- |
| `ready` | `protocol`, `voices[{id,name,quality?,language}]`, `defaultVoice` |
| `permission` | `microphone`: `granted` / `denied` / `undetermined` |
| `asset` | `status`: `installed` / `installing` / `failed`, `progress` |
| `state` | `listening`, `speaking` |
| `partial` / `final` | `text` |
| `speechStarted` / `speechFinished` / `speechCancelled` | `id` |
| `interrupted` | (barge-in cancelled speech) |
| `error` | `message`, `fatal` |
| `level` | `rms` (<= 10 Hz while listening) |

Unknown fields are ignored; every line is flushed as written and diagnostics go
to stderr only. `voices[].quality` is optional -- the AVSpeechSynthesizer backend
always sends it, a backend without quality tiers omits the key. Synthesis sits
behind the `SpeechBackend` protocol so another backend can replace `Speaker`.

An utterance ends when the transcriber returns a final result and then `endSilenceMs`
passes with no further result and no detected speech. While speaking, a partial of two
or more words or 300 ms of detected speech cancels the queue and emits `interrupted`;
the input node uses `setVoiceProcessingEnabled(true)` so it never interrupts itself.
