# crates/interface-contracts

Rules for editing here: `crates/AGENTS.md`. Navigation: `docs/CODEMAP.md`.

Leaf crate every other Rust crate depends on. Serde field names, enum
discriminators and `validate()` gates that must agree byte-for-byte with the
TypeScript oracle. Dependencies are `chrono`, `regex`, `serde_json`, `uuid`,
plus `async-trait` for the collaboration binding traits. Keep it I/O-free.

**Modules**

- `validation.rs` — `ValidationError` plus the `pub(crate)` primitives every gate
  builds on: `bounded_text`/`bounded_trimmed`, `validate_timestamp`,
  `validate_absolute_path`, `validate_digest`, `validate_identifier`, `ensure_unique`.
  Public: `canonical_timestamp` and `parse_canonical_timestamp` (the formatter
  and the parser for the frozen timestamp form, which collaboration and iface
  use too) and `lowercase_hex`, re-exported from `remote_connection`.
- `engine.rs` — `EngineId`, session keys, capability/session/usage DTOs, the
  `EngineEventKind` wire enum with its hand-written envelope `Deserialize`,
  collaboration MCP binding traits, context-window math.
- `ipc.rs` — desktop IPC DTOs plus the frozen channel table macro and `parse_key`.
  Current counts are in [checked protocol facts](../PROTOCOL_FACTS.md), asserted
  by the channel tests and `apps/interface-host/src/protocol.rs`. Do not infer
  the current protocol from a historical feature's version number.
- `terminal_controls.rs` — protocol v13 screen observations, model/effort selections
  and settings state. The settings channel is retired and answers unavailable;
  the screen shape is what autonomous pane input and phone approvals read. A
  shared JSON fixture pins Rust decoding and the TypeScript request encoder.
- `orchestration.rs` — the durable orchestration graph: limits, agent/message/
  effect/lease DTOs, `OrchestrationMutation`, and the authoritative snapshot
  validator.
- `orchestration_control.rs` — owner-private control-wire methods, request/
  response envelopes, body-free projected types, and the policy, coordination
  and model-catalogue DTOs the host's orchestration handlers build. The retired
  orchestrator tool DTOs are gone; the live `not_an_orchestrator` refusal is a
  JSON literal in `interface-collaboration/src/mcp.rs`.
- `provider_commands.rs` — normalization of untrusted provider command
  catalogues, slash-command parsing, the app-owned passthrough blocklist.
- `prompt_images.rs` — magic-byte format sniffing, data-URL preview validation,
  safe basename extraction, thumbnail sizing.
- `questions.rs` — `AgentQuestion` DTOs and tolerant cross-provider
  `AskUserQuestion` payload parsing.
- `retention.rs` — UTF-16-faithful label/output clipping with a truthful
  omitted-character marker. Live: `engine-codex` uses it.
- `appearance.rs` — `ThemePreference` (built-in palettes), `ThemeId` (a
  built-in or a theme file, `custom:<name>` on the wire and on disk),
  `CustomThemeName`, `SystemThemes` (Follow macOS's light/dark pair),
  `CustomThemeSummary`, `AppearanceState` and `AppearanceState::resolve` (a
  missing theme file paints what Follow macOS would). See
  [Theme files](../THEMES.md).
- `terminal.rs` — `TerminalId`/`TerminalGeneration`, launch profiles, workspace
  records, request DTOs, lifecycle events.
- `threads.rs` — Threads surface v1 (protocol 34): `ThreadCommand` (tagged on
  `kind`), `ThreadsView`/`ThreadView`, `ThreadControlResponse` and the
  workspace, turn, diff and restore DTOs. Rust and TypeScript round-trip
  `electron/src/fixtures/threads-v34.json`, `threads-v36.json` for each
  turn's `endedAt` (protocol 36), and `threads-hidden-v58.json` for the
  view's `hiddenKeys`, the registry sessions Threads and Chat leave out as
  workers (protocol 58). See the
  [threads design](../superpowers/specs/2026-10-06-threads-design.md).
- `updater.rs` — `UpdateState` and its validity invariants (`canRestart` iff Ready).

**Entry points** — `OrchestrationSnapshotV1::validate` (called by
`interface-orchestration/src/store.rs` on every load and CAS);
`ipc::ALL_CHANNELS`/`EXTRA_CHANNELS`/`is_desktop_channel`;
`EngineEvent`'s Serialize/Deserialize pair and `EngineEvent::validate`;
`normalize_provider_commands`.

**Invariants**

- Every DTO is camelCase + `deny_unknown_fields`; every enum carries an explicit
  serde rename; golden-JSON tests pin the exact bytes.
- `electron/scripts/contract-parity.test.mjs` (Electron's `npm run
  test:release`) reads these sources beside `electron/src/{shared,main,remote}`:
  every same-named struct and interface must agree on field names after serde
  renames and on which fields may be absent (`skip_serializing_if` needs a TS
  `?`), and the channel and push tables must name the same channels. Its
  `ALLOWED` and `UNCHECKED` tables hold the deliberate differences and the
  pairs it cannot read; each needs a reason.
- `validate()` returns `ValidationError`, never panics — it runs on JSON straight
  off disk and off the wire. Arithmetic on wire integers must saturate.
- Message receipts: non-empty, first state `Requested`, `receipts[0].at ==
  created_at`, `receipts.last().at == updated_at`, non-decreasing.
- Retained event window: contiguous by sequence, first `== floor + 1`, last
  `== revision`, empty iff `floor == revision`.
- Effect lifecycle: `Reserved` ⇒ attempts 0 and no timestamps; `Started`/
  `Succeeded`/`Unknown` ⇒ attempts 1 and `startedAt`; `Succeeded`/`Failed`/
  `Unknown` ⇒ `settledAt`. `Unknown` is deliberately grouped with the
  started-and-settled cases; do not simplify it into `Failed`.
- `EngineEventKind::PeerResult.untrusted` must be `true`.
- Timestamps are compared with string `<`. Sound only because
  `validate_timestamp` pins a fixed-width `YYYY-MM-DDTHH:MM:SS.mmmZ` form: a
  24-byte shape check, then a chrono RFC 3339 parse. Any value passing both
  renders back to itself, so no format round-trip is needed; the accept/reject
  set (leap second, year 0000, Feb 29) is pinned by
  `timestamp_gate_keeps_the_canonical_accept_reject_set`.
- Control lines: exactly one trailing `\n`, no interior newline, within
  `ORCHESTRATION_CONTROL_MAX_LINE_BYTES + 1`; a response object has exactly 3
  keys and exactly one of `result`/`error`.

**Gotchas**

- The retained-event-floor expression uses saturating subtraction in
  `orchestration.rs`, `ipc.rs`, and `interface-orchestration/src/projection.rs`.
  Preserve agreement between validation and projection when changing it.
- `provider_commands.rs` defines a private `bounded_text(&Value, usize) ->
  Option<String>` that shadows `validation::bounded_text` with completely
  different semantics (char-counted, truncating, `None` instead of an error).
- `APP_OWNED_COMMANDS` contains sentinel entries `fast-never-owned` and
  `unused-sentinel` mirroring the TS list. Not typos.
- `validate_identifier` is stricter than `bounded_trimmed`: it additionally
  rejects interior `\t\n\r`. That difference is load-bearing.
- Untagged enums here are order-sensitive. `ConsultationSafetyProfile` and
  `OrchestrationControlResponsePayload` rely on distinct field sets.
- `EngineEvent` has a hand-written `Deserialize` that flattens `at` beside the
  tagged event and rejects duplicate keys; derived serde attributes only affect
  the Serialize side.
- Snapshot validation deliberately skips role-claim and evidence-link checks the
  TS oracle performs. A snapshot that passed `validate()` is not proof of
  coherent claims or evidence.

Protocol 14 adds the Rust-owned Agent workspace command channel; see [agents](agents.md).
