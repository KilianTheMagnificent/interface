# September 14 performance review fixes

Review target: local Dev `cbd53ab`. Implementation starts from production main
`8711012` so selective release can exclude unrelated unshipped Dev landings.
Seven fixes were published and feed-verified in 0.3.117 (`1631fdc`). The owner
then approved lossless audit v2, implemented separately on that main revision.

## Changes and boundaries

| Finding | Result |
| --- | --- |
| R1: fragmented host NDJSON | Byte-oriented accumulator searches each incoming fragment once, grows geometrically, enforces the existing per-frame byte limit, and decodes UTF-8 only for a completed line. Large buffers are released after delivery. |
| R2: worker capture blocks other owners | At most two filesystem captures run outside the shared mutex. Durable state/archive operations run on serialized blocking tasks. Admission rechecks idempotency, capacity, parent state, shutdown and grant after capture. Durable admission owns scheduling even if its caller disconnects. |
| R3: closed runtime transcript retention | Disposable content defaults to 4 MiB/session and 32 MiB total, compacted in batches; closed streams release content. Approval requests, current control facts, tombstones and effect certainty remain. |
| R4: malformed Markdown rescans | Inline delimiter searches share a linear work budget, including nested formatting and directives. When exhausted, remaining source stays literal text. Ordinary annotation/link/formatting behavior is retained. |
| R5: disabled delegation polling | Unavailable capability stops its timer. Hidden documents suspend reads. Generation/visibility changes permit fresh reads; identical immutable views retain React state identity. |
| R6: growing Claude listing | Cache preserves summary fields at a complete-record offset. Growth verifies the entire old prefix, then parses only new records. Rewrite, truncation, replacement, unsafe metadata and partial EOF remain handled. |
| R8: duplicate menu focus | Workspace overflow uses the shared menu focus implementation. Initial focus stops after success or a newer user focus choice; keyboard/resize/Escape behavior remains tested. |

The runtime budget counts serialized disposable event content without allocating
another JSON string. It is not an allocator/RSS limit. Approval inputs and control
reserves may exceed it; session tombstones and uncertainty are deliberately kept.
`omittedEventsThrough` records the greatest omitted sequence while older reserved
control events may remain. Provider transcript stores remain authoritative. Frozen
history/event JSON schemas and host protocol versions are unchanged.

Worker state writes still serialize under one mutex to retain durable ownership.
This removes capture contention and blocking I/O from the asynchronous executor;
it does not make filesystem durability instantaneous or concurrent.

## Synthetic measurements

Measurements use synthetic local files and protocol payloads, without contacting
providers. Timings are machine/build specific, not installed-app latency claims.
Original review artifacts are in `/private/tmp/interface-code-review-20260914`;
implementation harnesses/results are in
`/private/tmp/interface-review-performance-evidence`.

| Fixture | Before | After |
| --- | ---: | ---: |
| 16 MiB host frame, 4 KiB fragments; Node 26.7 median | 3,134.6 ms | 7.95 ms |
| 64 KiB unclosed Markdown directive; Node median | 228.9 ms | 1.38 ms |
| Four closed sessions, each 1,024 × 16 KiB deltas | 64 MiB retained text | 0 retained text bytes; four tombstones |
| Cancel owner A while owner B captures for 500 ms; unoptimized Rust | 517 ms | 8.36 ms |
| Claude 10.7 MiB assistant transcript, append one 1,070-byte row; optimized Rust, nine trials | 5.58 ms median | 3.01 ms median |
| Same Claude transcript, cold listing | 5.67 ms median | 7.53 ms median |

The Claude integrity fingerprint adds cold-read cost. Repeated appends repay it;
unchanged listing stays near 0.1 ms. Debug builds did not improve the mostly
assistant-record append fixture (14.28 ms before, 16.76 ms after). This change
reduces repeated JSON/line interpretation; it does not eliminate old-prefix I/O.

## Validation

Focused checks passed: 21 runtime library tests, 77 Claude library tests (one
manual benchmark ignored), 15 worker tests, 39 tests across host framing/client
and Markdown suites, Electron typechecking, and the real Electron renderer suite.
New cases exercise Unicode splits/frame bounds, capture contention/concurrent
idempotency/revocation/shutdown, retention certainty, incremental partial EOF and
rewrite handling, unavailable polling, stable React state, and menu focus.

The full `cargo xtask check`, integrated Dev gate/refresh, selective promotion,
signed package and packaged smoke are release gates; consult release receipts
for their eventual outcome. No live provider call or production install/restart
is part of this task.

## R7: approved lossless audit v2

The owner explicitly approved preserving all detailed events in numbered sealed
segments, with growing disk usage and the v2-aware-reader/full-export downgrade
requirement. [Audit storage v2](AUDIT_STORAGE_V2.md) records the implemented format
and recovery contract. The original v1 log remains intact, lifetime counters
survive rotation, and each steady append replaces at most 256 KiB of event data.
The earlier two-slot retention proposal was not selected.

0.3.117 remains the published seven-fix candidate; R7 is a separate subsequent
release. Migration and performance tests use synthetic stores. Live provider
execution and production installation/restart are outside this task.

### R7 validation and measurements

Ten v2 storage tests and five existing audit tests passed (one manual benchmark
ignored). `cargo xtask check` passed on the main-based candidate: all Rust/Swift
checks, 2,707 JavaScript tests, real renderer fixtures and builds. Logs:
`/private/tmp/interface-review-audit-v2-final-tests.log` and
`/private/tmp/interface-review-audit-full-check.log`.

The final synthetic debug-build fixture measured steady append medians of
8.00/8.01/8.01 ms with 23,482/2,387,784/12,027,784-byte legacy logs; the review's
v1 values were approximately 8/23/79 ms. At 12 MB, one-time migration took
279 ms, initial v1 open 971 ms and v2 reopen 254 ms. Reopen still hashes the
preserved legacy file. Eight new events installed 9,652 bytes across the active
segment replacements and checkpoint; the legacy file was byte-identical and
reopened lifetime counters included all eight events. This counts installed
file content, excluding filesystem metadata/journaling and readback I/O.
Harness and exact results: `audit-final.rs` and `audit-final-results.jsonl` in
`/private/tmp/interface-review-performance-evidence/`.

## Release status — complete

The owner approved and published the releases in sequence:

- **0.3.117**, main `1631fdc`: seven non-audit fixes, signed package and feed verified.
- **0.3.118**, main `f8467b4`: 0.3.117 plus lossless audit v2; full source receipts,
  signed app/extracted ZIP, Electron fuses and packaged renderer/Rust-host smoke
  passed. The current updater feed matches these archived package bytes.
- **Dev:** all eight fixes landed; audit v2 was verified running at `cca5f93` with
  a real renderer → preload → Rust-host read. Publication merged into Dev `36d11b0`.
- **Installed production:** freshly verified 0.3.115. No install/restart occurred.

The exact 0.3.118 approval resolved the automatic publication rejection. Unrelated
Dev landings and the separate Away-only 0.3.116 selection remain excluded.
Notarization is disabled by the existing package configuration. Live-provider,
physical power-loss and installed-runtime acceptance tests were not run.

Final evidence: `/private/tmp/interface-review-performance-evidence/status-final-staged.json`,
`installed-final.json`, and `/private/tmp/interface-review-audit-promotion-publish.log`.
The [to-do entry](TODO.md#performance-review-and-staging-scope) marks the review,
implementation and staging complete; production installation remains separate.
