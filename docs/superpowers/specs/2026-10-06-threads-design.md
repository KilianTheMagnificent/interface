# Threads: design and frozen contract (2026-10-06)

Owner request (6 Oct 2026): turn the Chat surface into a T3 Code–style
**Threads** mode. Owner decisions, recorded in the session:

1. Project threads are full coding agents (tools, every provider). "No
   project" keeps today's plain, tool-less chat unchanged.
2. The Threads sidebar is an **inbox** (Pinned / Active / Snoozed / Settled,
   flat across projects with a project scope filter), not a project tree.
   The owner approved changing the settled look "Sidebar: one header row,
   single-line rows, soft count chips" for this, and recording its trailer.
3. No overlap with the Agent surface: Agent stays the place for work handed to
   teammates (its tasks always belong to a teammate); Threads is the owner's
   own work with a provider. Agent-kind sessions never appear in Threads and
   nothing on the Agent surface is removed.
4. Workspace default is **Local** (the project's own checkout); a new git
   worktree is opt-in per thread.

Research behind this: T3 Code main `9bd1d80` (Orchestration V2), Codex
app-server 0.160.1 schema, Claude Code 2.1.291. T3 facts used here: thread is
durable and app-owned; the provider session underneath is replaceable;
workspace chosen at creation; hidden git-ref checkpoints per turn give per-turn
diffs and rewind; file restore only in an isolated worktree; inbox sidebar;
status ladder; unread tracked by the server; settle instead of delete.

## Vocabulary

- **Thread**: one conversation listed on the Threads surface. Its identity is
  the existing session key `engine#sessionId`. There is no separate thread id:
  every pane, draft, queue and layout is already keyed by session key, and
  `registry.json` (a frozen format) stays untouched. Thread state is an
  overlay record in a new Rust-owned store.
- **Project thread**: a thread whose workspace is a folder (normally a git
  repository). Launched as session kind `code` with tools, any engine.
- **Plain chat** ("No project"): today's chat, kind `chat`, Claude-only,
  tool-less, private folder. Unchanged except that it now has thread state
  (pin/settle/snooze/unread) too.
- **Turn**: one owner prompt and everything the agent did until the session
  returned to idle (or failed / was interrupted). Turn ordinals count prompts
  sent through Interface for that thread, starting at 0.
- **Section**: pinned | active | snoozed | settled (derived in Rust).

Which sessions are threads: exactly the sessions the renderer lists on the
Chat surface today (`sessionSurfaceOf(summary) === "chat"`, minus the existing
Away/orchestration-worker exclusions). Agent-kind sessions are never threads.
Rust decides those exclusions from the whole orchestration agent list and,
since protocol 58, names the hidden registry sessions on the view
(`hiddenKeys`), which Chat's session list applies too (with the orchestration
snapshot's workers, for one just spawned).

## Rust ownership

| Piece | Owner |
| --- | --- |
| DTOs, command enum, fixture round-trip | `crates/interface-contracts/src/threads.rs` |
| Store `threads.v2.json` (was `threads.v1.json`): leased single writer, atomic, bounded | `crates/interface-settings/src/threads.rs` |
| Git mechanics: worktree create/recreate/remove, checkpoint capture, diff, numstat, restore, branch/worktree listing | `crates/interface-review/src/thread_git.rs` |
| Fork at a turn | `interface-engine` adapter trait + `engine-codex` + `engine-claude` + `interface-runtime` management op |
| Command routing, push, run tap, auto-settle, snooze wake, seeding, send hook | `apps/interface-host/src/threads.rs` (+ wiring in router/bootstrap) |
| Rendering only | `electron/src/renderer/**` |

The renderer never decides a section, unread state, order key, isolation or
whether a restore is allowed; it renders the `ThreadsView` and sends commands.

## Wire contract (protocol 34)

- New request channel `iface:threadControl` (Rust `CHANNELS.thread_control`,
  TS `CHANNELS.threadControl`). Served by a host dispatcher `ThreadHost`
  (`OperationOwnerRouter::with_threads`, ahead of the desktop dispatcher, like
  agents). Not replay-safe (same rule as agent/harness control).
- New push channel `iface:threads` carrying a full `ThreadsView` after every
  committed change (and on snooze expiry / auto-settle). TS
  `onThreads(cb: (view: ThreadsView) => void): () => void`.
- Protocol 33 → 34. Request operations 117 → 118, pushes 11 → 12, desktop
  channels 128 → 130. Update every place the counts are asserted
  (`crates/interface-contracts/src/ipc.rs`, `apps/interface-host/src/{protocol,router}.rs`,
  `electron/src/main/{ipc-payload,rust-host-client}.ts`, `docs/PROTOCOL_FACTS.md`
  + a "Protocol v34 adds…" paragraph, any smoke scripts that count).
- Shared fixture `electron/src/fixtures/threads-v34.json` with every command,
  a full view and a full response; round-tripped by Rust
  (`deny_unknown_fields`, extra field rejected) and by TypeScript tests.

### Rust types (`interface-contracts/src/threads.rs`)

All structs `#[serde(rename_all = "camelCase", deny_unknown_fields)]`;
optional fields `#[serde(default, skip_serializing_if = "Option::is_none")]`;
times are milliseconds since the Unix epoch (`u64`).

```rust
pub enum ThreadSection { Pinned, Active, Snoozed, Settled }            // "pinned" …
pub enum SettledReason { Manual, Idle, Merged, Edited, External }      // "manual" …
pub enum WorkspaceMode { Local, Worktree, None }                       // "local" | "worktree" | "none"
pub enum WorkspaceKept { Dirty, Ignored, InUse }                       // "dirty" | "ignored" | "inUse" (Fix F4)
pub enum RunOutcome { Completed, Failed, Interrupted }
pub enum TurnState { Running, Done, Failed, Interrupted }
pub enum LineageRelation { Fork, Edit, Handoff }  // handoff: protocol 46
pub enum TitleSource { Prompt, Generated, User }
pub enum PullRequestState { Open, Merged, Closed }
pub enum DiffScope { Turn, Thread, Uncommitted, Branch }
pub enum DiffFileStatus { Added, Modified, Deleted, Renamed }
pub enum DiffOmitted { Sensitive, Large, Binary }

pub struct ThreadWorkspace {
    pub mode: WorkspaceMode,
    pub project_root: String,          // "" for mode none
    pub worktree_path: Option<String>, // mode worktree
    pub branch: Option<String>,
    pub base_ref: Option<String>,
    pub owned: bool,                   // this thread created the worktree
    pub missing: Option<bool>,         // derived at read: worktree dir absent
    pub kept: Option<WorkspaceKept>,   // derived at read (Fix F4): an archive/delete/forget kept it
}
pub struct ThreadLineage { pub parent: String, pub relation: LineageRelation, pub kept_turns: u32, pub at: u64 }
pub struct ThreadRun { pub started_at: u64, pub ended_at: Option<u64>, pub outcome: Option<RunOutcome> }
pub struct ThreadTurn {
    pub ordinal: u32,
    pub sent_at: u64,
    pub prompt_digest: String,   // first 16 hex of sha256(normalized prompt); normalized = "\r\n"→"\n", then trimmed
    pub has_start: bool,
    pub has_end: bool,
    pub state: TurnState,
    pub files_changed: Option<u32>,
    pub added: Option<u32>,
    pub removed: Option<u32>,
}
pub struct ThreadPullRequest { pub url: String, pub number: u64, pub state: PullRequestState, pub checked_at: u64 }
pub struct ThreadView {
    pub key: String,
    pub section: ThreadSection,
    pub created_at: u64,
    pub pinned_at: Option<u64>,
    pub pin_order: Option<String>,
    pub active_order: Option<String>,
    pub settled_at: Option<u64>,
    pub settled_reason: Option<SettledReason>,
    pub snoozed_until: Option<u64>,
    pub woke: bool,
    pub unread: bool,
    pub last_visited_at: Option<u64>,
    pub last_run: Option<ThreadRun>,
    pub auto_settle: bool,
    pub workspace: Option<ThreadWorkspace>,
    pub lineage: Option<ThreadLineage>,
    pub pull_request: Option<ThreadPullRequest>,
    pub title_source: Option<TitleSource>,
    pub turns: Vec<ThreadTurn>,
}
pub struct ThreadSettings {
    pub auto_settle_days: u32,       // default 3; 0 = never
    pub settle_on_merge: bool,       // default true
    pub generated_titles: bool,      // default true since 15 Nov (was false: spends provider usage)
    pub track_pull_requests: bool,   // default true (needs `gh`; silent when absent)
}
pub struct ThreadsView { pub version: u8 /* 1 */, pub revision: u64, pub now: u64, pub settings: ThreadSettings, pub threads: Vec<ThreadView> }

pub struct PreparedWorkspace { pub token: String, pub cwd: String, pub workspace: ThreadWorkspace }
pub struct BranchList { pub current: Option<String>, pub default_branch: Option<String>, pub branches: Vec<String> }
pub struct WorktreeEntry { pub path: String, pub branch: Option<String>, pub owner_key: Option<String> }
pub struct DiffFile {
    pub path: String, pub old_path: Option<String>, pub status: DiffFileStatus,
    pub added: u32, pub removed: u32, pub binary: bool,
    pub patch: Option<String>, pub omitted: Option<DiffOmitted>,
}
pub struct ThreadDiff { pub scope: DiffScope, pub base: String, pub head: String, pub files: Vec<DiffFile>, pub truncated: bool }
pub struct RestoreResult { pub restored: u32, pub recovery_ref: String }

pub struct ThreadControlResponse {
    pub view: ThreadsView,
    pub prepared: Option<PreparedWorkspace>,
    pub branches: Option<BranchList>,
    pub worktrees: Option<Vec<WorktreeEntry>>,
    pub diff: Option<ThreadDiff>,
    pub restore: Option<RestoreResult>,
    pub key: Option<String>,        // new thread key (forkAt / editFrom)
    pub prompt: Option<String>,     // editFrom: the edited turn's prompt text when known
}

#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase", deny_unknown_fields)]
pub enum ThreadCommand {
    Read {},
    Pin { key: String },
    Unpin { key: String },
    /// Place `key` before `before_key` in `section` (pinned|active); absent = end.
    Reorder { key: String, section: ThreadSection, before_key: Option<String> },
    Settle { key: String },
    Unsettle { key: String },
    Snooze { key: String, until: u64 },
    Unsnooze { key: String },
    Visit { key: String },
    MarkUnread { key: String },
    SetAutoSettle { key: String, enabled: bool },
    SetSettings { settings: ThreadSettings },
    /// mode local|worktree|existing (`existing` requires worktree_path).
    PrepareWorkspace { project_root: String, mode: String, base_ref: Option<String>, worktree_path: Option<String>, hint: String },
    DiscardWorkspace { token: String },
    /// After a launch answered `key`. `token` from PrepareWorkspace; absent for plain chat.
    Bind { key: String, token: Option<String>, prompt: String, title_source: Option<TitleSource> },
    ListBranches { project_root: String },
    ListWorktrees { project_root: String },
    TurnDiff { key: String, ordinal: u32 },
    ThreadDiff { key: String, scope: DiffScope },
    RestoreTurn { key: String, ordinal: u32 },
    /// keep_turns / ordinal count the thread's RECORDED turns (turns[]), not the provider's prompts.
    ForkAt { key: String, keep_turns: u32 },
    EditFrom { key: String, ordinal: u32, restore_files: bool },
    ReleaseWorkspace { key: String },
    RegenerateTitle { key: String },
    RefreshPullRequest { key: String },
}
```

TypeScript mirror `electron/src/shared/threads.ts` uses the same camelCase
names, string unions for enums, `?:` for `Option`, and
`ThreadCommand` as a discriminated union on `kind`. `IfaceApi` gains
`threadControl(command: ThreadCommand): Promise<ThreadControlResponse>` and
`onThreads(...)`.

## Store `threads.v1.json` (now `threads.v2.json`)

*(Store version 2, protocol 36, 14 Oct; owner-approved.)* Each turn record
also keeps `endedAt` (when its run ended), `touched` (the repository-relative
files its run's own edits named, sorted, at most 1,000 paths / 64 KiB, else
absent) and `shared` (someone else worked in its checkout when it ended, as
`turn_measure` decides). The view's `ThreadTurn` gains `endedAt`. Version 2
lives in `threads.v2.json`: when it does not exist, `threads.v1.json` is
read once, never changed, moved or quarantined, and carried over with every
turn's new fields absent — unknown, never invented; an unusable v1 file is
left for the build that wrote it and the store starts empty. A version 1
document inside `threads.v2.json` is quarantined.

Private data root, beside `harness.v1.json` / `agents.v2.json`. Leased single
writer (`interface-storage` primitives), atomic replace, bounded (4 MiB, 4096
records, 200 turns per record keeping the newest, 64 pending prepared
workspaces). `revision` increments per committed change. Unknown or corrupt
file → quarantine and start empty (existing storage pattern), never crash.
*(Fix F2, 6 Oct.)* A fresh store (no file, or one quarantined at open)
starts its `revision` at the time it opened in milliseconds, so a renderer
holding a view from before takes the new one as newer. A write refused at
the byte cap first cuts the turns of the threads idle longest (any section)
to their newest 20, then evicts records whose session is missing (longest
missing first), then empties the idlest threads' turns, and retries once.
Only a missing record is ever evicted (also at the record cap): a listed
session's record would seed again as a new, active thread.

Record fields: `key, createdAt, pinnedAt?, pinOrder?, activeOrder?,
settledAt?, settledReason?, unsettledAt?, snoozedUntil?, snoozedAt?,
autoSettleDisabled, lastVisitedAt?, markedUnread, lastRun?, workspace?,
lineage?, pullRequest?, titleSource?, missingSince?, turns[]` (each turn also keeps its
`startRef`/`endRef` full ref names and the prompt preview, first 200 chars) and
`prepared[]` (token, cwd, workspace, baseline ref, createdAt). Settings with the
defaults above.

## Derived state (Rust, at read and on every push)

- `section`: settled if `settledAt`; else snoozed if `snoozedUntil > now`;
  else pinned if `pinnedAt`; else active. Settling clears pin and snooze;
  pinning un-settles; snoozing keeps pin/order.
- `woke`: `snoozedUntil <= now` and (no `lastVisitedAt` or
  `lastVisitedAt < snoozedUntil`). Expired snoozes are cleared on the next write.
- `unread`: `markedUnread`, or the last run ended (completed or failed) after
  `lastVisitedAt` (or never visited). A running thread is never unread.
- Ordering keys are fractional-index strings (base-62, lexicographic). New and
  un-settled threads go to the **top** of Active. Activity never reorders.
  Threads without `activeOrder` sort after keyed ones by `createdAt` desc.

## Behavior

**Seeding (first open with no file) and new sessions.** The host observes every
registry update. A Threads-surface session with no record gets one:
- origin `app` (started in Interface) → active, `createdAt` = registry
  `firstSeen` (or now);
- origin `store` (found in provider history, e.g. a terminal CLI run) →
  settled with reason `external`, unless it is working right now;
- at first seeding only: app sessions whose `lastActivity` is older than
  `autoSettleDays` → settled `idle`.
Records whose session left the registry (forgotten/deleted) are dropped on the
next write; archived sessions keep their record (restore brings state back).
*(Fix F4, 6 Oct.)* A `listSessions` request merges the providers' stores into
the registry but schedules no registry push (the renderer reads its answer).
When it changed the registry's content revision (or the revision is unknown),
the runtime dispatcher rings the thread host's registry signal
(`with_listing_observer`), so a session only a listing found (a terminal CLI
run) gets its record at once instead of at the next unrelated change. No
extra registry push is sent.
*(Fix F2, 6 Oct.)* A record whose session is missing from the registry (or
off the Threads surface) is marked `missingSince` and dropped only once it
has been missing for 24 h; it is cleared when the session comes back. While
the registry was not restored from its cache (it is being rebuilt from the
providers' listings) nothing is marked missing. A dropped record's worktree
is never released: only the owner's archive, delete or forget of that
thread's session releases one (`RemovalHooks` on `deleteSession` /
`forgetSession`).

**Run tap.** The host watches engine events per session key. Run start (a send
forwarded to the provider, or the launch prompt) sets `lastRun = {startedAt}`;
the session returning to idle sets `endedAt`/`outcome` (completed, failed, or
interrupted for a stop) and pushes. Interrupted runs do not make a thread
unread. *(Fix F2, 6 Oct.)* Every send the host records opens a new run
generation, in the same store commit as its turn; a run start or end the
worker applies after a newer send's turn began is dropped, so a stale end
never finishes the next turn, and no reconcile sees the new turn running
while the tap does not.

**Send hook (project threads with a git workspace).** Before the host forwards
a send for a thread key: (1) if the workspace is a worktree that is missing,
recreate it from its branch (`git worktree add <path> <branch>`), failing the
send definitely-unsent with a clear message if that fails; (2) capture the
turn's start checkpoint (bounded: 10 s, skipped when the capture would exceed
limits; a skipped capture records `hasStart: false` and never blocks the
send); (3) append the turn record. At run end capture the end checkpoint and
store `filesChanged/added/removed` from a numstat between start and end.
*(Fix, 7 Oct.)* A checkpoint snapshots the whole checkout, so where another
live session or unsettled thread works in it too
(`ThreadsDocument::turn_measure`), that numstat covers only the files the
run's own edits named (the diffs they produced and edit tools' own files, as
the run tap collects them); with none named, or the set unknown, the counts
stay unknown. *(Fix, 14 Oct, store v2.)* The turn keeps those files and that
it was shared, and its Review diff (`TurnDiff`, and `ThreadDiff` scope
`turn`) then covers only them; with none known it refuses
(`threads-turn-files-unknown`) rather than show someone else's changes. A turn
alone in its checkout still diffs all of it, shell commands' edits included.
The launch prompt's turn 0 uses the baseline captured by `PrepareWorkspace`,
attached by `Bind`, which renames it from `refs/interface/threads/pending/<token>`
to `refs/interface/threads/<h>/0/start` in one `update-ref` transaction
(`DiscardWorkspace` and the stale sweep delete unbound pending refs).
*(Fix F2, 6 Oct.)* A send is taken back (turn removed, run restored) only
when it never reached the provider, or when the provider refused it without
answering in its run; a refused send the provider had already answered
(an ACP prompt is answered only when its turn ends) ran, and keeps its
turn. The send gate is still held until the send settles: for ACP that is
the whole turn, as the runtime's own turn slot already queues the next
send behind it.
*(Integration, 6 Oct.)* A send the provider folds into a running turn —
Codex or Claude, while the turn is running by both the run tap and the
registry: Codex `turn/steer`, Claude's input queue — opens no turn and leaves
the running one running. If the provider instead starts a new turn with it
(the turn ended first), the tap records that turn when it starts, within 30 s
of the running one ending, with no start checkpoint.

**Checkpoint capture** (T3's method): temporary `GIT_INDEX_FILE` seeded from
HEAD, `git add -A` (untracked included, ignored excluded), `write-tree`,
`commit-tree` (parent HEAD when present), `update-ref
refs/interface/threads/<h>/<ordinal>/<start|end>` where `<h>` is the first 16
hex of sha256(key). Never touches the user's index, branch or stash. Skip when
more than 20 000 paths would be staged or any single new file exceeds 50 MiB.
*(Fix, 7 Oct.)* The temporary index is a copy of the real index, not HEAD:
`git add -A` makes the tree the working tree's either way, and nothing reads
HEAD's tree objects. Walking them (`read-tree -m HEAD`) on a project in iCloud
Drive, whose loose objects iCloud had evicted, held a new thread for the whole
10 s budget and then started it without a checkpoint. A file untracked with
`git rm --cached` and ignored now stays out of checkpoints.

**Diffs.** `TurnDiff` = start ref vs end ref (end absent → vs the working
tree). `ThreadDiff` scopes: `thread` = first start ref vs working tree;
`uncommitted` = HEAD vs working tree; `branch` = merge-base(baseRef, HEAD) vs
working tree (worktree threads with a base). Patches pass the
`interface-review` credential/binary path classifier (`omitted: sensitive`),
256 KiB per file (`large`), 2 MiB total (`truncated: true`).

**Workspaces.**
- `PrepareWorkspace local`: validates `projectRoot` is a folder the owner may
  open (same authority as today's composer folder), captures a baseline when
  it is a git repo, returns `cwd = projectRoot`, `workspace.mode = local`
  (non-git folders: mode local, no checkpoints, diff scopes unavailable).
- `PrepareWorkspace worktree`: git repo required. Creates
  `<repo parent>/.interface-worktrees/<slug>-<6 hex>` on new branch
  `iface/<slug>-<6 hex>` from `baseRef` (default: the current branch's HEAD).
  `slug` from `hint` (the first prompt), lowercase ascii words, ≤ 32 chars.
  `owned: true`. Copies nothing ignored. *(Fix F2.)* A `git worktree add`
  that fails or runs out of time is abandoned: its folder, registration and
  (commit-less) branch are removed.
- `PrepareWorkspace existing`: `worktreePath` must be a worktree of the same
  repository; `owned: false`.
- The launch then runs in `cwd` with kind `code`. If it fails the renderer
  sends `DiscardWorkspace` (removes a just-created clean worktree and its
  branch if it has no commits; otherwise keeps both). Prepared entries older
  than one hour that were never bound are discarded at host start.
  *(Fix F2.)* Neither removes a worktree an unarchived registry session,
  live session or thread works in (a launch whose bind never arrived); the
  entry then stays for a later sweep.
- *(Fix F2.)* `Bind` with an `existing` workspace owned by a settled thread
  moves the ownership to the new thread (`owned` true there, false on the
  settled one): "Edit from here" on the first prompt with its files
  restored settles the original and starts a thread in the same worktree.
- These worktrees are **not** in the managed-worktree ledger and are never
  released at stream end. `ReleaseWorkspace` (and archiving/deleting the owning
  thread) removes the worktree directory when it is clean and no live session
  or other unarchived thread uses it; the branch is always kept. A dirty
  worktree is kept and the command says so. *(Fix F2.)* So is one holding
  ignored files (`git ls-files -o -i --exclude-standard --directory`), of
  any kind, rebuildable or not: `git worktree remove` would delete them; the
  owner removes it. Recreating a missing worktree removes only that path's
  stale registration (`git worktree remove --force <path>`; a locked one is
  refused), never a repository-wide prune.
- *(Fix F4, 6 Oct; protocol 34 extended before release.)* When the release
  that follows an archive, delete or forget keeps the owned worktree, the
  view says why: `workspace.kept` is `dirty` (uncommitted changes),
  `ignored` (ignored files) or `inUse` (another live session or unarchived
  thread uses it). It is derived from the record's release outcome while the
  thread still owns the worktree, never stored in `workspace`, and cleared
  when a later release succeeds (a re-archive, or the owner's
  `ReleaseWorkspace`) or the session is unarchived. The archived or deleted
  thread's record stays in the view (an archived one for as long as it is
  archived, a deleted one for the day its record is kept as missing), so the
  renderer sees the outcome after its own archive answered. The renderer
  shows a notice the first time it sees a thread's `kept` (never for one
  already kept when the app opened), in the worktree notice region,
  dismissed by hand: "Kept the worktree <path>: it has ignored files such
  as .env / it has uncommitted changes / another session uses it. Remove
  it when you're done."
- Fork inside a thread-owned worktree is allowed (the fork shares it,
  `owned: false`).

**Restore** (`RestoreTurn`, and `EditFrom` with `restoreFiles`): allowed only
when the thread's workspace is a worktree it owns, no other live session or
unsettled, unarchived thread uses that directory (or one inside/containing
it), and the turn has a start ref. First captures a recovery ref
`refs/interface/threads/<h>/recovery/<ms>`, then `git restore --source <ref>
--worktree --staged -- .` and `git clean -fd` (never `-x`). Otherwise refuse,
definitely-unsent, with the reason. *(Fix F2.)* `RestoreTurn`, and
`EditFrom`'s restore and fork (and `ForkAt`), hold the thread's send gate
and recheck busy under it; a gate a send holds refuses them as busy.

**Fork at a turn** (`ForkAt keepTurns = k`, k ≥ 1): keeps the first k owner
prompts and their answers. *(Fix F2, 6 Oct; supersedes the counting below.)*
`k` counts the thread's RECORDED turns (`turns[]`), and the cut is made by
identity, not by position: the host takes the kept turn record (ordinal
`k−1`) and passes the adapter `ForkBoundary { promptDigest,
laterSameDigest }` (how many later recorded turns share that digest). The
adapter walks its own history from the newest and takes the
(`laterSameDigest`+1)-th prompt with that digest (same normalization):
Codex hashes each turn's first owner prompt and forks with that turn as
`lastTurnId`; Claude walks the prompts `fork_cut` counts and cuts after that
prompt's last answer. So history from before a seeded thread, a terminal's
prompt or a counted steer no longer shifts the cut. A record that no longer
holds turn `k−1` refuses (`threads-unknown-turn`); a prompt the provider's
history no longer has refuses definitely-unsent ("Interface can't find that
message in the provider's history any more."). Codex: `thread/fork` with `lastTurnId` = the
provider id of turn k−1 (from `thread/turns/list` or the rollout). Claude:
`--resume <id> --fork-session --resume-session-at <uuid>` where `<uuid>` is the
last assistant message before the (k+1)-th genuine user prompt in the
transcript (tool results and meta entries are not prompts; count exactly as
the history projection counts user turns). *(Integration, 6 Oct.)* k counts
turns as the host records them: Codex counts turns holding an owner prompt (a
steered prompt belongs to its turn); Claude also skips the user messages the
CLI writes itself (`[Request interrupted by user]`, its `for tool use` form,
`!` bash echoes), which the history page still shows. Other engines, or a boundary that
cannot be resolved, refuse definitely-unsent. The new session inherits kind,
cwd and model; the host binds a record: active at top, lineage `{parent,
relation: fork, keptTurns: k}`, workspace copied with `owned: false`, title
"<parent title> (fork)". k = 0 is refused ("start a new thread instead").

**Edit from here** (`EditFrom ordinal = n`): (1) when `restoreFiles`, run the
restore checks and restore to turn n's start; (2) fork at `keepTurns = n`, recorded turns (n =
0: refused, the renderer starts a new thread in the same workspace instead);
(3) the new thread takes over worktree ownership (`owned` moves to it);
(4) the original is settled with reason `edited`; (5) answer the new key and
turn n's prompt text so the renderer can put it in the new thread's composer.
*(Fix, 6 Oct.)* After (4) the host closes the original's live provider
session (rechecked idle under its send gate; a failed close is logged and
changes nothing else), so its handle no longer holds the worktree against
the new thread's restore. Restore and release refusals name the other
session by title and say what frees the folder: settle an idle unsettled
thread (restore), archive a thread (release, or any thread whose session is
still open), or close a session that is not a thread. A new thread from Fork
from here or Edit from here starts in the permission mode its parent was
given in the pane (the renderer reads it before the command and applies it
to the new session); model and effort come with the registry copy.

**Auto-settle.** Every 10 minutes and at start: settle (reason `idle`) any
**active** (not pinned, not snoozed) thread with
auto-settle enabled whose session is idle, has no pending approval, and whose
newest of `lastRun.endedAt`, `createdAt`, `unsettledAt` is older than
`autoSettleDays`. With `settleOnMerge`, a tracked PR in state merged settles
the thread (reason `merged`) unless it ran after the merge.

**Pull requests** (`trackPullRequests`): for unsettled worktree threads with a
branch, `gh pr list --head <branch> --state all --json number,url,state --limit 1`
in the project root, at most every 10 minutes per thread and on
`RefreshPullRequest`. Missing or unauthenticated `gh` = silently nothing.

**Shipping a thread's work** *(protocol 37, 14 Oct; owner-approved B1).*
`shipStatus {key, remote}` reads the thread's checkout (`thread_git::ship`):
branch, remote, upstream ahead/behind, default branch, and what Commit
stages — every change in a worktree thread's own worktree; in a Local
thread only the changed files its finished turns' edits named (store v2's
`touched`), with `filesUnknown` when a turn's files are unknown (shell
commands' edits are never named); never a credential-shaped file. The
suggested message is the thread's title then its turns' prompts. `commit
{message, paths}` commits exactly `paths`, refused (`threads-ship-stale`)
unless they are still what Commit stages, while the agent works, or with a
merge in conflict; the repository's hooks run; other staged and unstaged
changes stay as they were, and new files go back out of the index when the
commit does not happen. A checkout on the default branch (or `main`/
`master` when that is unknown) is never pushed: `pushBlocked` explains and
a Local thread is offered `createBranch` (`git switch -c`, carrying its
changes; refused for a worktree thread). `openPullRequest {title, body}`
pushes (`--set-upstream`), then `gh pr list --head <branch> --state open`
first and records an open pull request instead of opening another, else
`gh pr create`; the pull request is recorded on the thread, which tracks
it from then on (worktree threads by branch, others by its number,
`PullRequestLookupBy`) — a pull request Interface opened even while
`trackPullRequests` is off (`pullRequestTracked`, a store v2 record field
added before release; absent means false). `failingChecksPrompt` answers the prompt the
renderer sends the thread: the failing checks (`gh pr checks --json`) and
the tail of each failed job's `gh run view --log-failed` (at most three, 80
lines / 6 KiB each, 16 KiB in all). `reviewComments` answers the unresolved
review threads (`gh api graphql`), which the renderer puts into the draft
the way a diff line comment goes. `mergePullRequest {number, method}` merges
only the pull request the owner confirmed, then records it merged and runs
auto-settle (the settle-on-merge path). Fail-closed: one ship effect per
thread at a time; a push, create or merge whose end is unclear is re-read
(`ls-remote`, `gh pr list`, `gh pr view`) and answered `done` when the
re-read finds it, refused when it agrees nothing happened, and `shipped:
{certainty: unknown}` when the re-read fails too — never retried. `gh`
missing, signed out or silent refuses with `threads-gh-unavailable` and the
fix (`brew install gh`, `gh auth login`); the status says the same in
`ghFix`. Every git and `gh` child is bounded and runs without Interface's or
a provider's secrets in its environment; what they print is redacted.

**Titles.** Plain chats keep the existing first-80-characters seed. Project
threads use the same seed at launch (`titleSource: prompt`); the runtime
seeds every Code launch (an omitted kind included) as it does Chat, so the
registry never shows the session id first, and a fork's
"<parent title> (fork)" starts from it. With
`generatedTitles` on, after the first completed run the host asks for a 3–8
word title through a one-shot, tool-less Claude call (the cheapest configured
Claude model), then renames through the existing rename operation
(`titleSource: generated`). `RegenerateTitle` does the same on demand. A user
rename sets `titleSource: user` and is never overwritten automatically.
*(Fix F2, 6 Oct.)* A Code session's prompt seed is a placeholder against the
provider's own title: the first listing whose title is the provider's
(`titleFromSummary`: Claude's summary or custom title; a Codex thread
`name`, now flagged) replaces it. Chat keeps its seed; owner and generated
titles are registry overrides and always win.
*(Integration, 6 Oct.)* The call is `claude -p --output-format json --model
haiku --no-session-persistence --tools "" --setting-sources= --safe-mode`
(no MCP, 30 s) over the first prompt and answer (2 KiB each) from the
provider transcript, else the stored launch prompt. The automatic title is
for project threads (plain chats keep their seed), once per thread per host
launch; a failure logs and keeps the title. `RegenerateTitle` replaces any
title, the owner's included, and fails definitely-unsent
(`threads-titles-unavailable` without Claude, `threads-title-failed`).
*(Owner, 15 Nov: "written by Haiku 5.5 like T3 Code does".)* `generatedTitles`
is on by default. A store written before that holds the old default,
`false`, which its owner never chose: the host (`ThreadHost::open_store`)
turns it on once through the store's own writer
(`ThreadStore::turn_generated_titles_on_once`) and records that in the
marker file `threads.generated-titles-on` beside `threads.v5.json`, written
under the store's lease before the change, so it never runs twice and an
owner's later off stays off; the document's schema is unchanged. The call
pins Claude Haiku 5.5 by its exact id, `--model claude-haiku-5-5` (Write
with AI shares it). A thread is named as its first prompt goes out, in
parallel with the turn (`wants_title_at_first_prompt`: on, title still the
prompt's seed, its one turn running): at `Bind` for a launch, at the first
delivered send for a thread created empty (team tools, outside agents). The
excerpt is then the launch prompt alone, and the instruction asks for the
task or subject of the first message. Only when that call fails is it asked
for once more after the first completed run, with the first answer; a call
still under way when the run completes retries by itself if it fails. Every
thread still named by its seed is titled, a chat with no project included:
the call runs in a neutral folder (the temp directory) and reads only the
excerpt, so a workspace was never needed. Owner and earlier generated
titles are never replaced (the title gate holds), and a switched thread
(chain) never takes one.

## Away loops as threads (protocol 45, review B3, 2026-10-14)

A project Away loop runs as a thread. Its first work round prepares the
loop's own worktree with `PrepareWorkspace worktree` (hint "away <objective>"),
creates the empty work session there (named "Away · <objective>") and binds it
with `ThreadsDocument::bind_empty`: the workspace and title source `user`, no
turn and no run; the prepared baseline is deleted, and the first send records
turn 0 on its own checkpoint. Every later work round is a send to that one
session, so each is a turn with its checkpoints, counts and diff; each review
is a fresh read-only session in the worktree that Threads leaves out, on
the loop's reviewer (protocol 47: the other vendor by default, named on each
review card)
(`HiddenSessions` hides every Away session except each loop's thread).

While the loop runs it holds the thread (`threads/away.rs`): the send hook lets
through only the prompt the loop permitted (by digest, once) and hands the loop
the ordinal of the turn it began, which the Away store keeps as the work
turn's `threadTurn`. No window echoes a prompt the host sent, so the runtime
dispatcher shows the pane that prompt as a live `user.message` before the
send (`SendTicket::echoes_prompt`); the renderer's merge treats it as it
treats any engine's live user message; any other send, `restoreTurn`, `editFrom` and
`openInTerminal` refuse with `threads-away-running`, definitely unsent. The
hold is in memory: a host restart marks the loop unknown (never resumed) and
the thread is the owner's.

`ThreadView.away` is joined at read from the Away store's projection
(`Service::thread_views`): state, detail, provider, model, objective, start,
deadline, iteration limit and rounds (role, state, a work round's turn, a
review's session, verdict, summary and checks from its `AWAY_RESULT`; texts cut
to 512 bytes, at most four checks of 200; only the newest 24 loops carry their
rounds). Away hands the thread host its projections after every commit; a
changed set bumps the store's revision and pushes. The renderer draws a work
round's prompt as "Work round N of M" with the instructions folded, the review
card under the round's changes, and the report at the end (Stop while it runs,
Review changes after); the inbox marks the thread with Away's coffee glyph
before its title (settled look, owner-approved 14 Oct).

## Bridges to Code and the Agent tab (protocol 42, review C1, 2026-10-14)

One provider session has one writer. `interface_runtime::SessionClaims`
answers who else holds a session: a running Code pane bound to its provider
id (`TerminalService::bound_provider_sessions`), or a hand-off to a pane that
has not started (30 s at most). The runtime dispatcher refuses every resume,
send and setting change for such a session, live or not, definitely unsent.
Moving a session ends the side it leaves first (no fork):

- **From terminals.** A session a Code pane or the CLI started is seeded
  settled `external`; Rust derives the section `fromTerminals` for it until it
  is brought into Threads. Settling it by hand later makes it Settled. The
  renderer draws it with Settled until the inbox gives it a shelf (an owner
  look decision).
- **Open in terminal** (`openInTerminal`, row menu and pane `⋯`): refused
  mid-turn and while a pane holds the session; Rust closes the structured
  session (an unconfirmed close holds nothing and is not replay-safe), holds
  the session for the pane and answers `terminal`. The renderer opens a new
  Code workspace whose pane resumes it (`claude --resume`, `codex resume`).
- **Continue as thread** (`continueFromTerminal`, a Claude or Codex pane's
  `···`, between turns): the renderer ends the pane, lists sessions, then Rust
  waits up to 5 s for no running pane to be bound before it un-settles and
  visits the session; the renderer opens it in Threads.
- **Hand to teammate** (row menu): the teammate's gated `startAgentTask`
  (protocol 38) in the thread's folder, with the thread's title, folder,
  branch and key as the task. The teammate runs its own session; the thread is
  left as it is.

## Second opinion and Continue with (protocol 46, review B4, 2026-10-14)

- **Second opinion from <provider>** (`secondOpinion {key, engine,
  requestId}`, row menu and pane `⋯`): an ordinary human consult through the
  durable collaboration service. Refused for the thread's own provider, a
  session a Code pane holds or is opening for (`threads-in-terminal`), and
  mid-turn (`threads-session-busy`), definitely unsent. A paused session is
  resumed through the runtime dispatcher first (its claims and permission
  mode apply). The objective asks for a brief second opinion and lists the
  latest turn's changes (`interface_collaboration::thread_handoff`, inside
  the objective's 4,000 characters); the packet adds the recent
  conversation, the latest answer included. The answer's `consult` waits for
  the owner's approval of the exact packet; the other provider runs
  read-only; its reply is held for the owner, who sends it on into the
  thread (the consult's advisory, a turn of the thread's provider) or
  discards it. The collaboration port refuses every preview and effect that
  would write to a session a pane holds.
- **Continue with <provider>** (`continueWith {key, engine}`, project
  threads only, between turns): Rust prepares the same folder (Local) or the
  same worktree (`existing`, not owned) for a new thread, with a pending
  lineage `{parent, relation: handoff, keptTurns: 0}` on the prepared
  workspace, and answers its first prompt: who it takes over from, the
  thread's changes and the newest conversation that fits (32 KiB). The
  renderer launches it on `engine` like a New thread card launch and binds
  it with the token; Bind records the lineage. The original thread is left
  as it is. The coordinator's handoff mode is not used: it parks the source
  and drives its target outside the run tap, permission modes and session
  claims.
- Store version 3 (`threads.v3.json`) holds the new relation; a v2 file
  (else v1) is read once and carried over, never changed.
- Renderer: the pane `⋯` and the inbox row menu list "Second opinion from
  <provider>" (ready providers that advertise supervised consultation) and
  "Continue with <provider>" (ready providers, project threads only), held
  while a turn runs. A consult on the thread is a card above its composer
  (`ThreadOpinionCards`): what goes out (size, the packet behind "What is
  sent") with Ask / Cancel, "<provider> is reading the thread…", then the
  reply on an inset, attributed and marked "Not sent to <thread's provider>",
  with Send to <provider> / Dismiss. A continued thread's head says
  "Continued from <parent title>".
- Usage headroom (review E): the composer's model chip (Threads pane, New
  thread card, Agent composer, the pane status line) and the launcher's agent
  tiles say the provider's headroom ("38% left") from the fullest reported
  window; when a thread's provider reports its limit, a quiet line above the
  composer offers "Continue with <provider>" (the one with the most headroom).
  Nothing continues by itself.

## Renderer

**Naming.** The surface switch reads Agent / Code / Threads. Section label
"Threads", create button "New thread", empty text "No threads yet.", intro
"Start a thread" / "Ask a question, or pick a project and give an agent work.".
Internal identifiers (`"chat"` surface id, storage keys) stay as they are.

**Sidebar (Threads surface only; Code and Agent unchanged).**
- Header row as today (label, +, search, filter) plus a project scope picker
  ("All projects", each project, "No project").
- Sections in order: Pinned, Active, Snoozed (collapsed by default, shows wake
  time on hover), Settled (collapsed by default, newest settled first, shows
  the first 50 with "Show more"). Empty sections are hidden.
- Rows stay one line (settled look): title, then a muted project name, then
  the status mark. Status mark ladder, highest wins: approval / question
  (needs you) > working (existing wave/ticker) > failed > unread done (the
  existing blue done dot) > woke (snoozed thread back) > none.
- Drag to reorder within Pinned and Active; drag across sections pins, settles
  or wakes. Undo for settle, snooze, archive: a short toast with Undo, and
  ⌘Z while the sidebar has focus.
- Row menu: Pin/Unpin, Settle/Un-settle, Snooze ▸ (1 hour, Tonight 6 pm,
  Tomorrow 9 am, Next Monday 9 am), Mark unread, Rename, Regenerate title,
  Auto-settle ▸ Enabled/Disabled, Archive, plus the existing items.
- Selecting a thread sends `Visit` (also when its run ends while it is the
  focused pane of a focused window).
- Keyboard: ⌘⇧[ / ⌘⇧] previous/next thread in sidebar order; ⌘⇧S settle;
  ⌘⇧P pin (only if not already bound — otherwise pick free chords and record
  them); ⌘D diff panel; ⌘⌥Enter send and start another new thread.
- Notifications: when a thread you are not looking at finishes, fails, or
  needs approval/an answer, reuse the existing attention path (tray + any
  existing OS notification); no new notification system.

**New thread card** (Composer on the Threads surface).
- Project picker first: recent projects (project roots of known threads and
  project pins), "Choose folder…", "No project". Default: the scope filter's
  project, else the last used one.
- No project → today's chat card exactly (Claude-only, tool-less).
- A project → every engine, model picker, permission mode, and a workspace
  control: Local (default) / New worktree (with a base branch picker from
  `ListBranches`) / an existing worktree (`ListWorktrees`). The last workspace
  mode is remembered per project (renderer preference). *(Fix F4.)* Only
  Local and New worktree are remembered: an existing worktree (the card Edit
  from the first message seeds) is a one-off, and a stored one reads as
  nothing remembered.
- Launch: `PrepareWorkspace` → existing launch path with `cwd` and kind
  `code` → `Bind` (or `DiscardWorkspace` if the launch failed).
- Multi-model: shift-click several models (project + New worktree only) starts
  one thread per model, each in its own worktree. *(Fix F4.)* Each thread of
  a fan-out is launched with the name "<first 60 characters of the prompt,
  whitespace collapsed> · <model display name>", so the rows differ.
- ⌘Enter on the card starts the thread in the background and leaves a fresh
  card open with the same project and workspace mode.

**Thread view.**
- Pane head shows the branch chip (worktree threads) and a Diff button.
- Diff panel (right column, same slot as the subagent panel): scope tabs
  Turn N / Thread / Uncommitted / Branch, file list with +/−, patch view.
- User turn rows: the change summary opens that turn's diff; "Edit from here"
  opens a small dialog: "Revert and keep changes" / "Revert files too" (the
  latter disabled with Rust's reason when not allowed).
- Assistant turn ends: "Fork from here".
- Turn rows map to `turns[]` by prompt digest (k-th occurrence of a digest ↔
  k-th turn with it). No match → no control (never a wrong one).
- Local-mode threads keep today's "Undo · N files" control unchanged.
- Lineage: a forked thread shows "Forked from <parent title>" in its head
  (click opens the parent).
- *(Fix, 6 Oct.)* The dialog is titled "Edit from here" and offers "Keep
  the current files" / "Restore the files to before this message"; it says
  a new thread starts from the earlier message and this one is settled.
  Edit from the first message goes in steps in the renderer: `RestoreTurn 0`
  when restoring (the recovery ref is shown as "Recovery point saved"; *Fix
  F4:* the New thread card that opens over the pane also says "Files
  restored to before this message — recovery point saved"),
  `Settle`, the original's live session closed, then the card on the same
  worktree in `existing` mode (a missing worktree opens New worktree, never
  Local; Bind hands a settled owner's worktree over). A card closed without
  starting leaves the original settled, and the dialog says it un-settles
  from its menu. Turn rows map only where matched ordinals strictly increase
  down the transcript; Claude's interrupt markers and `!` echoes never take
  a control. One Fork or Edit per pane at a time, and the new thread's pane
  takes focus. Every way a thread becomes the focused pane sends `Visit`.
  The row menu's Move up / Move down reorder Pinned and Active. Archive
  offers Undo only after it succeeded. Extra models start after the first
  thread, without the launch lock, and a failed one is reported.

**Phone.** `electron/src/remote` and the native app keep today's
project-grouped lists. Shared helpers must keep their current output for the
phone callers.

## Acceptance (each with a pinned test)

1. Wire: fixture round-trips in Rust and TS; counts and protocol 34 asserted.
2. Store: derivation table tests (sections, woke, unread, ordering, settle
   clears pin), bounds, quarantine, seeding rules.
3. Host: run tap stamps start/end; auto-settle rules (blocked while working
   or waiting); push after every change.
4. Git: capture never changes `git status` or the index; turn diff; restore
   refused when shared and allowed when isolated; worktree create, recreate
   after removal, discard, release keeps dirty worktrees and all branches.
5. Fork: Codex `lastTurnId` and Claude `--resume-session-at` arguments from
   fake CLIs; unsupported engines refuse definitely-unsent.
6. Renderer: inbox sections/ordering from a view; status ladder; row menu
   commands; new-thread card modes; diff panel; edit/fork controls; prompt
   digest mapping; plain chat unchanged; Agent surface unchanged.
7. Hands-free app run: create a project thread (Local and New worktree) with
   the fake provider, see it top of Active, finish → unread dot, visit →
   cleared, settle/undo, snooze, pin/reorder, diff panel opens.
