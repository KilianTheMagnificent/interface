# Application performance and maintainability review

Review baseline: fetched Dev `6a2f10d2846b5ba236e9cb31de196e9ad905a938`.
Work branch: `perf/application-review-20260915`, isolated from the owner's dirty
canonical main checkout. September 15, 2026.

## Findings and implementation plan

| ID | Priority | Finding and trigger | Planned change |
| --- | --- | --- | --- |
| P1 | Medium | `interface-workspace/src/search.rs`: many matches on a long line repeatedly scan the line end, UTF-16 column prefix and whole preview. Even a short preview allocates the full tab-expanded line. | Advance line/column cursors once, reuse previews on the same line, and stop preview construction after the existing UTF-16 boundary. Preserve multiline matches and exact preview bytes. |
| P2 | Medium | `engine-claude/src/store/history_cache.rs`: an append hashes the old prefix to verify it, then rereads and hashes that prefix again to update the cache. Listing/history also duplicate identity/version/fingerprint helpers. | Share the read-only verification helpers. Extend the verified hash while parsing complete appended records; keep partial final records outside the committed fingerprint. |
| P3 | Medium | `use-shared-draft.ts`: fixed-interval reads queue behind slow saves/reads, including while hidden. `ControlDesk.tsx` can overlap its initial read with its first timer read. Both controls and drafts poll hidden windows. | Share a completion-scheduled visible polling helper across drafts, Away and Control. Start at most one automatic read, stop timers while hidden, refresh on return, and preserve serialized draft saves and conflict refusal. |
| P4 | Medium | `interface-engine/src/event_stream.rs`: dropping the sole receiver leaves its queued payloads owned by surviving producer handles, although no consumer can read them. | Take the queue during receiver teardown and destroy its contents outside the mutex. Preserve producer rejection and normal close/drain behavior. |
| P5 | Low | `interface-remote/src/replay.rs`: each caught-up read scans up to 512 retained frames under the shared mutex; active clients revisit frames preceding their cursor. | Skip frames before the cursor using their ordered offsets; allocate only the returned byte range. Preserve gap, generation and approval behavior. |

Implementation follows P1–P5 with focused regression checks and synthetic
before/after measurements, then `cargo xtask check`, Dev closeout and runtime
verification. No dependencies, wire versions, persisted layouts, detailed audit
retention or visual designs need changing.

## Review coverage and decisions

This is a repository-wide, risk-led source review of critical paths, supported by
the existing test gates; it is not an assertion that every line is defect-free.
Inspected the Rust provider framing and lifecycle, runtime history and root
projection, orchestration projections, collaboration/audit and Away stores,
storage/editor/search boundaries, terminal queues and transports, remote replay
and input authorization, Electron polling/state/transport, CLI redraw scheduling,
native phone transport/polling, voice boundaries and build/release workflow.

The September 14 fixes are present. Existing frame limits, queued-output credit,
bounded histories, incremental projections and event-driven CLI redraw are useful
and should remain. The retained audit history is an owner decision, not dead data.

Large `App.tsx`, `tui.rs`, terminal-service and provider-adapter files warrant
continued feature-by-feature extraction, but file size alone does not justify a
wide rewrite. Their lifecycle, uncertainty and compatibility branches are often
intentional. Do not merge provider decoders with different malformed-frame
semantics, remove uncalled frozen operations, drop durable receipts, weaken
file-identity checks, or change retained-event policy as cleanup. No evidence from
this pass justifies those changes. Existing whole-snapshot durable stores may
become a future bottleneck at their capacity limits; redesign requires separate
measurements and compatibility work.

## Implementation and measurements

P1–P5 are implemented. Claude listing/history share `store/cache_file.rs`;
`visible-polling.ts` replaces the three separate timer lifecycles. Neither change
adds a dependency or a canonical store. Event teardown destroys abandoned
payloads outside the stream mutex. Replay uses ordered frame offsets and retains
the existing wire bytes and approval-consumption rules.

Synthetic local fixtures, same machine/toolchain, unoptimized Rust test profile:

| Measurement | Before | After |
| --- | ---: | ---: |
| 1,049,976-byte line, 200 matches, median of five search projections | 2,248.69 ms | 0.1203 ms |
| Ten appending reads of a 10,650,000-byte Claude transcript, 500-record tail | 340.07 ms | 171.57 ms |
| Ten unchanged cached history reads (control) | 2.94 ms | 3.03 ms |
| Ten uncached history reads (control) | 422.82 ms | 425.03 ms |

The search measurement isolates match projection; it excludes directory traversal
and disk reads and deliberately exercises a pathological long line. History
still verifies the entire old prefix on each append. These are not release-build,
RSS, live-provider or installed-app latency claims. Manual benchmark entrypoints:
`search::tests::benchmark_dense_long_line_search` and
`store::history_cache::tests::benchmark_unchanged_and_growing_history` (ignored in
the normal gate).

## Validation and outcome

Focused checks passed: 44 workspace tests, 78 Claude tests, 24 engine tests,
29 remote tests; manual benchmarks above are separate. Clippy with `-D warnings`
and Electron typechecking passed. Five scheduler tests cover slow initial reads,
visibility churn, failures and disposal. Real Electron fixtures confirm slow-read
coalescing, hidden draft polling, conflict preservation, Away receipt reconciliation
and Stop while control delivery is uncertain.

Initial socket tests and renderer startup were refused by sandbox permissions;
the remote suite passed with local socket access enabled. The full gate passed
1,644 Rust tests (19 ignored), 2,704 JavaScript tests, three Swift-helper tests,
renderer interactions and builds. The final full gate also passed on the exact source including the smoke-script
fix. Implementation `38d547a` landed as `2c8c5559cbd430762ab29585e0fca34bc8e1048f`
with the name **Improve search, transcript reads and idle resource use**.
Closeout reused exact-source Rust/Electron receipts, pushed Dev and verified a
renderer → preload → Rust-host response from the refreshed Dev app on `2c8c555`.
Evidence: `/private/tmp/interface-review-20260915-closeout.log`. No provider calls, production
promotion, staging or installation are part of this review.

Evidence logs: `/private/tmp/interface-review-20260915-{search-before,history-before,
focused-rust,remote-tests,clippy,renderer,full-gate}.log`. Final gate: `/private/tmp/interface-review-20260915-final-gate.log`. Exact
measurement record: `/private/tmp/interface-review-20260915-measurements.json`.

## Follow-up discovered during verification

Fixed: `electron/scripts/smoke-remote.cjs` expected protocol 21 / 112 operations,
while the checked host/client contract is 22 / 113. It now imports the existing
client constants. The standalone smoke passed against the disposable Rust host,
original PTY and Chromium: shared-draft CAS, reconnect cursors, stale generation,
unknown outcomes, approvals, revocation and host restart all passed. See
`/private/tmp/interface-review-20260915-remote-smoke.log`; screenshots are in
`/private/var/folders/_b/hwd9v4ks1gb8dcf1jl66s8xc0000gn/T/interface-remote-evidence-BOmHSf`.
This is a test-maintenance fix, not a protocol change.

Live provider performance, physical phone/microphone acceptance, signed packaging,
updater staging, and installed-production tests were not run. Browser-based remote
smoke is not physical iPhone evidence.

