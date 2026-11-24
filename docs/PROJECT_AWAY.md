# One-button project Away loop

The coffee **Start Away loop** toolbar button works on the project in front of
you: the focused thread's project on Threads (its own record, so a worktree
thread names its project, not the worktree), the focused task's repository on Agent, the
active terminal's folder on Code. The first time in a project it opens the form
so the folder and agent are confirmed before anything edits them; once a
project has saved settings it starts at once with them. While loops run, the
button reads **View Away loops** and opens them instead of starting another. A
"No project" thread, or nothing focused, opens the form to choose a folder.
The adjacent **Away loop options** arrow opens the panel: the loops, running
first, each opening its thread, over the start form, drawn like the New thread
card. Choose the project, the objective, the model from the shared picker
(Codex and Claude only, each led by its own default model), the reviewer,
minutes and rounds; **Start loop** saves those choices for that project. Defaults: Codex's default
model (Claude when Codex is unavailable and Claude is ready), one hour, 12
iterations, useful work grounded in project instructions and its to-do list.

**A loop is a Threads thread (review B3, protocol 45).** Its first work round
makes the loop's own git worktree of the project (as a thread's New worktree
does: `<repo parent>/.interface-worktrees/away-…`, branch `iface/away-…`,
from the current branch's HEAD, so uncommitted changes in the checkout are not
in it), creates the work session there and files it as the thread. Starting
from the toolbar or the panel opens that thread on Threads. Every work round is
a turn of that one session, so the thread shows each with its changes card and
diff; every review is a fresh session in the same worktree (read-only), kept
out of Threads, and its verdict is a card under the round it reviewed. The
loop's report (its status and Stop while it runs) ends the thread; Review
changes opens the Changes panel, where the owner commits, opens a pull request
and merges with the ship controls. While the loop runs it holds the thread: the
owner's prompts, Edit from here, Restore and Open in terminal refuse with
`threads-away-running`, definitely unsent; when it ends, the thread is the
owner's to continue. **The reviewer (protocol 47; owner, 14 Oct: "Other vendor by default").** A
new project's form proposes the other vendor as the reviewer when it can run
(Claude reviews Codex's work and Codex reviews Claude's), else the worker's
own provider; the Reviewer picker changes it, and the choice is saved with
the project's settings. The reviewer runs at its provider's own default
model, in a fresh session in the loop's worktree, with that provider's
read-only restrictions unchanged (Codex in its read-only sandbox; Claude in
`acceptEdits` without Write, Edit, NotebookEdit, Bash, Agent, Task and
WebFetch). Options without a reviewer, which every loop and saved project
from before had, are reviewed by the worker's own provider and model, as
before. Each review card names its reviewer.

A project that is not a git repository cannot be given a
worktree, so its loop stops Blocked before any session starts. Loops from
before protocol 45 keep working from their records: they have no thread, their
sessions stay out of the lists, and the panel opens their newest session.

The desktop loop runs native managed agent sessions with project tools. Work
rounds inspect branch and dirty state, preserve unrelated changes, complete one
justified task and run relevant checks. A fresh session reviews the actual
source and evidence before continuation.
Review findings feed the next work session; unresolved reviews and repeated
reports stop the loop. The host also stops at the immutable deadline, iteration
limit, missing/malformed results, permission needs or uncertain provider outcomes.
This does not replace or alter the existing selected-file `iface-away` CLI/skills.

The panel and the thread's report provide **Stop loop**; the thread carries the
rounds, their reviews and the report, and the panel lists recent loops.
Closing it or hiding the app keeps work running.
Quitting asks first while a loop runs; an explicit Quit drains the loop's own
sessions. Interface keeps the Mac from idle sleep while a loop runs (the
display may still sleep); closing the lid or a forced sleep can still delay
work, and the absolute deadline is checked before further dispatch. Restart marks interrupted runs
unknown and never resumes them automatically. Interrupted edits remain in the
loop's worktree and may be incomplete; the thread is released, so the owner may
continue it by hand.

## Ownership

- `crates/interface-contracts/src/away.rs`: independent v1 envelope, shared with
  TypeScript through `electron/src/fixtures/project-away-v22.json` and, for
  protocol 45's thread fields and `ThreadAway` projection,
  `project-away-v45.json` and `threads-v45.json`.
- `crates/interface-collaboration/src/project_away.rs` and `project_away/store.rs`:
  admission, presets, iteration/review limits, cancellation, result validation,
  the thread's rounds as Rust projects them, and a private writer lease with
  compare-and-save at `<data root>/away/project-away.v3.json` (store version 3,
  protocol 47: options may name a reviewer; when it does not exist, a
  `project-away.v2.json`, else a `project-away.v1.json`, is read once, never
  changed, and carried over with every newer field absent; an unreadable one
  keeps Away unavailable). Uncertain storage (a failed or
  unproven write, or a target changed outside the writer) fences all new effects
  until restart. A full history is reported without that fence: see below.
- `apps/interface-host/src/project_away.rs`: exact host-authorized project roots,
  native sessions, completion observation, and the thread port: the worktree,
  the bind, the per-send permit and the projections it hands the thread host
  after every commit. `apps/interface-host/src/threads/away.rs` holds the
  thread, refuses others' sends and joins `ThreadView.away`. New sessions start without a
  project prompt; their exact identities are persisted before task dispatch.
  Events are subscribed before sending. A lost start response can be retried
  with the same identity; Stop-before-start leaves a durable cancellation receipt.
- `electron/src/renderer/components/AwayControl.tsx`: renders Rust state and sends
  owner commands; `away-thread.ts` and `components/ThreadAwayParts.tsx` draw the
  thread's round lines, review cards and report. It retains unresolved start identity across window reloads.
  Polling and closing the view never authorize a launch or continuation.

Codex work uses workspace-write with no automatic escalation; review requests
and verifies read-only mode. Claude runs in `acceptEdits` with the
workspace-write sandbox mapped onto Claude Code's own OS sandbox through the
flag-settings layer: Bash writes stay in the project, sandboxed commands have no
network and cannot retry unsandboxed, the session refuses to start without the
sandbox, and the file tools refuse reads outside the working directories.
WebFetch is denied, and so, for reviewers, are Bash, Write, Edit, NotebookEdit,
Agent and Task. Anything outside those bounds raises an approval, which stops the
run for the owner. A project's own `.claude` settings can still add allow rules or
sandbox exclusions. Run instructions prohibit secrets, delegation, background
jobs, dependency installation, commits, merges, publication and application
restarts. Those instructions override project automatic-closeout defaults within
Away runs. Installed provider CLIs own authentication; Interface reads no
credentials.

Settings allow 1–1440 minutes and 1–20 iterations, with up to four simultaneous
projects and one active run per canonical folder. Queues, reports, history and
display projections are bounded. Complete session histories remain in existing
provider/session stores; UI excerpts are labeled. Agent check/review evidence
does not establish deployment. Dev, main, updater and installation stay separate.

The store keeps every run; nothing prunes history. It refuses a change that would
exceed 256 runs, 256 presets or 4096 cancellation receipts, or a 32 MiB file.
These refusals are checked before anything is written, so they never mark
storage uncertain; neither does a write that failed before its rename. After a 32 MiB refusal, new runs and new turn receipts are
refused until restart while an in-flight run can still be finished (a turn whose
report could not be saved ends Unknown). If the file already sits at the cap,
startup recovery of an interrupted run can itself be refused and Away then
reports unavailable. There is no in-product archive or reset yet; retention is an
owner decision. Do not delete the store to resolve an unknown action.

## Verification

`cargo xtask check` is required for this Rust/Electron/protocol change. Controller
tests cover duplicate admission, Stop before start, cancellation, expiry, unknown
outcomes, fresh review/refinement and recovery. The host test uses a synthetic
Codex app-server to exercise real requests, separate native work/review sessions,
an actual disposable-file edit and retained results without a provider account.

`INTERFACE_AWAY_ONLY=1 npm run test:renderer` exercises one click, double click,
lost response/reopen reconciliation, Stop and project selection in real Electron
DOM at desktop and narrow widths. Live provider and running Dev verification
must be reported separately from these deterministic checks.

Live acceptance on 2026-09-14 exercised both installed Codex and Claude against
separate disposable clamp-repair projects through the real desktop host protocol.
Each loop completed one work session and a fresh review session. An independent
driver verified all six protected acceptance cases afterward. Codex's reviewer
also reran the checks; Claude's restricted reviewer inspected the source and
recorded that it could not independently execute tests. These small trials verify
provider execution and completion, not the quality of arbitrary unattended work.

## Persistence and UI reads (September 24)

All service reads/writes serialize through one owned async store guard; JSON
preparation, strict validation and fsync execute on a blocking worker. A cancelled
caller cannot cancel a committed admission or an in-progress write. Shutdown
closes admission, cancels/drains drivers and waits for pending storage work.
Preparation encodes once, while writing still rechecks the writer lease, target
identity and durability. Uncertain storage remains latched; identical presets
skip writes only while storage is healthy. Persisted snapshot v1 is unchanged.

Desktop protocol v28 adds read-only Away status v2. Closed controls poll compact
revision, active-run identity and all start/cancellation receipts; the initial
read and open panel use detailed v1 snapshots. Open/active/pending controls poll
at 2 seconds, idle closed controls at 15 seconds, hidden documents pause, and
requests never overlap. Opening refreshes immediately. Equal revisions reuse
object identity while still reconciling pending start/stop receipts.

The native turn listener also has a 4 MiB payload budget. Any omission notice
or control overflow ends that turn with an unknown outcome; it cannot certify
completion from an incomplete transcript.
