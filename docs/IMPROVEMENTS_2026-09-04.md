# Review implementation — September 4, 2026

Implemented the concrete changes across all twelve review areas. Conditional
redesigns remain conditional: the measurements and limits below distinguish
implemented code from follow-up work. No dependency was added.

## Changes

1. Transcript projection retains unchanged row and nested subagent identities.
   Row comparison selects relevant approval, restore, find and subagent state;
   callback freshness is independent of identity. The original reducer remains
   the reference implementation; no geometry-sensitive windowing was added.
2. Session/title position indexes and forgotten membership remove repeated
   linear lookups while preserving ordered persistence and merge semantics.
3. Root discovery has a 256-root queue, four concurrent Git children, deduplication,
   batched registry updates and expiring negative results. Late Git results
   cannot overwrite provider/durable roots; uncached roots precede expired misses.
4. Terminal replay uses a head-indexed deque with occasional compaction, bounded
   to 1 MiB and 4,096 chunks, including escape/UTF-8 boundary state.
5. Search excludes `target` and `.worktrees`, and bounds enumeration itself.
   Truncated directories expose a sorted filesystem-selected subset, not a
   globally sorted complete list; incompleteness is reported.
6. Orchestration pages/snapshots premeasure item contributions and adjust exact
   envelope/cursor/counter sizes before one final serialization. Reference
   tests retain removal order and byte-for-byte results at budget boundaries.
7. Voice scans each delta once, caps unpunctuated sentences at 16 KiB, bounds
   helper framing at 1 MiB, drains stderr in fixed chunks, and bounds Swift
   speech backlog to 128 entries/256 KiB. Overflow rejects new speech explicitly;
   accepted order and cancellation completion events are preserved.
8. Codex session listing reuses one supervised read-only connection, not cached
   results. Failure, cancellation and exit discard it; mutations stay isolated.
   TUI paints cached state before independent, identity-fenced discovery;
   explicit resume/continue still await fresh sessions. Existing probe freshness
   and Claude store caches were retained rather than duplicated.
9. Terminal data protocol v2 adds generation-scoped completion credits across
   Rust, main, preload, MessagePort and xterm. A live output window is capped at
   256 KiB/64 frames; one saturated pane does not block sibling input. See
   [the contract](TERMINAL_DATA_V2.md). Replay/startup retention has its own
   separate bounded budget; credits do not claim to bound all application heap.
10. Extracted transcript projection and TUI drawing; injected a typed terminal
    API into the terminal center. Conversation, Composer and FormSelect styles
    load once. Removed overridden Composer declarations while preserving theme
    variants; the narrow-pane fixture also exposed and fixed a clipped launch
    button by allowing its control row to wrap.
11. Typed desktop effect certainty replaces error-message substring matching.
    Public codes, messages, schemas and replay-safety outcomes are retained.
12. Added the fast isolated Electron DOM gate; replaced touched behavioral source
    pins with boundary/DOM checks where practical and retained wire/smoke pins.
    The canonical gate reports both Cargo and npm failures even when one fails.
    Swift buffer tests now run in the gate.

Also: collaboration audit counters are rebuilt once at open and maintained
after successful writes. Shutdown stages report duration and warn after five
seconds without cancelling persistence or replaying provider work.

## Local measurements

Synthetic debug fixtures, not production latency or heap-saving claims. Baseline
source was pinned at `21ff11a8f925a410a1277021d8dc0290e3ab9212`; machine load and
compiler profiles affect absolute timings.

| Fixture | Before | After |
| --- | --- | --- |
| Warm registry merge, 500 / 2,500 / 10,000 sessions, median five | 3.08 / 51.12 / 804.85 ms | 0.31 / 1.68 / 6.81 ms |
| Voice, 1,000 / 2,000 / 4,000 unpunctuated 16-character deltas | 123 / 486 / 1,893 ms | 0.29 / 0.46 / 0.99 ms |
| 100-record escaped near-capacity orchestration page, same output | 896.8 ms reference | 29.1 ms; 518,234 bytes retained |
| Ten fresh Codex listings, fake 50 ms initialization | 10 children; 904 ms | 1 child; 111 ms |
| Twenty audit snapshots after 2,000 events | 594 ms | 9.9 microseconds |
| Streaming transcript, 1,001 rows, ten tail updates | No prior projected item identities retained | One row rerendered in the DOM fixture |
| 2 MiB output, slow Electron consumer | Bare xterm enqueue reached 2 MiB pending | Real preload/MessagePort path peaked at 260,885 bytes; sibling input 6–9 ms |

The terminal fixture checks byte checksum, order, EOF and complete drain. Its
producer is synthetic; separate Rust duplex tests exercise real credit waiting,
ACK routing, sibling input and shutdown. The integrated terminal smoke uses the
actual Rust host and PTYs with fake providers, including restart and forced close.

## Verification

- Full `cargo xtask check`: Rust format/Clippy/workspace tests, Swift build and
  two buffer tests, 2,622 Vitest tests, release-contract tests, TypeScript,
  isolated Electron DOM tests and production build.
- Provider listing: fresh reads, child restart, cancelled/failed requests,
  mutation isolation and disposal; real-PTY TUI first paint with a slow provider.
- Visual fixtures: light/dark × CLI/refined × 640/1,100-pixel windows, two panes,
  long labels, prompt keyboard focus and reduced motion. Representative images
  inspected; this is not a claim of pixel-identical packaged parity.
- Root concurrency/negative-cache precedence, mixed registry lifecycle/reopen,
  orchestration reference equivalence, replay boundaries, wide directories and
  existing symlink/sensitive-path tests.
- Unix-socket/PTY and Swift-cache tests require local access unavailable in the
  restricted sandbox; the identical gates passed with that access enabled.

No live-provider conversation, microphone/speaker interaction, signed packaged
smoke, updater staging or production installation was performed for this work.
Dev closeout/runtime status is reported separately in the handoff.

## Profile-first follow-ups deliberately not applied

- **Capture scheduler:** 32 fake-provider panes used 159 host threads and about
  10 ms aggregate host CPU over five idle seconds after the initial burst
  (coarse `ps` measurement). Capture is only part of that thread count. Keep the
  authoritative hook/poll fallback; filesystem-read and lock-contention profiling
  on real sessions is still needed before a shared scheduler change.
- **Review patch batching:** this 91-file working-tree review took 73 Git children
  and 867 ms versus four children/64 ms without patches. This remains a promising
  follow-up, but batching needs a separate bounded parser/design preserving
  credential exclusions, rename pairing, per-file attribution and byte caps.
- **Cold surfaces:** the production JavaScript asset is 1,571,331 bytes after
  stylesheet extraction. A disposable network-blocked, no-host renderer settled
  in 377/134/142 ms with about 5.5 MB JS heap; only the first sample reported
  contentful paint (324 ms). These fallback-shell samples do not justify lazy
  loading live surfaces or remounting terminal buffers.
- **Audit segmentation:** read-side decode was removed; atomic whole-file writes,
  format and poison semantics remain. Rotation/append durability requires an
  explicit recovery design rather than silently changing the canonical store.
- **Shutdown deadlines:** the 32-pane fixture closed/dismissed terminals first,
  then host shutdown took about 3 ms. Slow-stage instrumentation is in place;
  no unconditional timeout was added around effects with unknown outcomes.
