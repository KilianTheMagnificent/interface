# Performance review implementation

The September 9, 2026 review was implemented on top of `f5225809`, with the
audit-storage migration withheld after automatic approval review required
authorization for its exact retention and recovery semantics. No first-party
store moved to Electron, and these fixes do not change frozen wire or persisted
schemas. Integration preserves Dev's separately landed agent workspace and
protocol v14 change (`9cfd42a2`); checked protocol facts reflect that revision.

## Finding coverage

| Finding | Implemented behavior | Limits and evidence |
| --- | --- | --- |
| R1: renderer history | `live-events.ts` batches immutable updates, accounts retained content, limits a continuous text run, and compacts disposable history with an explicit notice. | Defaults: 1 MiB estimated UTF-16 text per run, 4 MiB disposable history per session, 32 MiB across sessions, 10,000 disposable events. Unresolved approvals and current lifecycle/control facts are an exact reserve and may exceed those budgets. These are content estimates, not a hard JS heap cap. Tests cover Unicode, continuous/interleaved output, many sessions, approvals and reload. Provider history and outgoing prompts are unchanged. |
| R2: updater deadline | One timeout covers leader completion and both pipe drains; failure terminates the owned group with bounded cleanup. | Inherited stdout, stderr and a waiting leader all fail within the regression's one-second bound for a 50 ms deadline. Normal output, unsuccessful exit, retention overflow and read error paths are covered. Signing tools were not exercised live. |
| R3: voice tap | Meter publication and conversion-failure signaling use atomic single-slot handoffs. A non-audio timer emits telemetry and diagnostics. The converter input wrapper is reused. | A stalled-consumer test publishes 100,000 samples without accumulating a queue. Final transcript/speech messages retain their reliable ordered path. PCM output is still freshly allocated because the analyzer may retain it; microphone/Instruments verification is outstanding. |
| R4: test discovery | Vitest includes both `.test.ts` and `.test.tsx`; a guard compares its actual discovered paths with the source inventory. | The previously omitted `DelegationTaskCard.test.tsx` executes. The implementation snapshot discovers 136 files. |
| R5: Clean history | Claude has eight bounded parsed-tail cache slots with file identity/version checks, exact older-message counts, partial-line handling and prefix verification on growth. Codex permits four read-only lanes under contention. Clean pages retain unchanged event identities without whole-page JSON serialization. | Fresh-reader differential tests cover append, valid/invalid partial EOF, replacement, rewrite and truncation. Cached records are limited to an estimated 8 MiB per slot. Growth still scans existing bytes for integrity. Codex still requests full turns; it does not claim a delta or paged provider optimization. Cancellation/failure disposes the affected read child, and mutations remain outside these lanes. |
| R6: transcript projection | Flat streaming appends and replacement of a coalesced final delta reuse settled rows. | Tools, nesting, out-of-order structure and compaction fall back to the full reference reducer with identity reuse. Differential tests verify equivalence and immutable prior results. DOM windowing was not introduced: find, selection, scroll and draft behavior remain covered by the existing renderer lane. |
| R7: Clean scrollback | Settled scrollback rows are cached; current screen rows remain fresh. Public xterm marker, resize and buffer changes invalidate the cache. Native controls/search skip presentation work. | A 5,000-row fixture rereads fewer than 20 translated rows on an unchanged screen, and matches the reference after viewport edits, trimming and reflow. Cache entries are capped at 10,000; traversal and invalidation can still be linear. |
| R8: recovery polling | Hook-reset adaptive recovery intervals replace permanent 100 ms retries; manual capture is serialized by terminal ID and generation. Process observation has a deadline and output cap. | Independent-owner lock tests, inherited-pipe/overflow tests and the full terminal recovery suite cover the change. A regression caught during implementation was fixed so final direct-child receipts still fence stale live lookups. Exact PID/start-time/generation authority remains uncached. Idle CPU at 1/8/32 live panes has not been measured. |
| R9: audit rewriting | Design prepared; no audit implementation or migration applied. | [The concrete v2 proposal](AUDIT_STORAGE_V2_PROPOSAL.md) preserves the original file and lifetime counters but rotates detailed new events. It requires explicit owner approval. |
| R10: controller complexity | Extracted App session/config projections, conversation display grouping, terminal provider recovery, host collaboration DTO translation and TUI slash-command parsing. | Existing entry points retain effect/mutation ownership. Affected implementation-string assertions became behavior tests. Frozen boundary, contract, visual and packaging pins remain. These are focused responsibility separations; the large controllers still have further extraction opportunities. |
| R11: stale guidance | Area rules link to [checked protocol facts](PROTOCOL_FACTS.md); obsolete channel/version and subtraction claims were corrected. | A release-lane test compares the facts with handshake constants, alongside existing cross-language channel identity/count tests. Module maps describe the new boundaries. |

A secondary finding was also reproduced and fixed: request admission now wins
over a saturated push lane while there is capacity. Its regression starts the
push flood before submitting the request and preserves the existing completed
response ordering barrier.

## Measurements and reproducibility

The projection comparison uses actual TypeScript modules under Node 26.7.0,
ten warmups and fifteen measured updates. Each five-event group contains a user
message, prose, tool call/result and trailing prose. Only the last delta changes.
The original review snapshot was `033869c`; the implementation started at
`f5225809`. These are local synthetic timings, excluding React/layout and
provider execution, not an installed-app latency guarantee.

| Events / projected rows | Original median | Implementation median |
| --- | ---: | ---: |
| 500 / 400 | 0.186 ms | 0.0083 ms |
| 5,000 / 4,000 | 1.778 ms | 0.0264 ms |
| 10,000 / 8,000 | 3.184 ms | 0.0650 ms |

The continuous-text fixture supplies 4,096 ASCII deltas of 16 KiB each. It
previously retained all 64 MiB of text in one event; it now retains a 512 KiB
ASCII tail plus an omission notice (1 MiB under UTF-16 accounting).

The ignored Rust measurement
`cargo test -p engine-claude benchmark_unchanged_and_growing_history -- --ignored --nocapture`
compares the full reader and cached reader over 10,000 records / 10,650,000
bytes with a 500-record tail. In an unoptimized test build, ten fresh reads
took 611.7 ms, ten unchanged reads 8.5 ms, and ten growing reads 356.7 ms.
Growing reads still fingerprint the old prefix; no reduction in full-page IPC
bytes is claimed. The four-lane Codex fixture verifies concurrency without
contacting a provider.

The real Electron fixture exercised ten updates over 1,001 rows and only the
tail row rerendered. Four Clean panes retained drafts, scrolling, send scope,
approvals and nested activity. The terminal MessagePort fixture transferred
512 frames with a matching checksum and released all credits. These fixtures
validate behavior under synthetic load, not a live provider workload.

The installed Codex schema and [official app-server documentation](https://learn.chatgpt.com/docs/app-server)
were inspected before changing the history path. `thread/turns/list` is
experimental and does not supply the existing exact older-message total, so
the stable full-history request is retained pending an explicit compatibility
and counting strategy.

## Byte-budget inventory and remaining profiling

Renderer compaction does not bound the entire application. Current additional
retention surfaces include:

| Surface | Existing bound | Remaining work |
| --- | --- | --- |
| `interface-runtime/src/history.rs` | Process-local projection with 4 MiB/session and 32 MiB total disposable-content budgets; closed streams release content | Approval, lifecycle and effect reserves remain outside the payload budget. See [September 14 follow-up](PERFORMANCE_REVIEW_2026-09-14_FIXES.md). |
| `interface-engine/src/event_stream.rs` | Caller-supplied event count; oldest event dropped on overflow | A byte policy needs event semantics and must preserve the frozen nonblocking provider behavior. |
| Codex/ACP supervised transport | 512 inbound and 64 outbound entries, normally a 4 MiB frame cap | Count and frame limits do not provide a small aggregate heap budget. Profile decoded payloads and backpressure before changing policy. |
| Claude transport | 5,000 event entries, 128 writer entries, 8 MiB protocol frame cap | Profile large decoded messages and reliable control events before introducing byte eviction. |
| Image paste | Existing per-item, batch and wire bounds | Measure base64/JSON copy costs; binary transport would require a versioned cross-language change. |

Focus-triggered provider probes, TUI transcript revision granularity and the
deliberate single renderer bundle remain measurement candidates, rather than
claimed regressions fixed by this patch. Full-window DOM virtualization,
conversion-buffer pooling and a central terminal scheduler were not added
without the ownership and runtime evidence their designs require.

## Validation and release boundary

The implementation branch passed `cargo xtask check` across both Rust and
Electron in 90.3 seconds: 1,423 Rust tests passed (18 intentionally ignored),
2,649 Vitest tests across 136 files passed, and the three Swift tests, release
script tests, isolated renderer lane and build passed. Targeted
checks cover the new cache, retention, deadlines, recovery concurrency and
test-discovery behavior. The landing records the integrated source receipts
and Dev readiness separately.

Integration initially hit the renderer's 60-second process deadline after its
material screenshots. A diagnostic run completed the same assertions in
57.7 seconds, confirming inadequate margin as fixture coverage grew. Interaction
and visual checks now have separate processes, each retaining the original
deadline and all assertions; their output is combined in the same final report.

A later gate exposed an immediate post-drop assertion racing an in-flight
terminal lookup's last `Shared` reference. The regression now deliberately
holds that lookup open across service drop, releases it, waits for bounded
capture retirement, then checks the complete saved tuple and checkpoint. This
tests the asynchronous cleanup boundary already exercised by the neighboring
provider-poller lifetime test, without changing production teardown semantics.

Live-provider, microphone/Instruments, packaged/signing/updater and installed
production gates are not covered by these fixture results. Main promotion,
feed staging and production installation are separate owner-requested actions.
