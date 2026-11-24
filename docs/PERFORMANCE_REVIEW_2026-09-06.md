# Performance and implementation review — 2026-09-06

## Scope and method

Reviewed the current `dev` baseline `5e88f9c540b557c81c65163c6ef3246432c62a60`.
The scope contains 496 tracked Rust, TypeScript and Swift source/test files.
This was an architecture and risk-based code review, with detailed inspection
of the paths below; it was not a claim to inspect every line of every file.

| Area | Paths and questions examined |
| --- | --- |
| Runtime and providers | History retention, provider refresh/discovery concurrency, effect certainty, shared and Claude-specific framing, adapter/store authority. |
| Terminals and host | PTY flow control, binary decoder, fair queues, output tails, foreground/session polling, bounded process inspection, host supervision and logging. |
| Persistence and workspace | Atomic writes and lease/identity checks, registry indexes, capped Git/review output, search walk budgets, collaboration audit persistence. |
| Renderer | Event batching/compaction, conversation projection, resize scheduling, workspace persistence ordering, lifecycle cleanup and retained buffers. |
| CLI and orchestration | TUI redraw invalidation, history/tail projection, startup discovery, shutdown/effect boundaries and event-sourced persistence invariants. |
| Build and release | Deterministic gate, Rust/JS scoping, package gate stamps, named Dev landing and promotion boundaries. |

The previous performance audit and current area docs helped avoid repeating
already-landed work. Runtime history already uses a deque; provider reads have
blocking-worker boundaries; renderer events coalesce; registry lookups have
private indexes; the TUI redraws on invalidation. These were retained.

## Findings and implemented modifications

The implementation list was presented before editing, then extended with the
bounded host-log finding during verification. No dependencies, wire versions,
persisted formats, provider retry policies or settled visual decisions changed.

| Priority | Finding and original location | Modification and regression coverage |
| --- | --- | --- |
| P1 | `interface-review/src/process.rs::run_bounded_process` and `interface-registry/src/worktree.rs::run_bounded_command` stopped timing once the leader exited, then unconditionally joined stdout/stderr readers. A descendant inheriting either pipe defeated the timeout. | Spawn a private process group, keep checking the deadline until both pipes finish, stop the owned group on timeout/overflow/failure, and reap away from the caller. `CapturedPipe` returns bounded captured output without joining an unfinished reader. Tests cover inherited stdout/stderr, group cleanup, direct timeout, output caps and normal exit status. |
| P2 | Both runners silently converted a completed pipe-read error into empty successful output. | Completed capture errors now produce the existing failure result rather than a successful empty review/Git result. |
| P2 | `terminal-data-transport.ts::TerminalDataDecoder.push` concatenated all accumulated bytes for every fragment. | Validate the header before allocation; copy fragmented input into a single frame-sized buffer and decode whole frames directly. Payloads still own their storage. Tests cover one-byte through whole-frame input, maximum payloads, empty input, EOF, oversized fragmented headers and existing sequence/ACK rules. |
| P2 | `FairTerminalFrameQueue.pop` shifted the frame array on every dequeue. A 1 MiB lane admits over 20,000 empty frames. | Head-indexed queues reclaim consumed references immediately and compact amortized over consumed entries. Round-robin scheduling, byte accounting and refusal semantics remain intact. A deep drain/refill test exercises FIFO order, fairness, compaction, reuse and clear. |
| P2 | `engine-claude/src/protocol.rs::NdjsonDecoder` retained a large fragmented frame's allocation until the session ended. | After successful decode, cap the retained scratch allocation at 64 KiB, matching the shared decoder's policy. A large-frame test verifies reclamation and subsequent fragmented CRLF decoding. |
| P2 | `interface-voice/src/tail.rs::tail_lines` allocated a string for every retained line, then discarded all but the requested suffix. | Retain at most the requested number of lines and reuse evicted line allocations; zero-line reads return immediately. Scan the full bounded text to preserve ANSI/OSC and carriage-return semantics. Tests cover those controls, trailing blanks, extreme limits and a 100,000-line source. |
| P2 | `terminal-resize-queue.ts::flush` set `inFlight` before invoking a sender that could throw synchronously. A throw bypassed error handling and settlement. | Route synchronous throws through the same promise settlement path as asynchronous rejections. The queue recovers and accepts a later resize; cancellation/serialization tests remain green. |
| P2 | `RustHostClient.start` accumulated an unterminated stderr line without a limit. The 2,000-character display limit only applied after a newline. | `HostLogBuffer` retains only the first 2,000 characters while consuming every fragment. A test exercises split log levels, a 200,000-character line, quiet logs and the following line boundary. |
| P3 | `interface-common/src/process.rs::read_capped` propagated `Interrupted` rather than retrying the transient read. | Retry only `Interrupted`; retain output caps and propagate other errors. A reader fixture alternates interruptions and successful reads across the cap. |
| P3 | Architecture docs still described NDJSON v5 and conflated EOF retention with input-generation retirement. | Document the implemented v6 control protocol, the distinct 32-terminal and 1,024-EOF bounds, bounded host logging and inherited-pipe deadlines. |

`P1` is a hang/correctness issue; `P2` denotes measurable hot-path work,
unnecessary retained memory or recoverability; `P3` denotes smaller hardening
and maintenance corrections. Rankings are review judgments, not production
incident statistics.

## Local measurements

On macOS arm64 with Node v26.7.0, TypeScript sources were compiled to ES2022
without bundling. Before/after inputs and correctness checks were identical.
The table reports the median of seven alternating runs after warming both
implementations. Queue construction was outside the timed drain.

| Workload | Before | After |
| --- | ---: | ---: |
| Decode 32 × 64 KiB frames, 64-byte fragments | 29.44 ms | 3.07 ms |
| Decode 8 × 8 KiB frames, one-byte fragments | 18.60 ms | 5.35 ms |
| Decode 256 complete 64 KiB frames | 1.01 ms | 1.04 ms |
| Drain 20,000 empty frames from one lane | 20.15 ms | 0.49 ms |

A standalone reproduction compiled the original and changed Rust review
runners with the same 100 ms timeout. The shell printed `ready`, started a
two-second background sleep inheriting the output pipes, then exited:

- Original: 2,008 ms, `timed_out=false`.
- Changed: 104 ms, `timed_out=true`; the `ready` prefix was preserved.

These are synthetic mechanism checks, not measurements of total application
latency, idle CPU, or battery life. Small differences in complete-frame timing
are within ordinary local timing noise. No speedup claim is made for that case.

## Retained tradeoffs and follow-up work

- Conversation projection still replays the bounded event list and performs
  identity comparisons. Incremental projection or row virtualization should
  start with long-conversation UI profiles and a design preserving approval,
  echo, subagent, find and scroll behavior. It was not folded into this patch.
- Collaboration audit appends still atomically replace the full JSONL file,
  and record validation remains repeated at trust boundaries. Segmentation
  needs an explicit recovery/migration design; removing validation/fsync or
  changing journal writes as a micro-optimization would weaken guarantees.
- Terminal provider polling remains a potential idle-cost area. Receipts,
  process identities and generation fences are restoration authority. Moving
  to notifications alone would need a missed-notification recovery design and
  live-provider evidence.
- The process fix bounds pipe collection and avoids synchronous reaping after
  termination. It does not claim a hard real-time bound on OS process creation
  or signalling. A descendant that deliberately leaves the owned group can
  keep a detached pipe reader alive until it closes its descriptors; callers
  still return within their collection deadline and retained bytes are capped.

## Verification and delivery

Focused checks passed for transport/resize (28 tests), host supervision/logging
(16 tests), Claude decoding (69 tests), voice (13 tests), shared process helpers
(12 tests), registry (27 tests) and review (5 tests). These counts describe the
focused suites, not the final whole-workspace gate.

Full `cargo xtask check` passed after integrating the concurrent sidebar
landing `2c03be3`. It covered Rust formatting, workspace Clippy/tests, two Swift
helper tests, 113 Electron test files / 2,537 tests, release-script checks,
TypeScript checks, real Electron/xterm DOM fixtures and the production build.
The DOM transport fixture delivered all 512 frames with the expected checksum;
the output-credit fixture drained to zero outstanding bytes. Existing opt-in
live tests remained ignored.

Delivery targets `dev` followed by `cargo xtask dev-restart`; the final landing
SHA and runtime readback are reported separately in the task closeout.
Live-provider, packaged/signing, updater-feed and production-install gates were
not run for this Dev-only change.
