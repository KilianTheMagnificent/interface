# Performance audit implementation — September 24, 2026

Implements the seven findings from the September 23 audit, based on Dev
`0b53abb7a78e88934dc852a8749c9962fe4400e5`, including its workspace-switch GPU
pooling fix. Work happened in branch `improve/performance-audit-20260924`, in an
isolated worktree. The owner's older dirty main checkout was preserved.
Closeout incorporated concurrent Dev `c5e6ff7` (moved-folder recovery). Its v27
contract remains intact; this change advances the combined host to v28.

## Changes

| Finding | Implemented behavior | Main evidence |
| --- | --- | --- |
| F1 Away persistence | One serialized blocking worker operation per store transition; one JSON encoding, strict validation, unchanged atomic lease/CAS/fsync checks. Unchanged presets skip writes. Cancelled start callers cannot orphan admission; Stop signals before disk work, and shutdown drains tracked admission/drivers/writes. | Storage target-drift/bounds tests; Away cancellation, slow-storage, caller-abort, shutdown, uncertainty and recovery tests. |
| F2 Remote results | Single-flight reads, hidden-page pause, 10-second stale-response deadline, exact terminal/generation identity, disposal abort/response fence. A native request keeps its slot until it actually settles. | Deferred native replies, deadline, visibility, generation replacement and failure behavior tests. |
| F3 Away polling | Additive status v2 contains revisions, active identities and all start/cancellation receipts, without reports. Closed idle controls poll every 15 seconds; open/active/pending controls every 2 seconds. Opening refreshes reports immediately. Equal revisions retain view identity. | Shared Rust/TS golden fixture, full native host fixture, pending-receipt and hidden renderer tests. |
| F4 Event queues | Explicit policy v2: 8 MiB provider and 4 MiB subscriber payload bounds; one ordered omission notice per backlog. Controls are never silently evicted; control exhaustion explicitly closes the stream. Runtime ingress has a 32 MiB weighted bound; supervised framed channels clamp against 64 MiB encoded payload. Away's listener has a 4 MiB bound and treats omissions as unknown. | Large mixed bursts, control ordering and overflow, ingress permit lifetime, oversized ingress, subscriber sequence and terminal receipt, Away omission tests. |
| F5 Grok names | Root-scoped three-second miss caching; directory, prompt, summary and miss maps each capped at 128 entries. Generated names avoid unnecessary prompt reads. Existing containment checks remain. | Missing-to-created transitions, root changes, renamed summaries, symlink rejection and eviction. |
| F6 Controller boundaries | Extracted renderer collaboration requests/receipts, terminal activity/timers and focus guard; host terminal lifecycle transactions and collaboration projection; TUI agent socket implementation. Existing authority, state owners, public Rust imports and mounted terminal pool remain. | Controller tests plus existing host, collaboration, PTY and hidden renderer suites. |
| F7 Behavioral tests | Replaced the affected terminal focus and activity source-string assertions with generation/lifetime/focus behavior tests. Contract, package and settled-look pins remain. | Tests exercise delayed callbacks, disposed views, hidden workspaces, batched completion and timer cleanup. |

The structural work is one coherent extraction per audited area. It does not
redistribute every callback or merge provider-specific approval semantics into a
generic adapter. No runtime speedup is claimed for moving code. No dependency,
visual design or persisted schema changed. Host protocol is deliberately v28;
existing Away requests and snapshots remain v1, the new read-only request is v2.

## Reproduced measurements

Optimized release builds on this Mac, synthetic temporary stores, four settings
save samples per row. Each completed run has 16 turns with 8 KiB reports. These
are local workload measurements, not p95 latency or whole-app CPU/RSS results.

| Completed runs | Store bytes | Before median save | After median save | Before 5 ms timer | After 5 ms timer | Full inspect bytes | Compact status bytes |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 0 | 447 | 8.99 ms | 8.02 ms | 11.24 ms | 6.80 ms | 516 | 100 |
| 32 | 4,293,975 | 39.06 ms | 36.89 ms | 38.88 ms | 6.77 ms | 2,171,481 | 441 |
| 128 | 17,174,611 | 129.61 ms | 123.65 ms | 129.01 ms | 6.61 ms | 2,442,891 | 1,525 |

The large-store timer delay falls by about 95%; full-store writes still cost
roughly 124 ms, now off the async worker. The large closed-panel payload falls by
99.94%, before considering the slower idle cadence. Detailed inspect payloads
remain available and unchanged in size. Identical settings writes are skipped.

Claude transcript prefix verification is retained. Its existing per-path/limit
cache already serializes concurrent readers. The release benchmark for ten
reads of 10,000 records (10,650,000 bytes) measured 57.66 ms for fresh folding,
1.04 ms unchanged, and 30.32 ms while appending. It also checks the final cached
result against a fresh fold. Rewrites/replacements still invalidate correctly;
no weaker mtime-only shortcut was introduced.

The queue burst test feeds 64 MiB through a 128 KiB test budget, verifies retained
payload stays bounded, preserves approvals in order, and observes the omission
notice. Production payload budgets include conservative event accounting;
allocator overhead and whole-process RSS are not claimed to equal those bounds.

Reproduce the informational measurements:

```sh
cargo test --release -p interface-collaboration benchmark_away_snapshot_cost -- --ignored --nocapture
cargo test --release -p engine-claude benchmark_unchanged_and_growing_history -- --ignored --nocapture
```

## Verification and release state

`cargo xtask check` passed on the stable source: 1,932 Rust tests (24 existing
or manual tests ignored), 2,800 JavaScript tests, three Swift helper tests,
all default hidden renderer lanes, and desktop/remote builds. This includes
seven real-PTY TUI tests and the native Away host fixture. Formatting, strict
Clippy and the checked navigation map passed. The two release benchmarks above
were run explicitly despite being ignored by the ordinary gate.

The additional terminal-preservation lane passed six workspace switches,
scrollback beyond both replay limits, split parser state, typing/paste, hidden
credits, generation replacement and dismissal cleanup. Warm four-pane workspace
switches performed no GPU activation/disposal; the 20/40-pane cases retained the
12-context bound. The separate disposable-host remote smoke passed shared draft
CAS, result generation fencing, native approval, reconnect, restart, idempotency,
revocation and shutdown checks.

The integrated v28 gate, terminal-preservation lane and remote smoke all passed.
Closeout landed **Performance audit fixes and controller cleanup** on Dev as
`c5e6ff7..e13f03d` (`e13f03dbd0b8cb43e55c45865728550073239c20`). It reused
matching Rust/Electron source receipts and refreshed the Dev app. The renderer
→ preload → Rust-host readiness read passed on `e13f03d`; a subsequent fetched
status confirmed `runningVerifiedRevision: true` and both source checks green.

No implementation work remains. Main promotion, package/signing, feed
publication and production installation/restart were not performed for this
change. The existing main/feed remain at production 0.3.132.

Local evidence logs are `/private/tmp/interface-performance-v28-gate.log`,
`/private/tmp/interface-performance-v28-preservation.log`,
`/private/tmp/interface-performance-v28-remote.log`,
`/private/tmp/interface-performance-closeout-final.log`, and
`/private/tmp/interface-performance-status.json`.

Live provider accounts, physical iPhone/network behavior, microphone/speakers,
production packaging/signing/updater publication and installed production runtime
are separate gates and are not established by these synthetic tests.
