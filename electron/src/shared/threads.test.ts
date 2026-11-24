import { describe, expect, it } from "vitest";
import fixture from "../fixtures/threads-v34.json";
import fixtureV36 from "../fixtures/threads-v36.json";
import fixtureV37 from "../fixtures/threads-v37.json";
import fixtureV42 from "../fixtures/threads-v42.json";
import fixtureV45 from "../fixtures/threads-v45.json";
import fixtureV49 from "../fixtures/threads-commit-v49.json";
import workflowV49 from "../fixtures/workflows-v49.json";
import fixtureV46 from "../fixtures/threads-handoff-v46.json";
import fixtureV57 from "../fixtures/threads-team-v57.json";
import fixtureV58 from "../fixtures/threads-hidden-v58.json";
import fixtureV59 from "../fixtures/threads-switch-v59.json";
import fixtureV62 from "../fixtures/threads-schedules-v62.json";
import fixtureV65 from "../fixtures/outside-agents-v65.json";
import fixtureV66 from "../fixtures/threads-resume-v66.json";
import fixtureV67 from "../fixtures/quick-wins-v67.json";
import fixtureV69 from "../fixtures/threads-totals-v69.json";
import fixtureV71 from "../fixtures/ask-first-v71.json";
import fixtureV72 from "../fixtures/folder-trust-v72.json";
import type { AwayRoundState, AwayVerdict, ThreadAway, ThreadAwayRound } from "./away";
import { encodeRendererPayload } from "../main/ipc-payload";
import { RUST_HOST_PROTOCOL_VERSION } from "../main/rust-host-client";
import { CHANNELS } from "./ipc";
import type {
  BranchList, CheckBucket, ChecksState, DiffFile, DiffFileStatus, DiffOmitted, DiffScope, DiffSide,
  GhState, LeftOutReason, LineageRelation, MergeMethod, OutsideAgents, PreparedWorkspace, PullRequestState,
  RestoreResult, ReviewComment, RunOutcome, SettledReason, ShipAction, ShipCertainty, ShipCheck,
  ShipChecks, ShipFile, ShipLeftOut, ShipOutcome, ShipPullRequest, ThreadChain, ThreadChainSegment,
  ThreadCommand, HandoffDelivery,
  ThreadControlResponse, ThreadDiff, ThreadLineage, ThreadPullRequest, ThreadRun, ThreadSection,
  ThreadSettings, ThreadShip, ThreadTerminalHandoff, ThreadTurn, ThreadView, ThreadWorkspace,
  ThreadsView, TitleSource, ScheduleCalendar, ScheduleOutcome, ScheduleRun, ScheduleSetBy,
  ThreadScheduleView, ThreadSchedules, ThreadWatchView, ThreadLimit, ScheduleKind,
  FolderTrustAnswer, FolderTrustState, ThreadFolderTrust,
  TurnState, WorkspaceKept, WorkspaceMode, WorktreeEntry,
} from "./threads";

// Each field of T, flagged true when required and false when optional. The
// annotation is checked by tsc both ways: a missing, extra or wrongly flagged
// field is a compile error, so these tables are the TS types spelled as data.
type Fields<T> = { [K in keyof T]-?: undefined extends T[K] ? false : true };
// Every member of a string union, as data the compiler checks is exhaustive.
type Members<T extends string> = Record<T, true>;

const RUN: Fields<ThreadRun> = { startedAt: true, endedAt: false, outcome: false };
const TURN: Fields<ThreadTurn> = {
  ordinal: true, sentAt: true, promptDigest: true, hasStart: true, hasEnd: true, state: true,
  endedAt: false, filesChanged: false, added: false, removed: false,
};
const WORKSPACE: Fields<ThreadWorkspace> = {
  mode: true, projectRoot: true, worktreePath: false, branch: false, baseRef: false,
  owned: true, missing: false, kept: false,
};
const LINEAGE: Fields<ThreadLineage> = { parent: true, relation: true, keptTurns: true, at: true };
const PULL_REQUEST: Fields<ThreadPullRequest> = { url: true, number: true, state: true, checkedAt: true };
const THREAD: Fields<ThreadView> = {
  key: true, section: true, createdAt: true, pinnedAt: false, pinOrder: false, activeOrder: false,
  settledAt: false, settledReason: false, snoozedUntil: false, woke: true, unread: true,
  lastVisitedAt: false, lastRun: false, autoSettle: true, workspace: false, lineage: false,
  pullRequest: false, titleSource: false, turns: true, away: false, chain: false, account: false, schedules: false,
  startedFromOutside: false,
  limit: false,
  secretRequest: false, side: false,
  folderTrust: false,
};
const FOLDER_TRUST: Fields<ThreadFolderTrust> = { folder: true, asked: true };
const FOLDER_ANSWER: Fields<FolderTrustAnswer> = { folder: true, state: true };
const FOLDER_STATES: Members<FolderTrustState> = { trusted: true, unasked: true, declined: true };
const LIMIT: Fields<ThreadLimit> = { resetsAt: false };
const SCHEDULE_KINDS: Members<ScheduleKind> = { prompt: true, resume: true };
const SCHEDULES: Fields<ThreadSchedules> = { schedules: true, watch: false };
const SCHEDULE: Fields<ThreadScheduleView> = {
  id: true, name: false, prompt: true, when: true, enabled: true, createdAt: true, nextAt: false,
  lastRun: false, waiting: false, setBy: true, paused: true, kind: false,
};
const WATCH: Fields<ThreadWatchView> = {
  enabled: true, needsGh: true, failingChecks: true, reviewThreads: true, observedAt: false,
  lastRun: false, waiting: true,
};
const SCHEDULE_RUN: Fields<ScheduleRun> = { occurrence: true, at: true, outcome: true, detail: false };
const CALENDAR: Fields<ScheduleCalendar> = { timezone: true, weekdays: true, hour: true, minute: true };
const SCHEDULE_OUTCOMES: Members<ScheduleOutcome> = { sent: true, skipped: true, unknown: true, refused: true };
const SET_BY: Members<ScheduleSetBy> = { owner: true, agent: true };
const CHAIN: Fields<ThreadChain> = { segments: true };
const SEGMENT: Fields<ThreadChainSegment> = {
  key: true, engine: true, order: true, firstAt: true, lastAt: true, added: false, removed: false,
};
const HANDOFFS: Members<HandoffDelivery> = { sent: true, unsent: true, unknown: true };
const SETTINGS: Fields<ThreadSettings> = {
  autoSettleDays: true, settleOnMerge: true, generatedTitles: true, trackPullRequests: true,
  teamTools: false, outsideAgents: false, paneToolsWithoutAsking: false,
};
const OUTSIDE_LEVELS: Members<OutsideAgents> = { off: true, read: true, full: true };
const VIEW: Fields<ThreadsView> = {
  version: true, revision: true, now: true, settings: true, threads: true, hiddenKeys: false,
};
const PREPARED: Fields<PreparedWorkspace> = { token: true, cwd: true, workspace: true };
const BRANCHES: Fields<BranchList> = { current: false, defaultBranch: false, branches: true };
const WORKTREE: Fields<WorktreeEntry> = { path: true, branch: false, ownerKey: false };
const FILE: Fields<DiffFile> = {
  path: true, oldPath: false, status: true, added: true, removed: true, binary: true,
  patch: false, omitted: false,
};
const DIFF: Fields<ThreadDiff> = { scope: true, base: true, head: true, files: true, truncated: true };
const RESTORE: Fields<RestoreResult> = { restored: true, recoveryRef: true };
const RESPONSE: Fields<ThreadControlResponse> = {
  view: true, prepared: false, branches: false, worktrees: false, diff: false, restore: false,
  key: false, prompt: false, ship: false, shipped: false, reviewComments: false, terminal: false,
  consult: false, commitReview: false, workflow: false, setup: false, handoff: false, shipText: false,
  folderTrust: false,
};
const TERMINAL: Fields<ThreadTerminalHandoff> = { profileId: true, providerSessionId: true, cwd: true };
const SHIP_FILE: Fields<ShipFile> = { path: true, status: true };
const LEFT_OUT: Fields<ShipLeftOut> = { path: true, reason: true };
const SHIP_PR: Fields<ShipPullRequest> = {
  number: true, url: true, state: true, title: true, base: true, draft: true,
};
const CHECK: Fields<ShipCheck> = { name: true, workflow: false, bucket: true, link: false };
const CHECKS: Fields<ShipChecks> = {
  state: true, passed: true, failed: true, pending: true, skipped: true, failing: true,
};
const SHIP: Fields<ThreadShip> = {
  key: true, branch: false, defaultBranch: false, remote: false, upstream: false, ahead: false,
  behind: false, files: true, leftOut: true, filesUnknown: true, commitBlocked: false, message: true,
  pushBlocked: false, suggestedBranch: false, gh: true, ghFix: false, remoteRead: true,
  pullRequest: false, checks: false,
};
const OUTCOME: Fields<ShipOutcome> = { action: true, certainty: true, message: true };
const COMMENT: Fields<ReviewComment> = {
  path: true, startLine: false, endLine: false, side: true, code: true, comment: true, url: true,
  outdated: true,
};

const AWAY: Fields<ThreadAway> = {
  runId: true, state: true, detail: true, provider: true, model: false, reviewer: false, objective: true,
  startedAt: true, deadlineAt: true, maxIterations: true, rounds: true,
};
const AWAY_ROUND: Fields<ThreadAwayRound> = {
  iteration: true, role: true, state: true, turn: false, sessionKey: false, verdict: false,
  summary: false, checks: false,
};
const AWAY_STATES: Members<ThreadAway["state"]> = {
  running: true, stopping: true, completed: true, stopped: true, expired: true, blocked: true, unknown: true,
};
const ROUND_STATES: Members<AwayRoundState> = {
  started: true, completed: true, blocked: true, unknown: true, cancelled: true,
};
const VERDICTS: Members<AwayVerdict> = { continue: true, complete: true, revise: true, blocked: true };

type Kind = ThreadCommand["kind"];
const COMMANDS: { [K in Kind]: Fields<Omit<Extract<ThreadCommand, { kind: K }>, "kind">> } = {
  read: {},
  workflow: { command: true },
  pin: { key: true },
  unpin: { key: true },
  reorder: { key: true, section: true, beforeKey: false },
  settle: { key: true },
  unsettle: { key: true },
  snooze: { key: true, until: true },
  unsnooze: { key: true },
  visit: { key: true },
  markUnread: { key: true },
  setAutoSettle: { key: true, enabled: true },
  setSettings: { settings: true },
  prepareWorkspace: { projectRoot: true, mode: true, baseRef: false, worktreePath: false, hint: true },
  discardWorkspace: { token: true },
  bind: { key: true, token: false, prompt: true, titleSource: false },
  listBranches: { projectRoot: true },
  listWorktrees: { projectRoot: true },
  turnDiff: { key: true, ordinal: true },
  threadDiff: { key: true, scope: true },
  restoreTurn: { key: true, ordinal: true },
  forkAt: { key: true, keepTurns: true },
  editFrom: { key: true, ordinal: true, restoreFiles: true },
  releaseWorkspace: { key: true },
  regenerateTitle: { key: true },
  refreshPullRequest: { key: true },
  shipStatus: { key: true, remote: true },
  commit: { key: true, message: true, paths: true },
  reviewCommit: { key: true },
  commitSelected: { key: true, revision: true, message: true, choices: true },
  createBranch: { key: true, branch: true },
  openPullRequest: { key: true, title: true, body: true },
  failingChecksPrompt: { key: true },
  reviewComments: { key: true },
  mergePullRequest: { key: true, number: true, method: true },
  openInTerminal: { key: true },
  continueFromTerminal: { key: true },
  secondOpinion: { key: true, engine: true, requestId: true },
  continueWith: { key: true, engine: true, account: false },
  switchProvider: { key: true, engine: true, model: false },
  scheduleSet: { key: true, id: false, name: false, prompt: true, when: true },
  scheduleDelete: { id: true },
  scheduleEnable: { id: true, enabled: true },
  watchSet: { key: true, enabled: true },
  resumeAtReset: { key: true, prompt: false },
  suggestShipText: { key: true, what: true, paths: false },
  sideAsk: { key: true, question: true },
  sideClose: { key: true },
  folderTrust: { folder: true },
  trustFolder: { folder: true, trust: true },
};

function checkWhen(value: unknown, where: string): void {
  const when = record(value, where);
  switch (when.kind) {
    case "once": shape(when, { kind: true, at: true }, where); break;
    case "interval": shape(when, { kind: true, minutes: true }, where); break;
    case "calendar":
      shape(when, { kind: true, calendar: true }, where);
      shape(when.calendar, CALENDAR, `${where}.calendar`);
      break;
    default: expect.fail(`${where}.kind ${String(when.kind)}`);
  }
}

function checkScheduleRun(value: unknown, where: string): void {
  member(shape(value, SCHEDULE_RUN, where).outcome, SCHEDULE_OUTCOMES, `${where}.outcome`);
}

const SECTIONS: Members<ThreadSection> = {
  pinned: true, active: true, snoozed: true, settled: true, fromTerminals: true,
};
const SETTLED_REASONS: Members<SettledReason> = {
  manual: true, idle: true, merged: true, edited: true, external: true,
};
const MODES: Members<WorkspaceMode> = { local: true, worktree: true, none: true };
const KEPT: Members<WorkspaceKept> = { dirty: true, ignored: true, inUse: true };
const OUTCOMES: Members<RunOutcome> = { completed: true, failed: true, interrupted: true };
const TURN_STATES: Members<TurnState> = { running: true, done: true, failed: true, interrupted: true };
const RELATIONS: Members<LineageRelation> = { fork: true, edit: true, handoff: true, child: true };
const TITLE_SOURCES: Members<TitleSource> = { prompt: true, generated: true, user: true };
const PR_STATES: Members<PullRequestState> = { open: true, merged: true, closed: true };
const SCOPES: Members<DiffScope> = { turn: true, thread: true, uncommitted: true, branch: true };
const FILE_STATUSES: Members<DiffFileStatus> = { added: true, modified: true, deleted: true, renamed: true };
const OMISSIONS: Members<DiffOmitted> = { sensitive: true, large: true, binary: true };
const GH_STATES: Members<GhState> = { ready: true, missing: true, signedOut: true, unavailable: true };
const BUCKETS: Members<CheckBucket> = { pass: true, fail: true, pending: true, skipping: true, cancel: true };
const CHECKS_STATES: Members<ChecksState> = { passing: true, failing: true, pending: true, none: true };
const METHODS: Members<MergeMethod> = { squash: true, merge: true, rebase: true };
const SIDES: Members<DiffSide> = { new: true, old: true };
const ACTIONS: Members<ShipAction> = { commit: true, createBranch: true, openPullRequest: true, merge: true };
const CERTAINTIES: Members<ShipCertainty> = { done: true, unknown: true };
const REASONS: Members<LeftOutReason> = { credential: true, notThisThread: true };

type Json = Record<string, unknown>;

function record(value: unknown, where: string): Json {
  expect(typeof value === "object" && value !== null && !Array.isArray(value), where).toBe(true);
  return value as Json;
}

/** The object has exactly the TS fields: every key known, every required key present. */
function shape(value: unknown, fields: Record<string, boolean>, where: string): Json {
  const object = record(value, where);
  for (const key of Object.keys(object)) expect(Object.keys(fields), `${where}.${key}`).toContain(key);
  for (const [key, required] of Object.entries(fields)) {
    if (required) expect(object, where).toHaveProperty([key]);
  }
  return object;
}

function member(value: unknown, members: Record<string, true>, where: string): void {
  expect(Object.keys(members), where).toContain(value);
}

function optional(value: unknown, check: (value: unknown) => void): void {
  if (value !== undefined) check(value);
}

function checkWorkspace(value: unknown, where: string): void {
  const workspace = shape(value, WORKSPACE, where);
  member(workspace.mode, MODES, `${where}.mode`);
  optional(workspace.kept, kept => member(kept, KEPT, `${where}.kept`));
}

function checkView(value: unknown, where: string): ThreadsView {
  const view = shape(value, VIEW, where);
  expect(view.version).toBe(1);
  shape(view.settings, SETTINGS, `${where}.settings`);
  (view.threads as unknown[]).forEach((value, index) => {
    const at = `${where}.threads[${index}]`;
    const thread = shape(value, THREAD, at);
    member(thread.section, SECTIONS, `${at}.section`);
    optional(thread.settledReason, reason => member(reason, SETTLED_REASONS, `${at}.settledReason`));
    optional(thread.titleSource, source => member(source, TITLE_SOURCES, `${at}.titleSource`));
    optional(thread.lastRun, run => {
      const checked = shape(run, RUN, `${at}.lastRun`);
      optional(checked.outcome, outcome => member(outcome, OUTCOMES, `${at}.lastRun.outcome`));
    });
    optional(thread.workspace, workspace => checkWorkspace(workspace, `${at}.workspace`));
    optional(thread.secretRequest, request => shape(request, { id: true, name: true, reason: true, agent: true },
      `${at}.secretRequest`));
    optional(thread.side, side => shape(side, { key: true, question: true, askedAt: true }, `${at}.side`));
    optional(thread.folderTrust, trust => shape(trust, FOLDER_TRUST, `${at}.folderTrust`));
    optional(thread.lineage, lineage => {
      member(shape(lineage, LINEAGE, `${at}.lineage`).relation, RELATIONS, `${at}.lineage.relation`);
    });
    optional(thread.pullRequest, pr => {
      member(shape(pr, PULL_REQUEST, `${at}.pullRequest`).state, PR_STATES, `${at}.pullRequest.state`);
    });
    (thread.turns as unknown[]).forEach((turn, ordinal) => {
      member(shape(turn, TURN, `${at}.turns[${ordinal}]`).state, TURN_STATES, `${at}.turns[${ordinal}].state`);
    });
    optional(thread.away, away => {
      const checked = shape(away, AWAY, `${at}.away`);
      member(checked.state, AWAY_STATES, `${at}.away.state`);
      (checked.rounds as unknown[]).forEach((round, index) => {
        const where = `${at}.away.rounds[${index}]`;
        const shaped = shape(round, AWAY_ROUND, where);
        member(shaped.role, { work: true, review: true }, `${where}.role`);
        member(shaped.state, ROUND_STATES, `${where}.state`);
        optional(shaped.verdict, verdict => member(verdict, VERDICTS, `${where}.verdict`));
      });
    });
    optional(thread.chain, chain => {
      (shape(chain, CHAIN, `${at}.chain`).segments as unknown[]).forEach((segment, index) => {
        shape(segment, SEGMENT, `${at}.chain.segments[${index}]`);
      });
    });
    optional(thread.limit, limit => {
      const checked = shape(limit, LIMIT, `${at}.limit`);
      optional(checked.resetsAt, resetsAt => expect(typeof resetsAt).toBe("string"));
    });
    optional(thread.schedules, schedules => {
      const checked = shape(schedules, SCHEDULES, `${at}.schedules`);
      (checked.schedules as unknown[]).forEach((value, index) => {
        const where = `${at}.schedules.schedules[${index}]`;
        const schedule = shape(value, SCHEDULE, where);
        checkWhen(schedule.when, `${where}.when`);
        member(schedule.setBy, SET_BY, `${where}.setBy`);
        optional(schedule.kind, kind => member(kind, SCHEDULE_KINDS, `${where}.kind`));
        optional(schedule.lastRun, run => checkScheduleRun(run, `${where}.lastRun`));
      });
      optional(checked.watch, watch => {
        const shaped = shape(watch, WATCH, `${at}.schedules.watch`);
        optional(shaped.lastRun, run => checkScheduleRun(run, `${at}.schedules.watch.lastRun`));
      });
    });
  });
  return value as ThreadsView;
}

describe("threads v34 boundary", () => {
  it("has one command of every kind, each exactly the TS command shape", () => {
    const seen = new Set<string>();
    for (const value of [...fixture.commands, ...fixtureV37.commands, ...fixtureV42.commands,
      ...fixtureV46.commands, ...fixtureV49.commands, ...workflowV49.commands,
      ...fixtureV57.commands, ...fixtureV59.commands, ...fixtureV62.commands,
      ...fixtureV66.commands, ...fixtureV67.commands, ...fixtureV72.commands] as unknown[]) {
      const command = record(value, "command");
      const kind = command.kind as Kind;
      expect(Object.keys(COMMANDS)).toContain(kind);
      seen.add(kind);
      const fields = Object.fromEntries(Object.entries(command).filter(([key]) => key !== "kind"));
      shape(fields, COMMANDS[kind], kind);
      optional(command.section, section => member(section, SECTIONS, `${kind}.section`));
      optional(command.scope, scope => member(scope, SCOPES, `${kind}.scope`));
      optional(command.titleSource, source => member(source, TITLE_SOURCES, `${kind}.titleSource`));
      optional(command.settings, settings => shape(settings, SETTINGS, `${kind}.settings`));
      optional(command.mode, mode => expect(["local", "worktree", "existing"]).toContain(mode));
      optional(command.method, method => member(method, METHODS, `${kind}.method`));
      optional(command.when, when => checkWhen(when, `${kind}.when`));
      optional(command.what, what => expect(["commit", "pullRequest"]).toContain(what));
    }
    expect([...seen].sort()).toEqual(Object.keys(COMMANDS).sort());
  });

  it("encodes every command as the one named-object payload Rust decodes", () => {
    for (const command of [...fixture.commands, ...fixtureV37.commands, ...fixtureV42.commands,
      ...fixtureV46.commands, ...fixtureV49.commands, ...workflowV49.commands,
      ...fixtureV57.commands, ...fixtureV59.commands, ...fixtureV62.commands,
      ...fixtureV66.commands, ...fixtureV67.commands, ...fixtureV72.commands] as unknown as ThreadCommand[]) {
      expect(encodeRendererPayload(CHANNELS.threadControl, [command])).toEqual(command);
    }
    expect(() => encodeRendererPayload(CHANNELS.threadControl, [])).toThrow();
  });

  it("parses the shared view with every section, workspace mode, lineage, PR and turn", () => {
    const view = checkView(fixture.view, "view");
    // fromTerminals arrived with protocol 42 (threads-v42.json).
    expect(new Set(view.threads.map(thread => thread.section)))
      .toEqual(new Set(Object.keys(SECTIONS).filter(section => section !== "fromTerminals")));
    expect(new Set(view.threads.map(thread => thread.workspace?.mode)))
      .toEqual(new Set(Object.keys(MODES)));
    expect(view.threads.some(thread => thread.lineage !== undefined)).toBe(true);
    expect(view.threads.some(thread => thread.pullRequest !== undefined)).toBe(true);
    expect(view.threads.some(thread => thread.workspace?.kept !== undefined), "a kept worktree").toBe(true);
    expect(view.threads.flatMap(thread => thread.turns).length).toBeGreaterThan(0);
    // An Away loop's projection arrived with protocol 45 (threads-v45.json),
    // a switched thread's chain with protocol 59 (threads-switch-v59.json),
    // a thread's account with protocol 60 (provider-accounts-v60.json),
    // scheduled prompts with protocol 62 (threads-schedules-v62.json), who
    // outside started it with protocol 65 (outside-agents-v65.json), a
    // thread's limit with protocol 66 (threads-resume-v66.json), a secret request
    // and a side question with protocol 67 (quick-wins-v67.json), an
    // untrusted folder with protocol 72 (folder-trust-v72.json).
    for (const [key] of Object.entries(THREAD).filter(([key]) => !["away", "chain", "account", "schedules",
      "startedFromOutside", "limit", "secretRequest", "side", "folderTrust"].includes(key))) {
      expect(view.threads.some(thread => key in thread), `some thread carries ${key}`).toBe(true);
    }
  });

  it("parses the shared response with every optional field present", () => {
    const response = shape(fixture.response, RESPONSE, "response");
    // Protocol 37's ship fields are in threads-v37.json.
    const v34 = Object.keys(RESPONSE)
      .filter(key => !["ship", "shipped", "reviewComments", "terminal", "consult", "commitReview", "workflow", "setup", "handoff", "shipText", "folderTrust"].includes(key));
    for (const key of v34) expect(response, "response").toHaveProperty([key]);
    checkView(response.view, "response.view");
    const prepared = shape(response.prepared, PREPARED, "response.prepared");
    checkWorkspace(prepared.workspace, "response.prepared.workspace");
    shape(response.branches, BRANCHES, "response.branches");
    (response.worktrees as unknown[]).forEach((entry, index) => {
      shape(entry, WORKTREE, `response.worktrees[${index}]`);
    });
    const diff = shape(response.diff, DIFF, "response.diff");
    member(diff.scope, SCOPES, "response.diff.scope");
    const files = diff.files as unknown[];
    files.forEach((value, index) => {
      const file = shape(value, FILE, `response.diff.files[${index}]`);
      member(file.status, FILE_STATUSES, `response.diff.files[${index}].status`);
      optional(file.omitted, omitted => member(omitted, OMISSIONS, `response.diff.files[${index}].omitted`));
    });
    shape(response.restore, RESTORE, "response.restore");
    const typed = fixture.response as unknown as ThreadControlResponse;
    expect(typed.key).toBe(typed.view.threads[0]?.key);
    expect(typed.view.threads[0]?.lineage?.relation).toBe("edit");
  });
});

describe("threads v42 boundary", () => {
  it("parses the From terminals section and the Open in terminal answer, exactly the TS shapes", () => {
    expect(RUST_HOST_PROTOCOL_VERSION).toBeGreaterThanOrEqual(fixtureV42.protocolVersion);
    const view = checkView(fixtureV42.view, "view");
    expect(view.threads[0]?.section).toBe("fromTerminals");
    expect(view.threads[0]?.settledReason).toBe("external");
    const response = shape(fixtureV42.response, RESPONSE, "response");
    const terminal = shape(response.terminal, TERMINAL, "response.terminal");
    expect(["claude", "codex"]).toContain(terminal.profileId);
  });
});

describe("threads v45 boundary", () => {
  it("parses an Away loop's thread with its rounds and verdicts, exactly the TS shapes", () => {
    expect(RUST_HOST_PROTOCOL_VERSION).toBeGreaterThanOrEqual(fixtureV45.protocolVersion);
    const view = checkView(fixtureV45.view, "view");
    const away = view.threads[0]?.away;
    expect(away?.state).toBe("completed");
    expect(away?.rounds.map(round => round.verdict))
      .toEqual(["continue", "revise", "continue", "complete", undefined]);
    expect(away?.rounds.filter(round => round.role === "work").map(round => round.turn))
      .toEqual([0, 1, undefined]);
    expect(away?.rounds[1]?.sessionKey).toBe("codex#0199f2a4-review");
    // The reviewer arrived with protocol 47 (project-away-v47.json).
    for (const field of Object.keys(AWAY).filter(field => field !== "reviewer")) {
      expect(away, "away").toHaveProperty([field]);
    }
    const older = checkView(fixtureV42.view, "v42 view");
    expect(older.threads.every(thread => thread.away === undefined)).toBe(true);
  });
});

describe("threads v58 boundary", () => {
  it("parses the sessions a view hides as workers, absent from older views", () => {
    expect(RUST_HOST_PROTOCOL_VERSION).toBeGreaterThanOrEqual(fixtureV58.protocolVersion);
    const view = checkView(fixtureV58.view, "view");
    expect(view.hiddenKeys).toEqual(["codex#0199f2a4-7c1e-7d30-9a5b-1c2d3e4f5a6b", "codex#0199f2a4-review"]);
    expect(checkView(fixtureV42.view, "v42 view").hiddenKeys).toBeUndefined();
  });
});

describe("threads v46 boundary", () => {
  it("parses a handed-off thread's lineage and the consult a second opinion submitted", () => {
    expect(RUST_HOST_PROTOCOL_VERSION).toBeGreaterThanOrEqual(fixtureV46.protocolVersion);
    const view = checkView(fixtureV46.view, "view");
    expect(view.threads[0]?.lineage).toEqual({
      parent: view.threads[1]?.key, relation: "handoff", keptTurns: 0, at: 1_791_298_790_000,
    });
    const response = shape(fixtureV46.response, RESPONSE, "response");
    checkView(response.view, "response.view");
    checkWorkspace(shape(response.prepared, PREPARED, "response.prepared").workspace,
      "response.prepared.workspace");
    expect(typeof response.prompt).toBe("string");
    const consult = (fixtureV46.response as unknown as ThreadControlResponse).consult;
    expect(consult?.receipt.mode).toBe("consult");
    expect(consult?.receipt.state).toBe("awaiting_approval");
    expect(consult?.sourceSessionKey).toBe(view.threads[1]?.key);
    expect(consult?.approval?.approvalId).toBe("approval-2b9c");
  });
});

describe("threads v57 boundary", () => {
  it("parses a started thread's child lineage and team tools turned off", () => {
    expect(RUST_HOST_PROTOCOL_VERSION).toBeGreaterThanOrEqual(fixtureV57.protocolVersion);
    const view = checkView(fixtureV57.view, "view");
    expect(view.threads[0]?.lineage).toEqual({
      parent: view.threads[1]?.key, relation: "child", keptTurns: 0, at: 1_791_298_790_000,
    });
    expect(view.settings.teamTools).toBe(false);
    // Absent in an older view: on.
    expect(checkView(fixtureV46.view, "v46 view").settings.teamTools).toBeUndefined();
  });
});

describe("threads v65 boundary", () => {
  it("parses the outside agents level and a thread an outside agent started", () => {
    expect(RUST_HOST_PROTOCOL_VERSION).toBeGreaterThanOrEqual(fixtureV65.protocolVersion);
    const view = checkView(fixtureV65.view, "view");
    member(view.settings.outsideAgents, OUTSIDE_LEVELS, "settings.outsideAgents");
    expect(view.settings.outsideAgents).toBe("full");
    expect(view.threads[0]?.startedFromOutside).toBe("claude-code");
    const commands = fixtureV65.commands as unknown as ThreadCommand[];
    expect(commands.map(command => command.kind === "setSettings" ? command.settings.outsideAgents : null))
      .toEqual(["full", "read"]);
    for (const command of commands) {
      shape((command as { settings: unknown }).settings, SETTINGS, "command.settings");
      expect(() => encodeRendererPayload(CHANNELS.threadControl, [command])).not.toThrow();
    }
    // Absent in an older view: off, and nobody outside started anything.
    const older = checkView(fixtureV59.view, "v59 view");
    expect(older.settings.outsideAgents).toBeUndefined();
    expect(older.threads.every(thread => thread.startedFromOutside === undefined)).toBe(true);
  });
});

describe("threads v59 boundary", () => {
  it("parses a switched thread's chain, the switch command and whether its handoff arrived", () => {
    expect(RUST_HOST_PROTOCOL_VERSION).toBeGreaterThanOrEqual(fixtureV59.protocolVersion);
    const view = checkView(fixtureV59.view, "view");
    const head = view.threads[0];
    expect(head?.chain?.segments.map(segment => [segment.engine, segment.order]))
      .toEqual([["claude", 0], ["codex", 1]]);
    expect(head?.chain?.segments.at(-1)?.key).toBe(head?.key);
    const response = shape(fixtureV59.response, RESPONSE, "response");
    member(response.handoff, HANDOFFS, "response.handoff");
    expect(response.key).toBe(head?.key);
    // Absent in an older view: an ordinary thread.
    expect(checkView(fixtureV57.view, "v57 view").threads.every(thread => thread.chain === undefined))
      .toBe(true);
  });
});

describe("threads v62 boundary", () => {
  it("parses a thread's schedules of every kind and its watch, exactly the TS shapes", () => {
    expect(RUST_HOST_PROTOCOL_VERSION).toBeGreaterThanOrEqual(fixtureV62.protocolVersion);
    const view = checkView(fixtureV62.view, "view");
    const schedules = view.threads[0]?.schedules;
    expect(schedules?.schedules.map(schedule => schedule.when.kind)).toEqual(["interval", "calendar", "once"]);
    expect(schedules?.schedules.map(schedule => schedule.lastRun?.outcome)).toEqual(["sent", "refused", "skipped"]);
    expect(schedules?.schedules[0]?.setBy).toBe("agent");
    expect(schedules?.schedules[1]?.waiting).toBe(1_791_293_400_000);
    expect(schedules?.watch?.lastRun?.outcome).toBe("unknown");
    // A schedule's kind arrived with protocol 66 (threads-resume-v66.json).
    for (const key of Object.keys(SCHEDULE).filter(key => key !== "kind")) {
      expect(schedules?.schedules.some(schedule => key in schedule), `some schedule carries ${key}`).toBe(true);
    }
    for (const key of Object.keys(WATCH)) expect(schedules?.watch, "watch").toHaveProperty([key]);
    checkView(shape(fixtureV62.response, RESPONSE, "response").view, "response.view");
    // Absent in an older view.
    expect(checkView(fixtureV59.view, "v59 view").threads.every(thread => thread.schedules === undefined))
      .toBe(true);
  });
});

describe("threads v66 boundary", () => {
  it("parses a thread's limit, a resume schedule and Resume at reset, exactly the TS shapes", () => {
    expect(RUST_HOST_PROTOCOL_VERSION).toBeGreaterThanOrEqual(fixtureV66.protocolVersion);
    const view = checkView(fixtureV66.view, "view");
    expect(view.threads.map(thread => thread.limit)).toEqual([{ resetsAt: "2026-11-15T21:10:00.000Z" }, {}]);
    const resume = view.threads[0]?.schedules?.schedules[0];
    expect(resume?.kind).toBe("resume");
    expect(resume?.when).toEqual({ kind: "once", at: 1_794_777_060_000 });
    for (const key of Object.keys(LIMIT)) expect(view.threads[0]?.limit, "limit").toHaveProperty([key]);
    expect(fixtureV66.commands.map(command => command.kind)).toEqual(["resumeAtReset", "resumeAtReset"]);
    // A rate-limit notice carries its reset when the provider said, and only its message otherwise.
    expect(fixtureV66.events.map(event => Object.keys(event.rateLimited).sort()))
      .toEqual([["message", "resetsAt"], ["message"]]);
    // Absent in an older view and schedule.
    const older = checkView(fixtureV62.view, "v62 view");
    expect(older.threads.every(thread => thread.limit === undefined)).toBe(true);
    expect(older.threads[0]?.schedules?.schedules.every(schedule => schedule.kind === undefined)).toBe(true);
  });
});

describe("threads v67 boundary", () => {
  it("parses Write with AI's commands and the answer's shipText (quick-wins-v67.json)", () => {
    expect(fixtureV67.protocolVersion).toBe(67);
    expect(RUST_HOST_PROTOCOL_VERSION).toBeGreaterThanOrEqual(fixtureV67.protocolVersion);
    expect(fixtureV67.commands.map(command => command.kind))
      .toEqual(["suggestShipText", "suggestShipText", "sideAsk", "sideClose"]);
    const response = shape(fixtureV67.response, RESPONSE, "response");
    checkView(response.view, "response.view");
    expect(typeof response.shipText).toBe("string");
    // The private secret card's open request rides the thread's view.
    const view = checkView(fixtureV67.view, "view");
    expect(view.threads[0]?.secretRequest).toEqual({ id: "secret-1", name: "STRIPE_TEST_KEY",
      reason: "To run the payment tests against Stripe's test mode.", agent: "Claude" });
    // And the side question it has open.
    expect(view.threads[0]?.side).toEqual({ key: "claude#side-7f3a",
      question: "Which test covers the login timeout?", askedAt: 1_791_298_800_000 });
  });
});

describe("threads v72 boundary", () => {
  it("parses folder trust's commands, answers and the untrusted thread's row (folder-trust-v72.json)", () => {
    expect(fixtureV72.protocolVersion).toBe(72);
    expect(RUST_HOST_PROTOCOL_VERSION).toBeGreaterThanOrEqual(fixtureV72.protocolVersion);
    expect(fixtureV72.commands.map(command => command.kind))
      .toEqual(["folderTrust", "trustFolder", "trustFolder"]);
    const states = fixtureV72.responses.map((value, index) => {
      const response = shape(value, RESPONSE, `responses[${index}]`);
      checkView(response.view, `responses[${index}].view`);
      const answer = shape(response.folderTrust, FOLDER_ANSWER, `responses[${index}].folderTrust`);
      member(answer.state, FOLDER_STATES, `responses[${index}].folderTrust.state`);
      return answer.state;
    });
    expect(states).toEqual(["unasked", "trusted", "declined"]);
    const view = checkView(fixtureV72.view, "view");
    expect(view.threads.map(thread => thread.folderTrust)).toEqual([
      { folder: "/Users/fixture/Downloads/cloned-tool", asked: false },
      { folder: "/Users/fixture/Downloads/other-tool", asked: true },
      undefined,
    ]);
    // An older view carries none.
    expect(checkView(fixtureV67.view, "v67 view").threads.every(thread => thread.folderTrust === undefined))
      .toBe(true);
  });
});

describe("threads v69 boundary", () => {
  it("parses each chain segment's line totals, both counts or neither (threads-totals-v69.json)", () => {
    expect(fixtureV69.protocolVersion).toBe(69);
    expect(RUST_HOST_PROTOCOL_VERSION).toBeGreaterThanOrEqual(fixtureV69.protocolVersion);
    const view = checkView(fixtureV69.view, "view");
    expect(view.threads[0]?.chain?.segments.map(segment => [segment.added, segment.removed]))
      .toEqual([[7, 2], [undefined, undefined], [3, 1]]);
    expect(view.threads[1]?.chain).toBeUndefined();
    // An older chain carries neither.
    expect(checkView(fixtureV59.view, "v59 view").threads[0]?.chain?.segments
      .every(segment => segment.added === undefined && segment.removed === undefined)).toBe(true);
  });
});

describe("threads v71 boundary", () => {
  it("carries Pane tools without asking, absent meaning off (ask-first-v71.json)", () => {
    expect(fixtureV71.protocolVersion).toBe(71);
    expect(RUST_HOST_PROTOCOL_VERSION).toBeGreaterThanOrEqual(fixtureV71.protocolVersion);
    const settings = shape(fixtureV71.threadSettings, SETTINGS, "threadSettings") as unknown as ThreadSettings;
    expect(settings.paneToolsWithoutAsking).toBe(true);
    for (const command of fixtureV71.threadCommands as unknown as ThreadCommand[]) {
      shape((command as { settings: unknown }).settings, SETTINGS, "command.settings");
      expect(encodeRendererPayload(CHANNELS.threadControl, [command])).toEqual(command);
    }
    // Absent in an older view: off.
    expect(checkView(fixtureV69.view, "v69 view").settings.paneToolsWithoutAsking).toBeUndefined();
  });
});

describe("threads v36 boundary", () => {
  it("parses turns that carry when their run ended, beside older turns without one", () => {
    const view = checkView(fixtureV36.view, "view");
    expect(view.threads[0]?.turns.map(turn => turn.endedAt))
      .toEqual([undefined, 1_791_298_534_000, 1_791_301_041_000, undefined]);
    expect(view.threads[0]?.turns.at(-1)?.state).toBe("running");
    const older = checkView(fixture.view, "v34 view");
    expect(older.threads.flatMap(thread => thread.turns).every(turn => turn.endedAt === undefined)).toBe(true);
  });
});

describe("threads v37 boundary", () => {
  it("parses a response carrying every ship field, exactly the TS shapes", () => {
    const response = shape(fixtureV37.response, RESPONSE, "response");
    checkView(response.view, "response.view");
    const ship = shape(response.ship, SHIP, "ship");
    for (const key of Object.keys(SHIP)) expect(ship, "ship").toHaveProperty([key]);
    member(ship.gh, GH_STATES, "ship.gh");
    (ship.files as unknown[]).forEach((file, index) => {
      member(shape(file, SHIP_FILE, `ship.files[${index}]`).status, FILE_STATUSES, "status");
    });
    (ship.leftOut as unknown[]).forEach((left, index) => {
      member(shape(left, LEFT_OUT, `ship.leftOut[${index}]`).reason, REASONS, "reason");
    });
    member(shape(ship.pullRequest, SHIP_PR, "ship.pullRequest").state, PR_STATES, "pr.state");
    const checks = shape(ship.checks, CHECKS, "ship.checks");
    member(checks.state, CHECKS_STATES, "checks.state");
    (checks.failing as unknown[]).forEach((check, index) => {
      member(shape(check, CHECK, `checks.failing[${index}]`).bucket, BUCKETS, "bucket");
    });
    const shipped = shape(response.shipped, OUTCOME, "shipped");
    member(shipped.action, ACTIONS, "shipped.action");
    member(shipped.certainty, CERTAINTIES, "shipped.certainty");
    (response.reviewComments as unknown[]).forEach((comment, index) => {
      member(shape(comment, COMMENT, `reviewComments[${index}]`).side, SIDES, "side");
    });
    expect(typeof response.prompt).toBe("string");
  });
});
