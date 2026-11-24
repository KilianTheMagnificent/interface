# Harness usability on Dev

Requested 2026-09-12: implement the nine feature slices in
[the research report](../HARNESS_INTERFACE_RESEARCH.md), test them, and land on
Dev. Work branch: `feat/harness-usability-20260912`; initial source `20f01d1`.
Production promotion, updater staging, installation and restart are separate.

## What is implemented

| Slice | Behavior and entry point |
|---|---|
| Shared commands | Rust owns descriptions, argument hints, examples, targets, effects and capability requirements. `iface agent commands` returns the same catalog used by TUI discovery. The desktop no longer lists it (2026-10-14): it cannot run iface TUI commands. Browsing does not dispatch. `/spawn` accepts explicit `--model` and `--effort` before its prompt; `--` ends option parsing. Native provider command namespaces stay provider-owned. |
| Durable Attention | Finished/failed events persist in host-owned storage with original session or terminal generation, independent seen/resolved flags, and reconnect deduplication. Opening marks seen; Clear resolves history. Existing live waiting-state behavior remains. **Next actionable item** cycles through live needs without rearranging panes. A stale terminal target is refused. Completion is not described as verified success. |
| Input intent | TUI distinguishes steer receipts from the unsent follow-up and supports `/queue remove <number>`. Clean panes show editable, removable unsent follow-up text while working. Stop names its scope: Ctrl-C to this native terminal turn. Submission receipts distinguish terminal submission from a later matching message in provider history. Existing input admission, unknown-outcome handling and Close semantics remain authoritative. |
| Task results | **Task details** (one palette row; its context inventory and presets load inside it) is available from the palette, terminal pane menu and completed Attention events. A manual snapshot binds objective, exact checkout, HEAD, full authorized changed-file hashes, paths and omissions. It never attributes every dirty edit to the task. Running existing discovered project checks creates another immutable evidence record. Changed or incompletely covered candidates cannot borrow a passing result. Evidence can be attached to the original conversation or terminal's unsent draft. |
| Workspaces | Presets expose inspect-here, implement-in-worktree and review-existing entry points. Implementation uses the existing worktree creation/launch flow; existing worktree import remains in the session library. Inspection requires no setup. The inspector discovers listeners only inside the selected terminal's pinned process tree and rechecks ownership before opening. A listener is explicitly not an application health check. |
| Handoff and branching | Existing handoff approval now shows the exact destination folder/provider, source conversation, packet and omission limits. Supported provider forks retain a durable parent link with the supported **provider-current** boundary. Branch navigation preserves the source and names both sessions by title. Conversation copying explicitly shares files; separate worktrees provide file isolation. |
| Context | **Context inventory** shows discovered instruction/customization sources, grouped by kind with names and one-line details, and bounded route suggestions from `docs/agent-map.json`. Every entry uses the normal file-read policy; relative traversal and excluded files are refused. Discovery is not represented as proof of provider loading. Check failure output remains attached to its immutable result. |
| Phone continuity | The existing paired session gains generation-scoped result and draft routes over HTTP and encrypted native transport. Desktop and phone drafts use host revision compare-and-set; conflicts preserve local text and require deliberate reload. Results and input receipts appear in native and browser phone views. Existing grant, input lease, reconnect cursor, approval cursor and unknown-input protections remain in force. |
| Pairing experiment | `iface away pair` provides explicit lead/sidekick models and effort, a fixed objective/file scope/protected check plan, at most three rounds and 600 seconds, durable packets, Stop/recovery, and explicit accept/revise/take-over. Checked proposals are held. Acceptance revalidates original source guards and writes an uncertainty marker before file effects. Unknown acceptance is never replayed. Production delegation remains disabled. |

The interface uses the existing theme, typography, focus and modal primitives.
No dependencies were added. Legacy stores retain their schemas. The shared
host protocol is 20 (111 request operations); the new harness contract and
`harness.v1.json` store are version 1. Rust is the only canonical writer.

## Deliberate limits

These are the concrete initial slices specified in the research, not every
possible extension discussed for competitors. The workspace preset does not
install dependencies or invent a generic setup runner. It uses existing
creation failure/cancellation behavior and explicit discovered check actions.
Result checks currently use the existing npm check discovery; the inspector
never invents evidence for an externally run Cargo gate. Results are captured
explicitly rather than inferring task boundaries or authorship from a dirty tree.

Native terminal follow-ups are saved unsent drafts, not a second provider-side
input queue. CLI steering keeps its existing provider-supported semantics.
Provider-current is the supported fork boundary; arbitrary historical forks
are not advertised. Pair roles have separate durable packets and fresh
content-only provider invocations, not persistent native provider conversations.
There are no recursive worker tools or hidden children.

The store is bounded at 8 MiB, 4,096 attention records, and 512 each of results,
drafts and branch records. It refuses a full/corrupt/drifted store and preserves
existing bytes rather than silently evicting unresolved work or resetting history.
Retention (owner-approved, 27 Sep 2026) keeps it from filling in normal use:
recording attention past 2,048 records drops the oldest resolved ones, and
capturing a result past 256 drops the oldest settled results (no unresolved
attention names their target), down to those soft caps. A write refused at
8 MiB is retried once after the oldest settled results, then the oldest
resolved attention, are dropped down to 6 MiB. Unresolved attention, results
whose target still needs the user and the result being recorded are never
dropped; when only those remain at a limit the write is refused as before. The
file format is unchanged and existing files load as they are.
Image attachments remain under the existing attachment owner; shared drafts
contain text. Missing provider model, usage or costs remain unknown.

## Pairing usage

`iface away pair --help` explains the exact commands. Prepare a new absolute,
canonical run directory outside the project. A pair configuration wraps the
existing Away cycle v1 configuration:

```json
{
  "version": 1,
  "leadEffort": "high",
  "sidekickEffort": "medium",
  "cycle": {
    "version": 1,
    "project": "/absolute/project",
    "objective": "Repair the demonstrated boundary case",
    "files": ["value.py"],
    "contextFiles": ["test.py"],
    "validation": {
      "version": 1,
      "files": ["test.py"],
      "protectedFiles": ["test.py"],
      "checks": [["/usr/bin/python3", "-B", "test.py"]]
    },
    "workerModel": "codex/REPLACE_WITH_INSTALLED_MODEL_ID",
    "reviewModel": "claude/REPLACE_WITH_INSTALLED_MODEL_ID",
    "durationSeconds": 300,
    "maxIterations": 3
  }
}
```

Use `start --config … --directory …`, then `status`, `accept`, `revise --message …`,
or `take-over` with that directory. `stop` closes admission; `recover` preserves
interrupted evidence. A rejected or unknown run has no automatic resume.
`compare --baseline … --candidate …` shows both role receipts, reported total
cost, elapsed time and task-contract compatibility. A lead-only baseline uses
the same lead model and effort for proposal and review under the same fixed
protocol; it is not an unconstrained single-agent benchmark. Verify matching
initial source hashes before comparing runs. Never count unknown cost as zero.

Codex effort uses the documented
[`model_reasoning_effort` option](https://developers.openai.com/codex/config-reference);
Claude uses the existing `--effort` CLI argument. Unsupported selections fail,
with no provider fallback. Requested effort is not claimed as reported effort.

## Verification record

Targeted checks completed before the final gate:

- Strict Rust/TypeScript shared protocol fixture round-trips; unexpected fields
  are refused.
- Store reopen, duplicate event, seen/resolved separation, wrong-generation
  resolution, competing draft revisions, corrupt bytes and external drift.
- Real Git result snapshot and stale check refusal; check invocation is injected
  for that test and is not claimed as a production project test run.
- Two real Git worktrees with PTY-owned Python listeners. Discovery returned one
  correct server for each terminal, refused another generation, and closing one
  left both its sibling and an unrelated listener alive.
- Deliberately delayed provider discovery in a real TUI PTY: measured typing
  latency **32.3 ms** with resize. This measures startup discovery, not every
  foreground provider operation; it does not justify a broad event-loop rewrite.
- Local pairing evaluation with fixture role outputs and real protected Python
  checks: lead-only fixture used two role calls; injected faulty-first pair used
  three. Both held the checked candidate without applying. Accept applied once;
  repeated accept, Stop, source drift and an unknown application were refused.
  Revise preserved the source. No live-model quality or cost savings are claimed.
- Disposable Rust-host/Chromium phone smoke passed all 31 scenarios, including
  the new shared-draft revision conflict and result-generation assertions. The
  original PTY was preserved. Log: `/private/tmp/interface-harness-phone-smoke-final.log`.

The final stable-source `cargo xtask check` passed on 2026-09-12 in 150.3 seconds:
1,555 Rust tests passed with 18 existing ignored tests; all 2,686 JavaScript tests
in 144 files passed, plus the Swift voice tests, formatter, lint, type checks,
renderer fixtures and production web builds. Log:
`/private/tmp/interface-harness-gate.log`. Closeout checks the integrated Dev
source again and reuses these receipts only when source and toolchain match.

Rendered coverage includes 13 themes at two widths in both chrome modes,
inspector stale-evidence refusal, viewport placement, Escape, shared-draft
conflict preservation and malformed response refusal. Dark/narrow and
light/wide final screenshots were inspected manually. Evidence directory:
`/var/folders/_b/hwd9v4ks1gb8dcf1jl66s8xc0000gn/T/interface-renderer-fixtures-QesEiT`.
The existing four-pane stress fixture recorded maximum input dispatch 0.2 ms,
switch focus 11.6 ms, zero remounts and zero outstanding writes. It also recorded
a worst animation-frame interval of 717.7 ms during the stress run; passing this
gate does not establish uniformly smooth rendering under load.

All 20 native iPhone Simulator tests passed, including the encrypted Rust
fixture connection and shared WebKit conversation. An earlier WebKit load
failure passed on a focused rerun and the full suite with a fresh fixture;
the original failure's cause was not conclusively established. Log:
`/private/tmp/interface-harness-xcode-final.log`. The disposable fixture was
stopped after testing.

Testing corrected protocol count assertions, stale selectors, theme-token
violations and a real modal placement defect. The full theme matrix was divided
into bounded process lanes so coverage fits the existing per-process deadline.
No test coverage or deadline was removed to obtain the passing gate.

Dev landing and live revision evidence are recorded by `cargo xtask closeout`
and `cargo xtask status --json`, with a matching project ledger session. Physical
iPhone hardware, camera/Face ID hardware, network switching, live-model pairing
quality/cost, packaged-app smoke, signing and updater gates were not run for this
change. No production runtime claim is established by these fixtures.
