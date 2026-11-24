# Terminal responsiveness and idle work — 2026-09-11

This pass audited the current Dev architecture and implemented confirmed hot-path
improvements without changing wire formats, persisted stores, provider effect
semantics, terminal credits, or the settled visual design. The implementation
started from `0b996a35f4eefb6afc7e56de5e225d473dea2cac` in an isolated worktree;
the owner's unrelated checkout changes were preserved.

## Implemented changes

| Path | Problem and resulting behavior | Evidence |
| --- | --- | --- |
| Renderer terminal activity | Alternating text and ANSI repeatedly searched the remaining chunk for an absent BEL; bell-heavy output did the reverse. Each delimiter now advances independently, making search work linear in chunk size. | Deterministic scan-budget tests and split-boundary parser checks; before/after benchmark below. |
| Terminal presentation | Every parsed output burst advanced React state, rerendering the terminal/conversation to tell a periodic reader that output existed. A mutable read cursor now supplies that information without a render. Hidden and native-only panes skip presentation scheduling. | Four real xterm panes: 20 unchanged redraw frames produce zero terminal React commits in all eight fixture configurations. |
| Terminal settings | Identical acknowledged emulator snapshots repeatedly crossed IPC; identical result DTOs replaced state. A presentation memo now skips those reads and retains equal state. Input and binding/visibility changes invalidate the memo; choices, pending transactions and their timeout checks still advance Rust. | Same fixture: zero settings reads for those 20 frames; settings choice/error/cancel, native and GUI drafts, Send/Stop, images, and stale-binding checks pass. In-flight invalidation has a generation test. |
| Agent workspace polling | An unchanged five-second read replaced the full workspace DTO; slow reads could overlap and hidden windows kept reading. Reads now serialize, skip hidden windows, refresh on visibility, and retain equal DTO identity. | Full Electron typecheck, unit and interaction suites. |
| Standalone TUI | Any controller revision, including typing or another session's metadata, invalidated formatted transcript layout. Cache identity now follows the actual transcript and width. | Tests verify unrelated updates reuse layout, in-place deltas invalidate it, session changes stay correct, and weak cache handles do not force a deep clone of transcript cells. Real PTY suite passes. |
| Provider startup/readiness | Simultaneous readiness and catalogue calls could all miss the successful-probe cache and launch duplicate probes. A per-provider mutex now shares that successful result. | Sixteen concurrent readiness calls plus one catalogue read execute one probe. Existing failure/retry behavior remains covered. |
| Rust PTY title/private-frame parsing | Every ordinary output byte went through the full state machine and individual output push. Plain runs now skip/copy in bulk, and animated titles reuse the bounded payload allocation. | 24 OSC tests including every-split private-frame byte preservation and bounded allocation reuse; optimized Rust microbenchmark below. |

## Measurements

The renderer benchmark times one `feedTerminalActivity` call on a fresh monitor,
excluding monitor construction. Inputs are 4, 16 and 64 KiB of plain text,
alternating `x`/ANSI reset, or alternating `x`/BEL. These deliberately dense cases
expose algorithmic behavior; they are not a claim that the whole app is hundreds
of times faster. No wall-clock threshold is embedded in the correctness tests.

Run the checked-in benchmark from `electron/`:

```sh
node scripts/benchmark-terminal-activity.mjs 0b996a3
```

It uses the installed Vite transformer directly, opens no server, reads only the
two named source files from the optional baseline, and prints JSON lines with
the Node version and timing method. Node 26.7.0 results, median of 20 measurements
after five warm-ups (milliseconds per chunk):

| Input | Size | Before | After |
| --- | ---: | ---: | ---: |
| Plain text | 4 KiB | 0.011 | 0.010 |
| Plain text | 16 KiB | 0.043 | 0.042 |
| Plain text | 64 KiB | 0.145 | 0.162 |
| ANSI alternating with text | 4 KiB | 2.160 | 0.046 |
| ANSI alternating with text | 16 KiB | 33.275 | 0.144 |
| ANSI alternating with text | 64 KiB | 539.411 | 0.598 |
| BEL alternating with text | 4 KiB | 5.428 | 0.058 |
| BEL alternating with text | 16 KiB | 85.390 | 0.199 |
| BEL alternating with text | 64 KiB | 1,352.271 | 0.848 |

The plain-text path remains sub-millisecond with small absolute variation; the
gain is the removal of the quadratic case, not a general plain-text speedup.
Raw results: `/private/tmp/interface-activity-benchmark-20260911.jsonl`.

An additional optimized Rust comparison exercised the private OSC filter
on 200 chunks per round, discarding two warm-up rounds and taking
the upper median of ten measured rounds. Plain chunks were 65,536 bytes; ANSI chunks
65,530 bytes. Plain input improved from **15.168 to 3.542 ms** per round; ANSI
input from **26.729 to 17.909 ms**. These local parser timings exclude provider,
IPC, filesystem and GUI work.

## Audit coverage and remaining limits

| Area inspected | Outcome |
| --- | --- |
| React shell, terminal panes, conversation/history projection, Clean presentation, activity, polling and disposal | Changes above. Existing incremental conversation rows, bounded live events, scoped history projection and replay recovery remain in place. |
| Electron lifecycle, preload, host client, binary PTY transport and output credits | Existing generation fences, bounded queues, fair scheduling and acknowledgements preserved. No protocol changes or weakened input/output checks. |
| Rust terminal parsing, launch/lifecycle and provider adapters | Faster title/private-frame scanning and shared readiness probes. Existing bounded subprocess output, history/listing caches and limited parallel readers preserved. |
| Runtime, orchestration, persistence and audit | Existing effect certainty, idempotency, writer locks and durable recovery preserved. Audit snapshot rewrites remain a known architectural cost; changing retention/recovery is a separate decision, documented in `AUDIT_STORAGE_V2_PROPOSAL.md`. |
| CLI/TUI | Transcript layout now ignores unrelated state updates. Startup, terminal teardown and signal behavior remain covered by real PTY tests. |
| Workspace/files, search, updater, usage and voice boundaries | Existing bounded searches, cache freshness, file identity checks, updater integrity and cancellation guards retained. No new measured bottleneck established here. |

A live process snapshot during this investigation showed the production renderer
at roughly 53% CPU, the GPU helper at 13%, and the Rust host at 2%; fileproviderd
and WindowServer were also busy. This guided attention toward the rendering path
but does not attribute all observed CPU to a single function. A short native
renderer sample lacked useful JS symbols. This work does not claim a matched
before/after production CPU or battery measurement.

Further large-history work needs representative profiling and explicit semantics
for any new byte limits or storage migration. Long-term CPU/memory behavior across
real provider sessions, provider latency, and iCloud contention are not exhausted
by deterministic fixtures. This is a completed implementation of the confirmed
gains in this pass, not a claim that every possible optimization is proven.

## Validation and release boundary

`cargo xtask check` passed on 2026-09-11: formatting, Clippy, **1,447 Rust tests**
(18 ignored), **138 Electron test files / 2,663 tests**, release-script tests,
typechecks, real Electron renderer interaction fixtures, Swift helper build/tests,
and the application build. The four-pane redraw assertions passed in dark/light
and CLI/refined appearances with no remounts or disposals.

Canonical gate log: `/private/tmp/interface-perf-gate-20260911.log`.
Renderer artifacts:
`/var/folders/_b/hwd9v4ks1gb8dcf1jl66s8xc0000gn/T/interface-renderer-fixtures-W93EGv`.
The final closeout validates the integrated source again, reusing only matching
suite receipts. Consult `cargo xtask status --fetch --json` and
`cargo xtask landings` for the landing and running Dev identity.

Live-provider sessions, a signed production package, updater staging, production
installation/restart, and a matched production workload measurement were not run
as part of this task. Dev landing and its renderer → preload → Rust host readback
are reported separately by closeout.
