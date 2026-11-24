# Performance and Efficiency Audit — 2026-08-31

## Scope and constraints

This audit covered the Rust workspace, desktop host, `iface` TUI/agent server,
Electron main/preload/renderer layers, and build/package scripts. It traced
startup sequencing, provider refreshes, blocking work inside async paths, hot
event/transcript mutations, bounded retention, renderer payload/paint behavior,
process spawning, and repeated persistence.

The accepted contracts remain the boundary:

- provider stores stay authoritative;
- public Rust history DTOs and their JSON shape stay unchanged;
- provider-effect certainty remains fail-closed;
- the Electron renderer owns presentation, not canonical state;
- no dependency or persisted-schema change is part of this work.

The legacy TypeScript checkout was treated as a read-only behavioral oracle.
Credential and environment files were not inspected.

## Prioritization and implementation plan

The implementation tranche selected issues that were both on a real hot or
startup path and independently testable without changing a frozen protocol.
The order below was used so contract-preserving primitives landed before UI
wiring and whole-repository verification.

1. Remove synchronous provider-store work from Tokio worker paths while
   preserving effect certainty.
2. Bound initial renderer history and make deeper replay an explicit,
   generation-fenced request.
3. Remove avoidable O(n) retention and full-history debug work from the live
   runtime event path.
4. Isolate periodic renderer state and reclaim deleted-session projections.
5. Take independent startup work off the first-render critical path.
6. Bound CLI tail projection by the caller's requested result.
7. Run scoped behavior tests, the canonical workspace gate, then refresh the
   source-backed Dev app from the pushed `dev` revision.

## Implemented gains

| Area | Finding | Implementation and preserved behavior |
| --- | --- | --- |
| Grok adapter | `grok_store::{list_sessions,history,delete}` performed synchronous directory/file work inside async adapter methods. A large history scan could stall unrelated Tokio work, and an outer timeout could not advance on a current-thread runtime. | `engine-acp` now runs reads and mutations through blocking workers. Read-worker failures remain definitely unsent; a failed mutation worker is conservatively outcome-unknown. Provider rejections and returned DTOs are unchanged. |
| Renderer history | Opening each pane immediately requested the 5,000-message hard ceiling, bypassing the existing 500-message default and multiplying parse, IPC, allocation, and paint cost across panes. | Initial reads now request 500 messages. **Load older** advances in 500-message strict-superset pages up to 5,000, replaces rather than concatenates the page, preserves scroll anchoring, keeps the current page on failure, and uses request generations to reject stale success/failure/delete/retry results. After any live event, paging stays frozen through close and retry so historical and live prose cannot overlap; only authoritative removal resets that fence for a later same-key import. |
| Runtime history | Retention used `Vec::remove(0)`, shifting up to 10,000 events per eviction. Every debug mutation also revalidated every retained event in every session. | The private process-local store now uses `VecDeque` and validates only the newly introduced tail edge on append/status mutations. Public `HistorySession` and `RuntimeHistorySnapshot` still expose `Vec` fields with the same camelCase JSON arrays. Repeated wrap, omitted-sequence, order, and JSON round-trip tests cover the boundary. |
| Conversation paint | A one-second elapsed-time update lived in the top-level `Conversation`, forcing reconciliation of the transcript, composer, approvals, tools, and subagent UI every tick. | The interval and elapsed state now live in a leaf `LiveTurnElapsed`; the parent retains only turn-start/final-duration ownership. Each tick updates the clock subtree only. |
| Deleted sessions | Successful delete/forget cleanup left the reducer's potentially large event array and provider-command catalogue reachable; a queued animation-frame batch could also recreate dropped state. | A reducer `drop` action removes only that session's event and command projections, while centralized successful cleanup also clears history/born-here lifecycle state and invalidates queued events for that key. The bounded per-frame batch generation prevents a stale callback from resurrecting a deleted or same-key-reused session. Other reducer state and identity on no-op cleanup remain unchanged. |
| Desktop startup | Native-updater signature discovery was awaited before the renderer was loaded, even though appearance, bridge setup, and first paint do not consume its result. | Updater configuration starts early and overlaps renderer/bridge startup. Its rejection is normalized immediately; all public updater requests and `updater.start()` await the same successful configuration, so no stale pre-configuration state or native event can escape. A delayed fatal failure destroys the just-loaded surface and enters durable shutdown directly, preventing a dirty-unload veto from stranding a half-started process. |
| `iface agent read` | A small tail request formatted and allocated every eligible line in the complete transcript, then sliced the end. | The TUI now reverse-walks eligible cells and lines and stops at the requested count, preserving multiline order, ignored cell kinds, role labels, and terminal sanitization. |

## Measurements

Measurements are local synthetic evidence, not production latency guarantees.
The history benchmark used the same public `EngineHistoryStore` API, a 10,000
event warm history, and timed 5,000 additional appends on this machine.

| Build | Baseline | Candidate |
| --- | ---: | ---: |
| Debug | 884 ms | 1 ms |
| Release | 144 ms | <1 ms (millisecond timer resolution) |

Other bounded improvements are structural:

- the initial history record ceiling falls from 5,000 to 500 per pane (90%
  fewer records at the request boundary when the provider has that much data);
- retained-event eviction changes from O(n) front shifting to O(1);
- `iface agent read --lines N` formats at most the requested eligible tail
  instead of the whole transcript;
- updater configuration remains bounded by its existing startup deadline but
  is no longer on the renderer-load critical path.

## Deferred findings and next plans

These are real opportunities, but each needs a separate design/measurement
tranche because it changes process lifetime, freshness semantics, canonical
projection strategy, or renderer architecture.

| Priority | Finding | Next implementation plan and acceptance evidence |
| --- | --- | --- |
| P0 | Claude listing scans every candidate JSONL to derive metadata, while Codex control operations create short-lived app-server connections. Focus/visibility refresh can trigger the expensive provider path repeatedly. | Add provider-specific listing caches keyed by safe file metadata and a bounded freshness window; design one supervised persistent Codex control connection with explicit shutdown/reconnect fencing. Prove refresh freshness, provider restart recovery, and no stale lifecycle overwrite. |
| P0 | Project-root projection starts one Git process per uncached cwd; each completion scans the registry and can schedule a cache save. Many distinct roots therefore create process and persistence bursts. | Batch distinct cwd resolution behind bounded concurrency, apply all resolved roots in one registry transaction, and schedule one cache save. Measure process count and registry writes on a synthetic multi-root listing; preserve durable/worktree root precedence. |
| P0 | `iface` first paint awaits provider probes, listing, orchestration state, and command discovery in a mostly serial startup path. | Define a minimal first-frame snapshot, start independent reads concurrently, and apply generation-fenced late results. Accept only with PTY first-frame timing plus provider/listing failure behavior tests. |
| P1 | Registry/listing projection still contains repeated linear lookup and mutation patterns; runtime re-registration still performs a full debug history validation. | Introduce private indexes or batched mutation APIs without changing public order/DTOs. Benchmark N-session listing and reattach at retention cap, with exact snapshot equivalence tests. |
| P1 | The TUI's large transcript projection and renderer's up-to-10,000 live-event log still clone/recompute broad structures; long conversations can approach the DOM/reconciliation ceiling even after timer isolation. | Add revisioned per-session view caches and measured transcript virtualization/windowing. Preserve find, scroll anchoring, approvals, tool expansion, and accessibility in real rendered tests. |
| P1 | Review checks can spawn Git per file/layer, multiplying process startup for broad reviews. | Batch review Git queries by repository and reuse parsed repository facts across layers. Verify identical findings and compare process counts on a multi-file fixture. |
| P2 | The renderer ships as one eager bundle, and image paste creates large transient encoded copies. Dev startup also repeats some TypeScript/build work. | Establish bundle/heap/startup baselines first, then lazy-load genuinely cold surfaces, move image encoding across a bounded binary boundary, and remove only demonstrably duplicate dev compilation. Keep offline packaging and first-use behavior covered. |

The collaboration audit rewrite and several orchestration projection copies were
also identified. Their current behavior is deliberate for atomicity and bounded
snapshot semantics, so they should change only with explicit storage/protocol
design rather than as a local micro-optimization.

During final reconciliation, a separate concurrent `dev` change introduced a
request-animation-frame-coalesced terminal splitter controller with exact final
coordinate flushing. It is covered by the combined repository gate but is not
claimed as part of this audit's implementation.

## Verification record

- `cargo test -p engine-acp`: 54 passed.
- `cargo test -p interface-runtime`: 54 passed after the added wrap test.
- `cargo test -p iface`: 115 unit and 4 real-PTY tests passed.
- Scoped Clippy with warnings denied passed for `engine-acp`,
  `interface-runtime`, and `iface`.
- The merged Electron suite passed 2,418 tests; release-script tests,
  renderer/main TypeScript checks, and the production Vite build passed.
- `cargo xtask check` passed on the exact merged tree. Its first restricted
  run hit only sandbox-denied Unix sockets in nine `iface` tests; the identical
  unrestricted rerun passed, including those tests.
- One later full-gate attempt exposed a scheduler-sensitive command-discovery
  test (three probes observed instead of two). The exact test, its complete
  16-test binary, and the final canonical gate all passed on immediate rerun.
- Live-provider paging and packaged-app timing were not run. Packaging,
  updater-feed staging, and production installation were outside the approved
  `dev` closeout boundary.
