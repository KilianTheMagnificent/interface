# Performance, efficiency and maintainability review — October 14, 2026

Review base: `origin/dev` `15b7ca64610ade0f94a287a658f1f658654c1427`.
Task branch: `fix/performance-review-20261014`; isolated checkout:
`/private/tmp/iface-performance-review-20261014`.

This review followed the app's process boundaries, hot paths, lifecycle code,
existing regression suites and earlier review handoffs. The source inventory was
773 non-test Rust/TypeScript/Swift files, approximately 316,322 lines (inline Rust
tests are included in that count). This is a broad architectural and targeted
implementation review, not a claim that every line or every real-world workload
has been exhaustively verified. Measurements below are disposable fixtures;
they do not establish production startup, CPU, memory, or battery improvements.

## Findings implemented

| ID | Priority | Evidence before | Change and acceptance |
| --- | --- | --- | --- |
| PERF-01 | Medium | `useThreadIndex` rebuilt a Map and compared every thread on every App render, even with the same immutable view. | Reconcile only when the view changes; retain unchanged rows across pushes. Real Electron/React StrictMode fixture: 100 unrelated rerenders, zero extra index scans. Existing row-identity tests retained. |
| PERF-02 | Medium | `useThreadDiff` fenced answers but sent every scope/refresh request immediately. Superseded Git reads still competed for host/process capacity; unmount did not invalidate the final result. | `LatestRead` permits one active read plus one replaceable pending selection and suppresses stale answers. Unit fixture: 100 selections produce one trailing read. Real Electron: 20 rapid selections produce two total reads, no obsolete result painted, no pending read after unmount. |
| PERF-03 | Medium | `useThreads` sent a fresh full-state read on every focus event and accepted read completion after effect cleanup. | Reuse the same read-only lane for focus reads, preserving push ordering through `newerThreadsView`. Mutations still send individually. |
| PERF-04 | Medium | `PlanUsageReads.dispose()` cleared only its timer. A pending answer could publish after disposal and launch another queued provider usage probe. A synchronous bridge throw could leave the lane busy. | Explicit lifecycle generation, dropped pending work, suppressed late callbacks, synchronous error handling, and safe effect reactivation. A regression failed on the original code (two CLI reads instead of one) and passes after the fix. |
| PERF-05 | Medium | `ThreadsDocument::place_first` sorted and allocated the entire section for every seeded thread, although normal insertion needs only its first key. | Find the minimum in one pass; sort only when order keys need spreading. Shared comparator preserves ordering. Full-capacity 4,096-thread seed regression checks newest-first order, valid state and idempotent reconcile. Local debug timing: 1,752.6 ms before, 880.6 ms after, single observations; not a production benchmark. |
| PERF-06 | Low | Every fork sharing a worktree repeated the same filesystem existence check in one projected view. | Memoize by exact path within that view only. Three shared records now make one check; a subsequent view checks again and sees a removed directory. Regression failed on the original code (three checks). |
| DEP-01 | High upstream severity; app exploitability not established | npm reported six high-severity package entries, including the locked Electron 43.3.0 runtime. Electron is a devDependency but its binary ships. | Updated Electron to 43.7.11, electron-builder to 26.17.0, and compatible transitive fixes within existing ranges. No forced major upgrade or builder downgrade. Post-update audit: zero high/critical findings; the remaining moderate dependency chain is described below. |
| TOOL-01 | Medium | A clean install leaves Electron's binary manifest unprovisioned until its first import. Parallel unit/renderer checks raced: the fake-provider shell lacked `path.txt`, fell back to `node` on its deliberately restricted PATH, and exited 127. | Provision Electron once in `check.mjs` before starting parallel parts. Confirmed with captured fake-provider stderr; no test timeout or assertion weakened. |

## Maintainability changes

- `latest-read.ts`: one small read-only lifecycle controller shared by the diff
  and focus refresh paths. Its contract explicitly excludes sends and mutations.
- `plan-usage-reads.ts`: scheduling and disposal logic extracted from the React
  hook; `use-plan-usage.ts` now owns React state and wiring. Existing exports
  remain compatible.
- `interface-settings/src/threads/view.rs`: wire projection and ordering extracted
  from the large store/domain file. Front insertion and projected ordering use
  one comparator. Persistence, schema versions and domain transitions stay in
  their existing owner.
- Navigation and area documentation now name these modules. Existing pinned
  tests remain at their original paths and retain their assertions.

The largest controllers still deserve incremental work: App.tsx was 6,434 lines,
Sidebar.tsx 3,337, IntegratedTerminalCenter.tsx 2,913, and tui.rs 3,977 at review
base. Size alone is not a demonstrated defect. Rewriting these wholesale would
mix ownership, terminal identity, provider certainty and approved visual
behavior. The extraction here follows tested responsibilities and fixes actual
lifetime/performance faults; it does not certify the entire app free of tangled
code.

## Coverage and retained protections

| Area | Source paths reviewed or traced | Conclusion / limits |
| --- | --- | --- |
| Desktop renderer | `App.tsx`, Threads hooks/models/fixtures, usage reader, conversation projection, polling, terminal overview and WebGL lifetime | Concrete fixes above. Existing long-transcript and terminal lanes remain the behavioral guard. Several presentation clocks still use intervals; their aggregate idle CPU was not measured, so no battery claim is made. |
| Electron transport/lifecycle | `rust-host-client.ts`, `ndjson-line-buffer.ts`, `terminal-data-transport.ts`, bridge and preload contracts | Bounded pending requests and frames, amortized framing, large-buffer release, and a separate terminal byte plane already exist. No unsupported wire redesign. |
| Rust runtime and providers | `interface-runtime` listing/project-root paths; `interface-engine` event_stream/supervised; Claude store and ACP adapter | Bounded control/output queues, delivery certainty, provider listing reuse and bounded root lookup already exist. Live-provider behavior was not exercised. |
| Persistence and thread domain | `interface-storage/atomic.rs`, settings/thread store/view, host thread worker/run tap | Atomic writes distinguish pre-rename failure from uncertain post-rename writes. Thread projections and insertion improved without schema changes. Durable writes were not weakened. |
| Workspace and review | `interface-workspace/search.rs`, thread workspace/ship adapters and their process boundaries | Search has explicit file/entry/byte/result bounds and credential filtering. No removal of identity/TOCTOU checks to save syscalls. |
| Remote and native shell | `interface-remote/http/direct.rs`, replay, iOS PhoneModel/DirectConnection lifecycle | Existing request semaphore, bounded replay, task cancellation and trust boundary retained. Physical-phone and full Simulator acceptance not run. |
| CLI, orchestration, collaboration and agents | Owning rules/maps; CLI run path; host orchestration and agent seams; shared queue/storage contracts | Existing explicit uncertainty and shutdown-drain rules retained. Gate coverage is broader than manual code inspection here; no claim of a new complete policy/security audit. |
| Build, tests and release | Cargo profiles, Electron dependency graph/check runner, xtask gate/closeout contracts | Avoided duplicate target trees and preserved exact-source receipts. Updated dependency range floors and the lockfile. Production promotion remains separate. |

## Remaining dependency issue

The post-update npm audit reports **8 moderate package entries**, all inherited
from one unpatched `sprintf-js` issue through:
`electron-builder → app-builder-lib → @electron/get → global-agent → roarr → sprintf-js`.
The eight entries are dependency propagation, not eight independent root flaws.
`npm audit --omit=dev` reports zero, but that alone says nothing about Electron's
binary (which is now updated). The remaining chain belongs to the build/download
proxy logger, not the app's production npm dependency tree.

The advisory describes a denial of service when an attacker controls a format
string; this review has not established such an input path in this app's build.
There is no patched sprintf-js release listed. npm proposes forcing
**electron-builder backwards to 26.5.0**, outside the declared range. That was
not used as a substitute for a reviewed packaging change. Follow-up belongs in
`docs/TODO.md`: adopt a compatible upstream fix, or independently validate a
replacement/removal of that build-time chain before changing it.

## Validation and release state

`cargo xtask check` passed on the completed source in 148.5 seconds:

- 37 Rust test executables passed, plus formatting, Clippy and navigation checks.
- 3,929 Electron unit tests in 240 files, release-script tests and typechecks passed.
- Three Swift tests and the voice helper build passed.
- Desktop/remote builds and all 25 Electron renderer lanes passed on 43.7.11,
  including terminal preservation, workspace switching and the new Threads hooks.
- Removing the isolated checkout's Electron `path.txt` before that run verified
  the new preflight provisioning path; fake-provider tests then passed in the
  same full run.
- A separate hidden assembled-app smoke passed on Electron 43.7.11: the real
  renderer/preload/Rust host opened Threads, started a no-project conversation
  with the fake Claude, persisted a completed turn and displayed its reply.
  The kit confirmed it never became frontmost; the disposable session was
  stopped. Screenshot: `work/performance-review/threads-smoke.png`.
  The log includes the kit's documented cross-model collaboration refusal
  (the disposable path exceeds the Unix socket limit), so this is not evidence
  for live collaboration.
- `git diff --check` passed.

The first full gate had four timing failures during the cold build (three
Electron tests and one host MCP test) and one real provisioning race (TOOL-01).
The timing cases passed independently without changed deadlines (the MCP case
in 0.57 seconds), and passed again in the complete gate after the provisioning
fix. The early sandboxed DOM attempt could not bind a loopback port; the same
isolated test ran successfully with the required execution permission.

The initial Threads-only lane on the old runtime passed. The final full gate's
long-transcript fixture (300 turns, 1,800 rows) on 43.7.11 measured an open frame
of 224.1 ms, stream median 9.8 ms / p95 16.9 ms and max 17.8 ms. These are
acceptance observations, not before/after claims for the source changes.
Renderer artifacts: `/var/folders/_b/hwd9v4ks1gb8dcf1jl66s8xc0000gn/T/interface-renderer-fixtures-ZEFrun`.
The build still reports a chunk over its 1,700 kB warning threshold; this review
does not establish its startup/parse cost or justify changing the existing split
strategy without a profile.

After integrating Dev through `d5ce8054` (the concurrent phone/remote backend),
`cargo xtask check --changed` reran both suites and passed in 125.2 seconds:
37 Rust test executables, 3,930 Electron unit tests, all 25 renderer lanes,
types, builds and Clippy. The only merge conflict was the shared TODO list;
all workstreams' entries were preserved. Integrated renderer artifacts:
`/var/folders/_b/hwd9v4ks1gb8dcf1jl66s8xc0000gn/T/interface-renderer-fixtures-Bhnnqp`.

Dev closeout is named **Threads avoid redundant reads and clean up background work**.
Its final landing and live verification are reported by `cargo xtask status`
and the owner-facing closeout response.

Production promotion, packaging/signing, updater staging, installation/restart,
live-provider checks, physical phone and a full Simulator run are outside this
Dev landing. The new Electron/build-tool combination still needs the normal
packaged smoke before any production release.

## Sources and method

Memoization and effect cleanup follow [React useMemo](https://react.dev/reference/react/useMemo)
and [React useEffect](https://react.dev/reference/react/useEffect); correctness
continues to rely on immutable data, generation checks and tests, not cache
retention. [Electron's performance guide](https://www.electronjs.org/docs/latest/tutorial/performance)
supports measuring work and avoiding redundant/blocking UI work.

Dependency evidence: npm audit against the task lockfile on 2026-10-14, plus
[Electron's preload-cache advisory](https://github.com/electron/electron/security/advisories/GHSA-qmv3-fv6v-rmhq)
and [sandbox-inheritance advisory](https://github.com/electron/electron/security/advisories/GHSA-gr2m-v5gq-v685).
The [sprintf-js advisory](https://github.com/advisories/GHSA-hp3w-g68c-fv3c) lists no
patched version. Raw local audit snapshots are in
`work/performance-review/npm-audit-{before,after}.json`; scratch is not committed.
