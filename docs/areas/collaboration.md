# crates/interface-collaboration

Rules for editing here: `crates/AGENTS.md`. Navigation: `docs/CODEMAP.md`.

The durable cross-model collaboration domain. Provider launches and source
snapshots live outside; this crate is handed values and returns
`EffectCommand`s only after the corresponding attempt is durably committed.
Everything model-facing is capability-gated: model input can never choose its
own authority.

The separate v1 Away pilot uses `away.rs` for bounded queue/retry/acceptance
and recovery, with effects behind its `Executor` trait. It has its own store and
does not use or change the legacy collaboration record contract below. See
[`AWAY_PILOT.md`](../AWAY_PILOT.md).

`away_cycle.rs` and `away_cycle/store.rs` add the separate cycle v1 controller:
fixed deadlines, immediate feedback receipts, explicit acknowledgments, fresh
candidate-bound review and cancellation. This does not alter the pilot or legacy
record schemas. The shared CLI/skill is documented in [`AWAY_CYCLE.md`](../AWAY_CYCLE.md).

The control-desk v1 store (`voice_control.rs`) was retired with the Control
assistant on 2026-10-14; its `voice/control/control.v1.json` is left on disk,
unread. See [the archive](../archive/control-assistant-20261014/README.md).
`valid_id`, the identifier rule it shared with project Away and the workers
broker, now lives at the crate root.

`project_away.rs` and `project_away/store.rs` own the separate desktop project loop:
one-click admission, saved presets, native work/fresh review, exact Stop, deadlines
and no replay after restart. See [Project Away](../PROJECT_AWAY.md).

**Modules**

- `snapshot_file.rs`: the commit protocol project Away, the Away pilot, the
  Away cycle and the workers index share: writer lease, strict
  preparation before any I/O, the last write's exact snapshot, and
  `WriteError::Refused` (nothing reached the target) versus `Uncertain` (disk
  may have moved). Each store keeps its own validation, revision rule and latch
  policy: project Away latches only on `Uncertain`; the Away
  pilot (edits land in memory before the write) and the workers broker
  (documented: any failed write poisons it) still latch on both. Their bytes
  are pinned by `tests/golden/`.
- `workers.rs` / `workers/`: separate v1 optional report service, host-provided
  scope, shared queue, immutable report evidence, per-lead collection and review.
  Capture runs outside the shared state lock with two capture slots. Durable
  state operations run serially on blocking tasks; admission rechecks capacity,
  parent identity, idempotency and the current grant after capture.
  `workers/history.rs` archives acknowledged evidence and replay receipts;
  `workers/grants.rs` stores owner-only project grants outside model tools.
  It does not enable legacy mandatory delegation; [current scope](../WORKERS_MCP.md).

- `apps/interface-host/src/collaboration/translation.rs` — frozen request
  validation and DTO translation, separated from host effect coordination.

- `packet.rs` — the canonical packet v2 builder and the v1 peer advisory: the
  pure transcript-to-prompt rules that decide what a peer model sees
  (sanitization, per-mode context budgets, newest-first record selection,
  handoff file list, 32 KiB packet bound). The host reads the source's history
  and live transcript and calls `build_collaboration_packet`; its live
  projection shares `sanitize_text` and `bounded_tail`.
- `state/` — the versioned contract, re-exported whole by `lib.rs`
  (`pub use state::*`). `mod.rs` holds `CollaborationEngineId` (its source is
  read by the renderer's `draft-store.test.ts`), the `invalid_if!` macro and
  the unit tests; `types.rs` the constants, enums and DTO/persisted structs;
  `normalize.rs` the byte and character bounds, normalization and hashing
  primitives; `validate.rs` the per-sub-record and full record validation (one
  `invalid_if!(failed, path, rule)` per rule, in first-failure order) and
  record creation; `transition.rs` the legal-transition, maintenance-effect
  and effect-successor tables, the transition-delta checks and
  `transition_collaboration_record`; `receipt.rs` the receipt/audit
  projections.
- `journal.rs` — single-writer durable store: writer lease bound to the records
  inode, in-memory record/dedupe/snapshot indexes, create + compare-and-set with
  on-disk byte and inode verification, stale `.tmp` quarantine on open.
- `coordinator.rs` — the driver: admission and idempotent replay, approval
  issue/refresh/expiry, concurrency and target-capacity limits, effect
  reservation/start/settlement, peer hold/accept/discard, recovery quarantine.
- `broker.rs` — provider-free authority core: capability digests and one-time
  use, session bindings and exact source confirmation, Codex source
  registration, depth fences, synchronous revocation.
- `thread_handoff.rs` — Threads' Second opinion and Continue with (review
  B4): the consult objective naming the latest turn's changes (inside the
  objective's bounds) and the first prompt of a thread continuing another on
  a different provider (the thread's changes and newest conversation, inside
  the packet bound). Pure; the host's `threads/opinion.rs` calls it.
- `broker_runtime.rs` — host-owned lifetime: validates the socket root and the
  `nc` command, removes sockets a killed broker left (owner-only, broker-named,
  connect refused; a socket that accepts stays), starts one MCP listener per
  binding, implements the session
  factory, Codex registrar and depth-fence leases with idempotent dispose.
- `mcp.rs` — the owner-private MCP server: socket identity and permission
  verification, newline and Content-Length JSON-RPC framing, initialize/ready
  gating, per-connection in-flight limits, the four tool definitions
  (`collaboration_mcp_tools`; the nine retired orchestrator names stay in
  `COLLABORATION_MCP_TOOL_NAMES` and refuse with `not_an_orchestrator`),
  untrusted worker-output clamping. Each client connection is one
  `Arc<Connection>` (listener settings, its own settlement tracker, writer,
  pending requests, initialize flags) shared by its reader loop and request
  tasks.
- `delegates.rs` — the Away CLI's worker runner: `DelegateRequest` (task,
  files, context files, model), `DelegateJob` and the `DelegateRunner` trait
  (`run`, `apply`); the host's `CliDelegate` implements it. Away cycles
  validate proposals with `interface_workspace::delegation::prepare_validation`
  in iface's `CycleExecutor::validate`, not through the runner. The rest of
  pane delegation was retired on 2026-10-14
  ([archive](../archive/pane-delegation-20261014/README.md)).
- `clip.rs` — byte-bounded `head`/`tail` clipping on character boundaries,
  shared by the coordinator's provider-body bound, project Away and the Away
  pilot's refinement feedback.
- `audit.rs` — privacy-preserving counters and `CollaborationAuditSink`, an
  lossless JSONL log. `audit/storage.rs` owns numbered 256 KiB active segments
  and cumulative v2 checkpoints; sealed segments and the original log are kept.
  See [audit storage v2](../AUDIT_STORAGE_V2.md).

**Entry points** — `CollaborationJournal::open(root)` →
`CollaborationCoordinator::with_generators` → `recover()` **before any
admission** (recovery flips `accepting` on and quarantines durable `started`
attempts into `unknown`). Human flow: `submit_human`, or `begin_human` →
`human_preview_material` → `complete_human_preview`/`fail_human_preview`, then
`approve_human`/`deny_human`/`cancel_human`. Effect flow: `reserve_prepare` →
`start_reserved_effect` → `settle_effect`. Peer flow: `hold_peer_result` →
`accept_peer_human`/`discard_peer_human`. Model flow:
`CollaborationBrokerRuntime::start` → `provider_bindings()` for the engine crates.

**Invariants**

- Every record mutation goes through `commit` → `transition_collaboration_record`
  → `journal.compare_and_set`. Nothing else writes a record, and the journal
  re-reads and byte-compares the committed file before each CAS.
- One attempt per effect: reserved → started (in its own same-state maintenance
  revision) → settled. A `started` attempt is never replayed; recovery converts
  it to `unknown` (non-retryable).
- Ambiguous outcomes are never retryable. `failure.ambiguous ==
  code.is_ambiguous()`, `Unknown` requires an ambiguous effect or a
  RecoveryRequired marker, and ProvenUnsent codes are demoted to
  BrokerUnavailable if ambiguous.
- Approval is single-use and bound to (packet hash, source guard hash, source
  registry revision, target engine). Any drift in the source guard forces a
  fresh approval record.
- Terminal records must drop `intent.objective`, `packet.body` and
  `peer_result.body`, and `terminal_at` must equal `updated_at`.
- Identifiers that must survive a replay are deterministic domain-separated
  digests, never random.
- The journal file name is `sha256(collaboration_id).json`, and `records/` may
  contain nothing but those files plus stale `.tmp` writes. That is the
  rollback-compatibility contract with the frozen TypeScript reader.
- One writer per provider session (review B4): the host's port refuses a
  preview whose source a Code pane holds (or is opening for), and every
  effect that would write to such a session — target launch, the consult's
  advisory, a handoff's park — definitely unsent, before reading the source
  (`SessionRuntimeCollaborationPort::set_session_claims`).
- Raw broker capabilities are never stored, only sha256 digests; each capability
  registers exactly once; source ownership is exclusive and revocation is
  synchronous.
- The MCP transport never lets the connection capability into model input;
  authority is resolved out-of-band from the listener the request arrived on.

**Gotchas**

- Do **not** replace `journal.rs`'s `recover_legacy_layout` with
  `interface_storage::quarantine_stale_atomic_writes_for`: the storage helper
  quarantines into a `.quarantine` child of the same directory, and
  `load_records` rejects any entry in `records/` that is not `<64-hex>.json`.
  The sibling `quarantine/` directory is deliberate.
- `lib.rs` does `pub use state::*`, so a lot of surface is public. Several `pub`
  items have no in-repo caller and are retained parity surface
  (`COLLABORATION_STATES`, `COLLABORATION_ERROR_CODES`,
  `COLLABORATION_CONTRACT_VERSION`, `MAX_TERMINAL_RECEIPTS`,
  `TERMINAL_RETENTION_MS`, `TRANSIENT_BODY_CRASH_BACKSTOP_MS` (mirroring
  `state.ts:36` in the oracle), `CollaborationJournal::root`, and most of the
  `Collaboration*` aliases in `coordinator.rs`).
- `BrokerRuntime::drain` is intentionally a no-op — the authority core has no
  transports. `CollaborationBrokerRuntime::drain` is the one that drains sockets.
- `EffectOutcome::Ambiguous.code` is ignored by `settle_effect`; the ambiguous
  code is derived from the effect kind, which is what record validation requires.
- Records in `Unknown` are reconciled to `Failed` by the host at startup
  (`reconcile_unknown_failed`, wired 2026-08-29), unlocking their source
  session; the record keeps a reconciliation marker and the effect is never
  redispatched.
- Approval drift is checked in one place, `source_drift`, shared by approval,
  target reservation and dispatch. It compares and records the snapshot
  worktree with the same `unwrap_or(Unmanaged)` normalization `complete_preview`
  uses; without it `changed` is permanently true.
- `tests/suite/record_golden.rs` pins the persisted bytes, receipts and
  commands of every coordinator path (`tests/golden/collaboration-coordinator-trace.txt`)
  and the first failure message of single-field mutations of those records.
  A refactor must keep both passing unchanged.
- `tests/suite/mcp_transcript.rs` pins every byte the MCP server writes, and
  their order, for a scripted session per source engine
  (`tests/golden/mcp-transcript.txt`): tool schemas, refusals, error payloads,
  deadlines, limits and both framings.
- `CollaborationRecordV1::validate` is pure but expensive (canonical JSON +
  SHA-256 several times, all timestamps re-parsed) and already runs roughly seven
  times per committed transition. Do not add more calls casually.
- MCP request cancellation tokens are children of the per-request token, not the
  socket stop token, so cancelling the socket does not cancel in-flight requests.
- The audit sink atomically replaces only its active segment per append and
  poisons itself on any storage error; numbered sealed segments are never reused.
  Audit failure never rolls back a committed transition.
  Snapshot counters restore from the v2 checkpoint plus active segment and update
  only after successful writes. Opening v1 is read-only; the first event publishes
  the migration. Downgrade needs a v2-aware reader or a complete detailed export.
- `javascript_trim` / `has_unsafe_controls` / `JS_MAX_SAFE_INTEGER` exist because
  the wire format round-trips through the legacy JavaScript bridge. Do not swap
  them for `str::trim` or `u64::MAX` bounds.
