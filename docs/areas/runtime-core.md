# Runtime core — interface-engine, interface-runtime, interface-orchestration

Rules for editing here: `crates/AGENTS.md`. Navigation: `docs/CODEMAP.md`.

The provider-neutral spine between engine adapters and the two hosts.

**crates/interface-engine** — behavioral boundary only.

- `adapter.rs` — `EngineAdapter`/`SessionHandle` traits, approval verdicts,
  `ProviderCommandRequest`/`Result`, `HistoryRequest`.
- `blocking.rs` — `run`, which moves provider-store work onto the blocking
  pool: a task that never returns is definitely-unsent for a read and
  outcome-unknown for a mutation. Claude's and Grok's store calls use it.
- `outcome.rs` — `EffectCertainty`, `EngineErrorKind`, `EngineError`,
  `DeliveryOutcome`: the crate's fail-closed vocabulary. `into_result` and
  `error` unwrap an outcome for the adapters; the error keeps its certainty.
- `shell_words.rs` — `shell_word`/`is_plain_byte`, the POSIX quoting Codex and
  ACP both use to join an argv into the command text the approval policy reads.
- `jsonrpc.rs` — NDJSON framing with a hard per-line byte bound, plus
  `PendingRouter`, the dispatch ledger that keeps definitely-unsent and
  outcome-unknown honest across writer and transport failures, and the
  RPC-outcome → `DeliveryOutcome` translations `delivery_from_rpc`,
  `prepare_error` and `frame_error`.
- `supervised.rs` — the one supervised JSON-RPC child engine-codex and
  engine-acp run on: `SupervisedTransport<E>` (spawn, the writer, reader,
  stderr and supervisor tasks, the request family with its timeouts, receipts
  and shutdown), `Wire<E>` (the process-free decoder over `PendingRouter`), the
  byte-budgeted inbound queue, `TransportLimits` and `validate_limits`. `E` is
  the provider's event; its `Classify` impl turns each routed stdout line into
  that event and supplies the nouns in the frozen error prose (`Codex` /
  `Codex app-server`, `ACP` / `ACP agent`).
- `session_core.rs` — `SessionCore<E>`, the session half both share:
  readiness (`ReadyState`, `wait_ready`), the one `emit` path, `fail_open`, and
  `fail_transport`, the single report of a transport's death (closed event or
  end of inbound routing, whichever comes first); plus `closed_outcome`,
  `validate_cwd` and `value_text`.
- `event_stream.rs` / `event_budget.rs` — policy v2 byte/count bounded channels.
  Providers retain at most 8 MiB of measured event payload, subscribers 4 MiB.
  Text/tool/diff output may be replaced by one in-order omission notice per
  backlog. Approval, lifecycle and delivery control events are never silently
  dropped; control-only exhaustion closes with an explicit stream error.
  The old count-only factory remains available for legacy callers/tests.
  interface-runtime's `pump.rs` also bounds its ingress channel with a 32 MiB
  weighted semaphore; Codex/ACP framed queues clamp capacity against 64 MiB of
  encoded frame bytes.
  These are payload bounds, not allocator/RSS measurements.
- `test_support.rs` — built only for tests and the `test-support` feature
  (enabled from dev-dependencies): the collaboration broker fake that
  engine-acp's and engine-claude's adapter tests share.
- `thinking.rs` — `ThinkingRepeats`, which `SessionCore::emit` holds across
  every enqueue: a thinking signal identical to the event just
  emitted is dropped (the repeat the desktop drops), except once a second so a
  long reasoning span keeps the session's last activity moving.
- `executable.rs` — provider executable resolution order (override, login-shell
  PATH, known locations), PATH augmentation, the argv-only `ProcessSpec` that
  never goes through a shell, and the shared `--version` probe: `cli_version`
  plus `VersionCache`, which keeps a successful answer only while the path
  resolves to the same file (`FileVersion`: dev, inode, size, mtime, ctime).
  The probe ends when the CLI exits, not when its stdout closes.
- `child_output.rs` — reading a child's pipes until the child (and the group
  it leads) is done rather than until every copy of the pipe closes: on macOS
  a child another thread spawns before a pipe becomes close-on-exec inherits
  it. Used by `cli_version`, worker metadata `capture` and OpenCode usage.
- `lifecycle.rs` — `SessionPhase`/`SessionLifecycle` gate, kept separate from the
  visible `SessionStatus`. A failed turn moves only the status; only
  `mark_failed` and the close transitions end a handle.
- `worker.rs` — the content-worker proposal format and the one supervised CLI
  worker run (`run`, `capture`). Its `RuntimeDirectory` (with `create_named`
  for a caller's own prefix and error text), `ProcessGroup`, `cancelled` and
  `drain` are public so engine-acp's OpenCode and Antigravity workers and the
  OpenCode usage probe share them instead of keeping copies.

**crates/interface-runtime** — the host-facing service layer.

- `runtime.rs` — `RuntimeServices`/`SessionRuntime`: adapter registry, session
  open/resume transaction, live-attachment map and state machine, effect
  delivery and audit, shutdown. Its unit tests are in `runtime_tests.rs`.
  Provider readiness calls share a per-engine in-flight probe and recheck the
  kept successful probe after waiting. A success is reused for five minutes
  while the adapter's `executable()` still resolves to the same `FileVersion`
  (taken before the probe ran), and for 10 s when there is no file to watch.
  A failure is never kept and drops the kept success; unrelated providers do
  not share that lock. `probe_all_providers` (the renderer's `probeEngines`,
  sent on mount and by the Refresh actions, never on window focus) is the
  explicit refresh: it reuses only a probe that began after it asked.
- `pump.rs` (a child of `runtime`) — provider ingress, the per-session event
  pump with its process-local projection, the fail-closed stream-terminal and
  quarantine transitions, and the history/registry persistence helpers.
- `registry_cache.rs` (a child of `runtime`) — the `registry.json` cache
  writer. It is coalesced: status and every other content change save within
  1 s, but a provider event that moved only `last_activity` (the registry's
  `content_revision` did not advance) waits for a separate 15 s timer, a
  stream close or shutdown. Only crash-time `last_activity` on disk can lag;
  the in-memory registry never does. The writer holds the registry lock only
  for `prepare_save` (a row copy, about 0.2 ms for 1,000 rows); serializing
  and the fsync'd write run after it.
  A registry transaction that changed nothing (an identical provider listing)
  schedules no save while the cache is current, and the writer skips the copy
  when it is.
- `bridge.rs` — the transport-neutral `RuntimeBridgeCommand`/`Response` schema
  and the single async dispatcher.
- `management.rs` — the shared parts of management: `SessionManagementState`,
  the settle-once slot, registry access (`registry_transaction`,
  `registry_after_provider`) and the sanitized provider-error helpers. Each
  operation family is a submodule of `management/`, adding its own
  `impl SessionRuntime` block:
  - `listing.rs` — provider listing refresh and ingest fences. A full refresh
    skips a provider whose last complete listing is younger than its adapter's
    `listing_reuse` (Codex: 60 s; everyone else asks every time): its rows are
    already merged and its status keeps the time it really listed. A failed,
    incomplete or unavailable listing drops the kept one.
    `probe_all_providers` restarts every kept listing, and a refresh asked
    after it never joins a refresh that began before it.
  - `lifecycle.rs` — per-session lifecycle leases and coalesced settle-once
    units. `operations.rs` — forget, local/provider archive, rename, delete,
    fork, adopt, `send_live`, `engine_ops`.
  - `commands.rs` — provider command discovery: scope admission, the
    per-scope lease, the per-engine and global slots.
  - `transcripts.rs` — `read_provider_history` and the bounded transcript
    search. `import.rs` — bulk adoption with progress. `usage.rs` — plan usage.
- `history.rs` — process-local bounded event/effect projection: per-session
  sequences, retention trimming, stream and delivery terminal states. Disposable
  content budgets default to 4 MiB/session and 32 MiB total. Closed streams
  release content; approval/lifecycle/effect reserves preserve control facts and
  uncertainty and are outside these payload budgets. Provider stores own history.
- `project_root.rs` — bounded root discovery (256 queued roots, four Git children
  per batch), in-flight dedup, batch registry updates and shutdown idle barrier.
  A cwd that is missing or not a directory is answered by a stat, never a Git
  child. Negative results expire (missing directory: 15 min; non-repository:
  5 min; failures: 2 s); durable/provider roots win late results, and uncached
  queue tails precede expired negatives.
- `error.rs` — `RuntimeError`/`RuntimeErrorKind`, the sanitized failure shape
  carrying `EffectCertainty` through from the engine layer.

**crates/interface-orchestration** — deterministic multi-agent state.

- `reducer.rs` — the pure mutation reducer: per-entity state machines,
  capacity and idempotency gates, worktree lease and escrow transitions,
  `append_event` with the retention window.
- `store.rs` — one-writer CAS store over the legacy state/records layout:
  envelope validation, stale-temp quarantine recovery, append-only transition
  checks, poisoning on uncertain writes. `apply` keeps an index of the
  retained event ids so the reducer's duplicate-event check is a lookup, and
  release builds skip comparing every retained event of reducer output (it
  only appends); debug builds and every other replacement still compare them.
  The single-file snapshot layout is frozen 0.1.169 format, so each mutation
  still rewrites the whole record.
- `projection.rs` — byte-budgeted control-plane projections (agent pages,
  transcript, inbox, event pages) and the snapshot trimmer, plus
  `worker_session_keys`, the one rule for which agents are workers (a parent,
  not an orchestrator), read over the whole agent list: the control snapshot
  keeps only 64 agents.
- `transcript.rs` — the frozen human transcript renderer: one line per visible
  provider event, and `clean_orchestration_text`'s Node byte truncation (4080
  bytes, a split character becomes U+FFFD), pinned by
  `orchestration_text_cleaning_matches_node_byte_truncation`.
- `error.rs` — `OrchestrationErrorCode` taxonomy and the `StorageError` mapping.

**Entry points** — `SessionRuntime::builder(RuntimeServices)` →
`register_native_adapters_with_collaboration` → `build` (the only way a host
gets a runtime); `SessionRuntime::handle_bridge_command`
(`bridge.rs`, the single typed dispatcher every transport goes through);
`start_session`/`resume_session` → `open_session` → `prepare_provider_open` →
`activate_prepared_session`; `SessionRuntime::take_session_events`;
`SessionRuntime::shutdown`; `reduce_orchestration` (`reducer.rs`);
`OrchestrationStore::{open, snapshot, apply, compare_and_set_with, close}`;
`PendingRouter::{prepare_request, mark_write_started, mark_frame_accepted,
fail_before_acceptance, route, transport_failed}`.

**Invariants**

- Effect certainty is three-state and never collapsed. `DefinitelyUnsent` is the
  only replay-safe outcome, and `replay_is_safe` agrees across `EngineError`,
  `DeliveryOutcome`, `RpcCallOutcome` and `RuntimeDeliveryReceipt`.
- The dispatch ladder `Prepared -> WriteStarted -> FrameAccepted` is monotonic.
  Once a frame is accepted, the call settles only as Response / Rejected /
  OutcomeUnknown — never DefinitelyUnsent.
- A live session's `AtomicU8` state authorizes provider effects. `Close` is
  admitted from ACTIVE | OUTCOME_UNKNOWN | PERSISTENCE_FAILED; every other
  effect only from ACTIVE. A quarantine is never cleared implicitly.
- Two locks order a live session's effects (`LiveSession`). `effects`, the
  turn slot, runs sends, setting changes and closes one at a time in call
  order. `dispatch` orders every state gate, provider write and outcome
  publication. A send whose adapter reports its frame queued before the turn
  ends (`SessionHandle::send_reporting_dispatch`, ACP) releases `dispatch`
  and parks; while it is parked an approval or cancel skips the turn slot, and
  a close (or the pump's and shutdown's closes) cancels each parked send once
  before waiting for it. Every other adapter reports at the end of its send, so
  it never parks. Lock order is `effects` then `dispatch`.
- A subscriber queue that overflows with control events fails the session
  closed: the pump records an `Overflow` stream terminal, closes and evicts the
  attachment. A consumer that dropped its stream is not an overflow.
- Only the in-memory `live` map authorizes live effects. Durable history may
  still say open after a forced exit and must not be consulted.
- Evict an attachment only through `remove_if_current`, never a bare
  `HashMap::remove`, or a completed pump evicts its replacement.
- Provider-first ordering for management ops: mutate the provider store, then the
  local projection. A local failure after a confirmed provider effect surfaces
  as `provider_applied_local_error` (OutcomeUnknown), never a plain local error.
- Orchestration snapshots are append-only per collection and advance by exactly
  one revision with exactly one appended event; only the oldest event prefix may
  be compacted.
- `reduce_orchestration` is pure: no clocks, no randomness, no I/O. Timestamps
  come from mutation metadata and are compared as strings, so every persisted
  timestamp must be RFC3339 UTC with millisecond precision.
- Control-plane projections must fit `ORCHESTRATION_CONTROL_PUBLIC_MAX_BYTES`
  (512 KiB) and assert it before returning.
- Rust and Node never write the same canonical store. `OrchestrationStore` holds
  a `WriterLock` for its whole lifetime and poisons itself on `WriteUncertain`.

**Gotchas**

- Policy v2 `BoundedEventStream::recv` can return count or byte overflow. Every
  consumer must fail closed on either; a missing control receipt is not success.
  Legacy `bounded_event_stream` retains the old count-only drop-oldest behavior.
- `EngineHistoryStore` is process-local by design: `open(_root, ..)` ignores its
  path argument and creates no file. It is a projection, not a store.
  `read_provider_history` and `history_session` are deliberately different
  things; do not merge them.
- `interface-runtime` does not use `tracing`; best-effort failures go to
  `eprintln!`. Adding tracing here is a dependency change, not a drive-by.
- Deliberate frozen surface with no Rust caller: `RuntimeBridgeCommand` variants,
  `EngineOp` entries, `RuntimeStreamState`/`RuntimeEventTermination` variants,
  and the adapter trait's defaulted methods — they exist so a provider lacking a
  capability answers explicitly instead of returning an empty success.
  `EngineAdapter::supports_history` is separate from `read_history` for exactly
  that reason.
- `ProviderCommandRequest` deliberately omits the engine id, and `setting_sources`
  distinguishes `None` from `Some([])` for Claude settings isolation. Do not
  normalize either.
- `catalogue_model_key` strips a trailing `[..]` suffix only for the claude
  engine; model-catalogue merge dedup depends on it.
- `canonical_command_scope` does a TOCTOU-hardened symlink check
  (`symlink_metadata`, canonicalize, re-stat, dev/ino compare) before any
  provider is contacted. Do not reduce it to a single `canonicalize`.
- `is_legacy_temp_name` (`store.rs`) must stay `strip_suffix(".tmp")`: clippy
  pedantic forbids `ends_with` extension comparisons, and the check is
  deliberately case-sensitive for 0.1.169 parity.
- The lifecycle map coalesces identical concurrent operations but rejects
  differing ones; `fork_stored_session` (and `fork_session_at`, which shares
  its body and calls the adapter's `fork_session_at`) uses the non-coalescing
  `begin`, so a concurrent fork is refused rather than joined.
- `ProjectRootProjector::prepare` must be called while holding the
  session-registry lock; `schedule()` must then be called *outside* it.
- `SessionManagementState::deleted_through_listing` is a generation fence, not a
  cache.
