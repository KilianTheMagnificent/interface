# apps/interface-host

The Rust process Electron speaks to over stdio. Every module here is an
**adapter**: validate a frozen payload, delegate to a crate under `crates/`,
translate failure into a `DispatchError` carrying explicit
`(retryable, replay_safe)` certainty. Canonical business logic belongs in the
crates — if you are writing a rule here, it is in the wrong file.

Module map, entry points and the mechanism behind these rules:
`docs/areas/interface-host.md`. Read it before editing here.

## Do not break

- **Exactly one owner per frozen operation.** `OperationOwnerRouter::new`
  validates every table and refuses to serve on overlap. There is no fallback
  dispatcher; an unowned operation returns `operation-not-migrated`.
- **The counts are asserted.** [Protocol facts](../../docs/PROTOCOL_FACTS.md)
  names the current version and request/push totals. The router sums its owner
  tables and checks them against `interface-contracts`; Electron checks the
  same handshake. Changing a table means updating those contracts together.
- **Never remove a channel.** `readFileSnapshot`, `saveFile`, `setWorkspaceOpen`,
  `searchProject`, `listReviewCheckpoints` are deliberately retained wire surface
  with no in-tree caller. A retired feature's channel (`advanceTerminalControls`,
  `voiceControl`, `readTerminalDelegation`) stays served and answers unavailable.
- `PROTOCOL_VERSION` is checked against the [protocol facts](../../docs/PROTOCOL_FACTS.md). A push channel can never be invoked as a request; a
  request channel can never be built as a `PushFrame`.
- Response/push ordering: a completed response is written only after every push
  already queued when it completed. Later pushes cannot extend that barrier.
- Fail-closed errors: anything that may already have taken effect must be
  `replay_safe == false`. Orchestration effects follow Reserve → Start → Settle
  exactly once; `Unknown` settles Unknown and is **never** retried.
- One writer per canonical store. Only the process holding `host.lock`
  constructs the stores, and a degraded owner keeps the lock so degradation can
  never admit a second writer.
- Desktop stores open before the runtime, so the startup ACP roster is immutable
  for the process's life.
- Terminal PTY bytes never cross the JSON control plane. Sequences are per
  (terminal_id, generation, direction), start at 0, generation 0 is invalid, and
  an EOF frame carries no payload.
- `orchestration_transport::prepare_interactive` / `prepare_one_shot` must run
  before any authoritative store is opened.

## Gotchas

- `TerminalService` callbacks run on plain `std::thread`s, not Tokio workers.
  `blocking_send` is correct there; `tokio::spawn` is not.
- `desktop_replay_is_safe` (`router.rs`) maps typed `DesktopEffectCertainty`
  exhaustively. Error prose must never determine replay safety.
- `clean_orchestration_text` (`interface-orchestration/src/transcript.rs`)
  truncates by bytes at 4080 and can emit a mid-character U+FFFD. Deliberate
  Node byte-truncation parity, pinned by
  `orchestration_text_cleaning_matches_node_byte_truncation`.
- The orchestration dispatcher holds the desktop host's live collaboration
  projection itself: the observer feeds it every forwarded event, and
  `OrchestrationRuntimePort::transcript` takes it as `live`, so no port method
  can bypass it.
- A missing terminal data plane is normal, not an error — protocol tests and
  non-Electron embedders have no fd3/fd4.
- Two shutdown drains must both run: `DurableCollaborationService::close`
  (no timeout) and `RuntimeHostDispatcher::close_event_pumps`, which gives
  each event pump 15 s and then aborts it, so a stuck pump can skip its
  terminal close projection and worktree release.
  `shutdown::timed` reports stages exceeding five seconds without cancelling
  effects or persistence. Do not turn that warning into an abort.

## Verify

```
cargo test -p interface-host
cargo clippy -p interface-host --all-targets -- -D warnings
cargo xtask check                  # canonical gate
```

Touching a channel table also means the Electron side: `cd electron && npx
vitest run src/main/ipc-payload.test.ts src/main/rust-host-client.test.ts`.
