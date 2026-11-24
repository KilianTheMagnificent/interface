import type { WorkflowCommand, WorkflowView, SetupReport } from "./workflows.js";
import type { EngineId } from "./engine.js";
import type { CollaborationEngine, CollaborationStatusDto } from "./ipc.js";

// Threads surface v1 (protocol 34): the `iface:threadControl` command and
// response, and the `ThreadsView` the `iface:threads` push carries. Mirrors
// crates/interface-contracts/src/threads.rs; both sides round-trip
// src/fixtures/threads-v34.json and, for protocol 36's turn ends,
// src/fixtures/threads-v36.json; protocol 37's ship commands (status, commit,
// branch, push and pull request, checks, review comments, merge) round-trip
// src/fixtures/threads-v37.json; protocol 46's second opinion and Continue
// with round-trip src/fixtures/threads-handoff-v46.json; protocol 59's switch
// provider round-trips src/fixtures/threads-switch-v59.json; protocol 62's
// scheduled prompts and pull-request watch round-trip
// src/fixtures/threads-schedules-v62.json; protocol 66's resume at reset
// round-trips src/fixtures/threads-resume-v66.json. Rust derives every section, unread flag,
// order key and restore permission; the renderer only draws the view and sends
// commands. Times are milliseconds since the Unix epoch. A thread's key is its
// session key (`engine#sessionId`).

import type { ThreadAway } from "./away.js";

/** `fromTerminals` (protocol 42): a Code pane or CLI session never brought into Threads. */
export type ThreadSection = "pinned" | "active" | "snoozed" | "settled" | "fromTerminals";
export type SettledReason = "manual" | "idle" | "merged" | "edited" | "external";
/** `none` is plain chat ("No project"). */
export type WorkspaceMode = "local" | "worktree" | "none";
/** Why an archive, delete or forget kept a thread's own worktree. */
export type WorkspaceKept = "dirty" | "ignored" | "inUse";
export type RunOutcome = "completed" | "failed" | "interrupted";
export type TurnState = "running" | "done" | "failed" | "interrupted";
/** `handoff` (protocol 46): a new thread on another provider continuing this one (Continue with). */
/** Protocol 57 adds `child`: a thread another thread's agent started through team tools. */
export type LineageRelation = "fork" | "edit" | "handoff" | "child";
export type TitleSource = "prompt" | "generated" | "user";
export type PullRequestState = "open" | "merged" | "closed";
export type DiffScope = "turn" | "thread" | "uncommitted" | "branch";
export type DiffFileStatus = "added" | "modified" | "deleted" | "renamed";
export type DiffOmitted = "sensitive" | "large" | "binary";
/** `prepareWorkspace` modes; `existing` requires `worktreePath`. */
export type PrepareWorkspaceMode = "local" | "worktree" | "existing";
/** What the GitHub CLI can do for a thread (protocol 37). */
export type GhState = "ready" | "missing" | "signedOut" | "unavailable";
/** `gh pr checks`' own grouping of a check's state. */
export type CheckBucket = "pass" | "fail" | "pending" | "skipping" | "cancel";
/** Failing if any failed, else pending if any run, else passing; none without checks. */
export type ChecksState = "passing" | "failing" | "pending" | "none";
export type MergeMethod = "squash" | "merge" | "rebase";
export type DiffSide = "new" | "old";
export type ShipAction = "commit" | "createBranch" | "openPullRequest" | "merge";
/** A refused effect is an error; only what happened, or what could not be told, answers. */
export type ShipCertainty = "done" | "unknown";
export type LeftOutReason = "credential" | "notThisThread";

export interface CommitReview { revision: string; files: CommitReviewFile[] }
export interface CommitReviewFile {
  path: string; status: DiffFileStatus; attributed: boolean;
  hunks: { id: number; patch: string }[]; blocked?: string;
}
export interface CommitChoice { path: string; hunks?: number[] }

export interface ThreadWorkspace {
  mode: WorkspaceMode;
  /** `""` for mode `none`. */
  projectRoot: string;
  /** Mode `worktree` only. */
  worktreePath?: string;
  branch?: string;
  baseRef?: string;
  /** This thread created the worktree. */
  owned: boolean;
  /** The worktree directory is absent (derived at read). */
  missing?: boolean;
  /**
   * Archiving, deleting or forgetting this thread's session kept the worktree
   * it owns, and why (derived at read). Cleared when a later release succeeds
   * or the session is unarchived.
   */
  kept?: WorkspaceKept;
}

export interface ThreadLineage { parent: string; relation: LineageRelation; keptTurns: number; at: number }

export interface ThreadRun { startedAt: number; endedAt?: number; outcome?: RunOutcome }

export interface ThreadTurn {
  ordinal: number;
  sentAt: number;
  /** First 16 hex of sha256(normalized prompt). */
  promptDigest: string;
  hasStart: boolean;
  hasEnd: boolean;
  state: TurnState;
  /**
   * When the turn's run ended (protocol 36). Absent while it runs, for a turn
   * recorded before protocol 36, and for one closed without an end Rust saw.
   */
  endedAt?: number;
  filesChanged?: number;
  added?: number;
  removed?: number;
}

export interface ThreadPullRequest { url: string; number: number; state: PullRequestState; checkedAt: number }

export interface ThreadView {
  key: string;
  section: ThreadSection;
  createdAt: number;
  pinnedAt?: number;
  pinOrder?: string;
  activeOrder?: string;
  settledAt?: number;
  settledReason?: SettledReason;
  snoozedUntil?: number;
  woke: boolean;
  unread: boolean;
  lastVisitedAt?: number;
  lastRun?: ThreadRun;
  autoSettle: boolean;
  workspace?: ThreadWorkspace;
  lineage?: ThreadLineage;
  pullRequest?: ThreadPullRequest;
  titleSource?: TitleSource;
  turns: ThreadTurn[];
  /** Protocol 45: the Away loop this thread runs as; absent for every other thread. */
  away?: ThreadAway;
  /**
   * Protocol 59: the provider sessions this thread ran on, when its provider
   * was switched; absent for every other thread. The head (this thread's own
   * key) is last.
   */
  chain?: ThreadChain;
  /** Protocol 60: the added provider account its session is bound to (absent =
   *  the inherited Default). An id the accounts view lacks was removed. */
  account?: string;
  /**
   * Protocol 62: the thread's scheduled prompts (soonest first) and its
   * pull-request watch; absent when it has neither.
   */
  schedules?: ThreadSchedules;
  /**
   * Protocol 65: an agent outside Interface started this thread through
   * `iface mcp`; the name its MCP client declared (display only).
   */
  startedFromOutside?: string;
  /**
   * Protocol 66: the thread's provider hit its usage limit since the thread
   * last completed a turn; absent otherwise.
   */
  limit?: ThreadLimit;
  /** Protocol 67: the secret this thread's agent asks the owner for, while it waits. Never a value. */
  secretRequest?: import("./thread-secrets.js").ThreadSecretRequest;
  /** Protocol 67: the side question open on this thread (at most one). */
  side?: ThreadSide;
  /**
   * Protocol 72: the thread runs on Claude in a folder nobody has trusted,
   * so it runs without that folder's own settings and instruction file.
   */
  folderTrust?: ThreadFolderTrust;
}

/** Protocol 72: a Claude thread's folder is not trusted. */
export interface ThreadFolderTrust {
  /** The folder a trust answer covers (its repository's top folder), to show. */
  folder: string;
  /** false: nobody has answered for this folder yet; true: the owner said Not now. */
  asked: boolean;
}

/** Protocol 72: where a folder stands. */
export type FolderTrustState = "trusted" | "unasked" | "declined";

/** Protocol 72: the answer to `folderTrust` and `trustFolder`. */
export interface FolderTrustAnswer {
  /** The folder the answer covers (the trust root). */
  folder: string;
  state: FolderTrustState;
}

/** A side question (protocol 67): a hidden fork of the thread asked one question. */
export interface ThreadSide {
  /** The side thread's own session key; its events carry the answer. */
  key: string;
  question: string;
  askedAt: number;
}

/** Protocol 66: a thread's provider hit its usage limit. */
export interface ThreadLimit {
  /** When it resets (UTC ISO, the usage windows' form); absent when unknown, never guessed. */
  resetsAt?: string;
}

/** Protocol 66: `resume` is Resume at reset (one per thread); absent = a prompt. */
export type ScheduleKind = "prompt" | "resume";

/** When a scheduled prompt runs (protocol 62). Times are ms since the epoch. */
export type ScheduleWhen =
  | { kind: "once"; at: number }
  /** Every `minutes` (at least 5) from its last due time. */
  | { kind: "interval"; minutes: number }
  | { kind: "calendar"; calendar: ScheduleCalendar };

/** `hour:minute` in `timezone` on ISO `weekdays` (Monday 1; empty = every day). */
export interface ScheduleCalendar { timezone: string; weekdays: number[]; hour: number; minute: number }

/** What became of one firing; `unknown` is never sent again. */
export type ScheduleOutcome = "sent" | "skipped" | "unknown" | "refused";

export interface ScheduleRun { occurrence: number; at: number; outcome: ScheduleOutcome; detail?: string }

export type ScheduleSetBy = "owner" | "agent";

export interface ThreadScheduleView {
  id: string;
  name?: string;
  prompt: string;
  when: ScheduleWhen;
  enabled: boolean;
  createdAt: number;
  /** Absent once a one-time schedule has run. */
  nextAt?: number;
  lastRun?: ScheduleRun;
  /** A firing waiting for the thread's turn to end (its occurrence). */
  waiting?: number;
  setBy: ScheduleSetBy;
  /** The thread is settled: nothing is sent until it is active again. */
  paused: boolean;
  /** Protocol 66: `resume` for Resume at reset; absent for a prompt. */
  kind?: ScheduleKind;
}

export interface ThreadWatchView {
  enabled: boolean;
  /** `gh` is missing or signed out: the watch sends nothing. */
  needsGh: boolean;
  failingChecks: number;
  reviewThreads: number;
  observedAt?: number;
  lastRun?: ScheduleRun;
  waiting: boolean;
}

export interface ThreadSchedules { schedules: ThreadScheduleView[]; watch?: ThreadWatchView }

/** A switched thread's provider sessions, oldest activation first (protocol 59). */
export interface ThreadChain { segments: ThreadChainSegment[] }

export interface ThreadChainSegment {
  key: string;
  engine: EngineId;
  /** Activations: 0 for the first, one more each time a segment became the head. */
  order: number;
  firstAt: number;
  lastAt: number;
  /**
   * Protocol 69: the lines the segment's own turns changed, summed over the
   * turns that measured both counts; both absent when none did. The head's
   * are its own turns' totals.
   */
  added?: number;
  removed?: number;
}

/** Whether a `switchProvider`'s handoff prompt reached the new head (protocol 59). */
export type HandoffDelivery = "sent" | "unsent" | "unknown";

export interface ThreadSettings {
  /** Default 3; 0 = never. */
  autoSettleDays: number;
  /** Default true. */
  settleOnMerge: boolean;
  /**
   * Default true: Claude Haiku 5.5 names a thread from its first prompt, on a
   * little of the owner's Claude plan. Rust turns an older store's `false`
   * (the old default) on once; an owner's later off stays off.
   */
  generatedTitles: boolean;
  /** Default true: needs `gh`, silent when absent. */
  trackPullRequests: boolean;
  /**
   * Protocol 57: Threads agents get team tools (start, read, wait for and
   * message other threads). Absent means on; Rust writes only `false`.
   */
  teamTools?: boolean;
  /**
   * Protocol 65: what a Claude Code or Codex outside Interface may do through
   * `iface mcp`. Absent means off; Rust writes only `read` or `full`, and
   * keeps the level in the team tools ledger, not the Threads store.
   */
  outsideAgents?: OutsideAgents;
  /**
   * Protocol 71: an agent in a Claude or Codex pane may type into another
   * pane or start one (`send_to_pane`, `pass_off`) without asking. Absent
   * means off; Rust writes only `true`, and keeps it beside the Threads
   * store, not in it.
   */
  paneToolsWithoutAsking?: boolean;
}

/** Protocol 65: nothing, read threads, or also start and message threads. */
export type OutsideAgents = "off" | "read" | "full";

export interface ThreadsView {
  version: 1; revision: number; now: number; settings: ThreadSettings; threads: ThreadView[];
  /**
   * Protocol 58 (src/fixtures/threads-hidden-v58.json): the registry's
   * sessions Threads leaves out as workers (orchestration workers, from the
   * whole agent list, and Away's reviews), sorted. Chat's session list leaves
   * them out too. Absent when there are none.
   */
  hiddenKeys?: string[];
}

export interface PreparedWorkspace { token: string; cwd: string; workspace: ThreadWorkspace }
export interface BranchList { current?: string; defaultBranch?: string; branches: string[] }
export interface WorktreeEntry { path: string; branch?: string; ownerKey?: string }
export interface DiffFile {
  path: string;
  oldPath?: string;
  status: DiffFileStatus;
  added: number;
  removed: number;
  binary: boolean;
  patch?: string;
  omitted?: DiffOmitted;
}
export interface ThreadDiff { scope: DiffScope; base: string; head: string; files: DiffFile[]; truncated: boolean }
export interface RestoreResult { restored: number; recoveryRef: string }

/** A file Commit would stage, relative to the repository root. */
export interface ShipFile { path: string; status: DiffFileStatus }
/** A changed file Commit leaves out, and why. */
export interface ShipLeftOut { path: string; reason: LeftOutReason }
export interface ShipPullRequest {
  number: number;
  url: string;
  state: PullRequestState;
  title: string;
  /** The branch it merges into. */
  base: string;
  draft: boolean;
}
export interface ShipCheck { name: string; workflow?: string; bucket: CheckBucket; link?: string }
export interface ShipChecks {
  state: ChecksState;
  passed: number;
  failed: number;
  pending: number;
  /** Skipped and cancelled. */
  skipped: number;
  /** The failing checks, at most 20. */
  failing: ShipCheck[];
}
/** A thread's checkout, read for shipping its work (protocol 37). */
export interface ThreadShip {
  key: string;
  /** Absent on a detached HEAD. */
  branch?: string;
  defaultBranch?: string;
  remote?: string;
  upstream?: string;
  ahead?: number;
  behind?: number;
  /** What Commit stages: a worktree thread's every change; a Local thread's own files. Never credentials. */
  files: ShipFile[];
  leftOut: ShipLeftOut[];
  /** A Local thread has turns whose files are unknown, so Commit may leave some of its changes out. */
  filesUnknown: boolean;
  /** Why Commit cannot run now. */
  commitBlocked?: string;
  /** The suggested message: the thread's title and its turns' prompts. */
  message: string;
  /** Why Push & open PR is not offered. */
  pushBlocked?: string;
  /** On the default branch: a branch name to create first. */
  suggestedBranch?: string;
  gh: GhState;
  /** How to make `gh` work, when it does not. */
  ghFix?: string;
  /** The remote parts (`gh`) were read. */
  remoteRead: boolean;
  pullRequest?: ShipPullRequest;
  checks?: ShipChecks;
}
/** How a ship effect ended, when it did not fail outright. */
export interface ShipOutcome { action: ShipAction; certainty: ShipCertainty; message: string }
/** An unresolved pull request review thread, for the thread's draft. */
export interface ReviewComment {
  path: string;
  startLine?: number;
  /** Absent for a comment on the whole file. */
  endLine?: number;
  side: DiffSide;
  /** The commented lines from the review's diff hunk ("" when unknown). */
  code: string;
  /** Every comment in the thread, "@author: body", credentials redacted. */
  comment: string;
  url: string;
  /** The lines changed since the comment was made. */
  outdated: boolean;
}

export interface ThreadControlResponse {
  workflow?: WorkflowView;
  setup?: SetupReport;
  view: ThreadsView;
  prepared?: PreparedWorkspace;
  branches?: BranchList;
  worktrees?: WorktreeEntry[];
  diff?: ThreadDiff;
  restore?: RestoreResult;
  /** The new thread's key (`forkAt` / `editFrom`). */
  key?: string;
  /** `editFrom`: the edited turn's prompt text when known; `failingChecksPrompt`: the prompt to send. */
  prompt?: string;
  /** The thread's checkout for shipping (protocol 37). */
  ship?: ThreadShip;
  /** How a ship effect ended (protocol 37). */
  shipped?: ShipOutcome;
  commitReview?: CommitReview;
  /** `reviewComments`: the pull request's unresolved review threads. */
  reviewComments?: ReviewComment[];
  /** `openInTerminal` (protocol 42): the Code pane to open, resuming the session. */
  terminal?: ThreadTerminalHandoff;
  /** `secondOpinion` (protocol 46): the consult it submitted, as the collaboration channels report it. */
  consult?: CollaborationStatusDto;
  /** `switchProvider` (protocol 59): whether the handoff reached the new head (`key`). */
  handoff?: HandoffDelivery;
  /** `suggestShipText` (protocol 67): the text Claude wrote, for the owner to edit. */
  shipText?: string;
  /** `folderTrust` and `trustFolder` (protocol 72): where the folder stands now. */
  folderTrust?: FolderTrustAnswer;
}

/** Protocol 67 (T3 section 12): which text Write with AI writes. */
export type ShipTextKind = "commit" | "pullRequest";

/** Where and how a Code pane resumes a thread's provider session (protocol 42). */
export interface ThreadTerminalHandoff {
  profileId: "claude" | "codex";
  /** The provider's own session id. */
  providerSessionId: string;
  /** The folder the session runs in (a worktree thread's worktree). */
  cwd: string;
}

export type ThreadCommand =
  | { kind: "workflow"; command: WorkflowCommand }
  | { kind: "read" }
  | { kind: "pin"; key: string }
  | { kind: "unpin"; key: string }
  /** Place `key` before `beforeKey` in `section` (pinned|active); absent = end. */
  | { kind: "reorder"; key: string; section: ThreadSection; beforeKey?: string }
  | { kind: "settle"; key: string }
  | { kind: "unsettle"; key: string }
  | { kind: "snooze"; key: string; until: number }
  | { kind: "unsnooze"; key: string }
  | { kind: "visit"; key: string }
  | { kind: "markUnread"; key: string }
  | { kind: "setAutoSettle"; key: string; enabled: boolean }
  | { kind: "setSettings"; settings: ThreadSettings }
  | {
    kind: "prepareWorkspace"; projectRoot: string; mode: PrepareWorkspaceMode;
    baseRef?: string; worktreePath?: string; hint: string;
  }
  | { kind: "discardWorkspace"; token: string }
  /** After a launch answered `key`. `token` from prepareWorkspace; absent for plain chat. */
  | { kind: "bind"; key: string; token?: string; prompt: string; titleSource?: TitleSource }
  | { kind: "listBranches"; projectRoot: string }
  | { kind: "listWorktrees"; projectRoot: string }
  | { kind: "turnDiff"; key: string; ordinal: number }
  | { kind: "threadDiff"; key: string; scope: DiffScope }
  | { kind: "restoreTurn"; key: string; ordinal: number }
  | { kind: "forkAt"; key: string; keepTurns: number }
  | { kind: "editFrom"; key: string; ordinal: number; restoreFiles: boolean }
  | { kind: "releaseWorkspace"; key: string }
  | { kind: "regenerateTitle"; key: string }
  | { kind: "refreshPullRequest"; key: string }
  /** Protocol 37: the checkout for shipping; `remote` also asks `gh`. */
  | { kind: "shipStatus"; key: string; remote: boolean }
  /** Commits exactly `paths`, the files the last status listed. */
  | { kind: "commit"; key: string; message: string; paths: string[] }
  | { kind: "reviewCommit"; key: string }
  | { kind: "commitSelected"; key: string; revision: string; message: string; choices: CommitChoice[] }
  /** A Local thread's checkout switches to a new branch at HEAD. */
  | { kind: "createBranch"; key: string; branch: string }
  /** Pushes, then opens the branch's pull request unless one is open. */
  | { kind: "openPullRequest"; key: string; title: string; body: string }
  | { kind: "failingChecksPrompt"; key: string }
  | { kind: "reviewComments"; key: string }
  /** Merges pull request `number`, which the owner confirmed. */
  | { kind: "mergePullRequest"; key: string; number: number; method: MergeMethod }
  /**
   * Protocol 42: closes the thread's structured session and holds it for a
   * Code pane (the answer's `terminal`). Refused while a turn runs or a pane
   * already holds it.
   */
  | { kind: "openInTerminal"; key: string }
  /** Protocol 42: brings a Code pane or CLI session into Threads, once no pane runs it. */
  | { kind: "continueFromTerminal"; key: string }
  /**
   * Protocol 46: a consult with `engine` on the thread's latest answer and its
   * turn's changes; the answer's `consult` waits for the owner's approval.
   */
  | { kind: "secondOpinion"; key: string; engine: CollaborationEngine; requestId: string }
  /**
   * Protocol 46: the thread's folder prepared again for `engine` (`prepared`)
   * and the new thread's first `prompt`; launch it and bind it with the token.
   * Protocol 60: `account` continues on that account of `engine`, which may
   * then be the thread's own provider (another account of it).
   */
  | { kind: "continueWith"; key: string; engine: EngineId; account?: string }
  /**
   * Protocol 59: carry on in the same thread on `engine` (back to its earlier
   * session when there is one). The answer's `key` is the new head and
   * `handoff` says whether its handoff arrived.
   */
  | { kind: "switchProvider"; key: string; engine: EngineId; model?: string }
  /**
   * Protocol 62: schedules `prompt` for thread `key` (or, with `id`, changes
   * that schedule). Refused: an interval under 5 minutes, a time in the past.
   */
  | { kind: "scheduleSet"; key: string; id?: string; name?: string; prompt: string; when: ScheduleWhen }
  | { kind: "scheduleDelete"; id: string }
  | { kind: "scheduleEnable"; id: string; enabled: boolean }
  /** Protocol 62: the thread's pull-request watch (on needs an open pull request). */
  | { kind: "watchSet"; key: string; enabled: boolean }
  /**
   * Protocol 66: Resume at reset. Thread `key` carries on by itself a minute
   * after its limit resets (the reset Rust recorded), replacing its earlier
   * resume; `prompt` is the owner's queued follow-up that never went out.
   * Refused when the reset is not known.
   */
  | { kind: "resumeAtReset"; key: string; prompt?: string }
  /**
   * Protocol 67 (T3 section 12): Write with AI. One tool-less Claude call
   * writes a commit message (`paths`: the files being committed) or a pull
   * request's text, answered as `shipText`. Nothing is committed or pushed.
   */
  | { kind: "suggestShipText"; key: string; what: ShipTextKind; paths?: string[] }
  /**
   * Protocol 67: asks `question` without adding to the thread (a hidden,
   * read-only fork at its latest turn); the answer's `key` is the side thread.
   */
  | { kind: "sideAsk"; key: string; question: string }
  /** Protocol 67: closes the thread's side question and removes its side thread. */
  | { kind: "sideClose"; key: string }
  /** Protocol 72: where `folder` (absolute) stands for a new Claude thread. A read. */
  | { kind: "folderTrust"; folder: string }
  /**
   * Protocol 72: records the owner's answer for `folder`'s trust root
   * (`trust: false` declines it; a trusted folder stays trusted).
   */
  | { kind: "trustFolder"; folder: string; trust: boolean };
