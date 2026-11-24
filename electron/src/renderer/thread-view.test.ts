import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it } from "vitest";
import type { ThreadCommand, ThreadControlResponse, ThreadTurn, ThreadView, ThreadsView } from "../shared/threads";
import fixture from "../fixtures/threads-v34.json";
import { AgentTile } from "./components/AgentTile";
import { Conversation, type ConversationProps } from "./components/Conversation";
import { ConversationTranscript } from "./components/ConversationTranscript";
import type { TranscriptDensity } from "./transcript-density";
import {
  CHAT_WHERE_WORDS, LOCAL_WHERE_WORDS, MISSING_WORKTREE_TEXT, pullRequestHref, ThreadDiffToggle,
  ThreadPaneHead, threadHeadWhere, threadPullRequestLabel, WORKTREE_WHERE_WORDS,
} from "./components/ThreadPaneHead";
import { readFileSync } from "node:fs";
import {
  KEEP_FILES_LABEL, RESTORE_FILES_LABEL, ThreadEditDialog, threadEditCardNote, threadEditDescription,
} from "./components/ThreadEditDialog";
import type { ConversationThread, TurnRestorePoint } from "./conversation-types";
import type { ConvItem } from "./conversation-reducer";
import {
  conversationThread, editThreadFrom, forkThreadAt, restoreThreadFiles, REVERT_LOCAL_BLOCKED,
  REVERT_SHARED_BLOCKED,
  retainThreadIndex, sameWorkspaceAs, THREAD_FIRST_EDIT_RESTORED, THREAD_STILL_WORKING, threadDiffSignal,
  threadChangeTotals, threadFilesRestorePoint, ThreadPaneLock, threadWorkspaceLabel,
  threadRecoveryNotice, type ThreadPanePort,
} from "./thread-pane";
import { forgetThreadTurnTimings } from "./thread-transcript";
import { threadPromptDigest } from "./thread-turns";
import { threadDiffToggleAction } from "./use-thread-pane";

/* ==========================================================================
   THE THREAD VIEW: a thread pane's head (branch chip, lineage), its turn
   controls (the change summary opens the turn's diff, Edit from here, Fork
   from here) and the two commands they send. A row that maps to no Rust turn
   gets no control, and a pane with no thread is exactly what it was.
   ========================================================================== */

const view = fixture.view as unknown as ThreadsView;
const worktreeThread = view.threads.find(thread => thread.workspace?.mode === "worktree")!;
const localThread = view.threads.find(thread => thread.workspace?.mode === "local")!;

const turn = (ordinal: number, prompt: string, over: Partial<ThreadTurn> = {}): ThreadTurn => ({
  ordinal, sentAt: ordinal, promptDigest: threadPromptDigest(prompt), hasStart: true,
  hasEnd: true, state: "done", ...over,
});
const ITEMS: ConvItem[] = [
  { kind: "user", id: "user-0", text: "Fix the flaky login test" },
  { kind: "text", id: "text-1", text: "Fixed it.", streaming: false },
  { kind: "user", id: "user-2", text: "typed in a terminal" },
  { kind: "text", id: "text-3", text: "Sure.", streaming: false },
  { kind: "user", id: "user-4", text: "Now add a regression test" },
  { kind: "text", id: "text-5", text: "Added.", streaming: false },
];
const TURNS = [
  turn(0, "Fix the flaky login test", { filesChanged: 3, added: 41, removed: 12 }),
  turn(1, "Now add a regression test", { filesChanged: 1, added: 18, removed: 0 }),
];
const POINTS = new Map<string, TurnRestorePoint>([["l:user-0", { description: "Restores 3 files.",
  label: "Undo · 3 files" }]]);

function paint(thread: ConversationThread | undefined, over: Partial<ConversationProps> = {}): string {
  return renderToStaticMarkup(createElement(Conversation, {
    items: ITEMS, thinking: false, live: true, sessionKey: "codex#t",
    config: { effort: null, models: [], canSetModel: true, effortLevels: [], showEffort: false,
      canSetEffort: true, pendingModel: false, pendingEffort: false, error: null },
    approvals: {}, activeApprovalIds: new Set<string>(),
    onSend: () => undefined, onStop: () => undefined,
    onSetModel: () => undefined, onSetEffort: () => undefined,
    onApprove: () => undefined, onDeny: () => undefined,
    turnRestorePoints: POINTS, onRestoreTurn: () => undefined,
    ...(thread === undefined ? {} : {
      thread,
      onThreadTurnDiff: () => undefined,
      onThreadEditFrom: () => Promise.resolve(),
      onThreadForkAt: () => undefined,
    }),
    ...over,
  })).replace(/<style[^>]*>[\s\S]*?<\/style>/gu, "");
}

const count = (markup: string, needle: string): number => markup.split(needle).length - 1;

describe("thread turn controls", () => {
  const worktree: ConversationThread = { turns: TURNS, project: true, worktree: true,
    revertFilesBlocked: null };

  it("puts the change summary and Edit from here on matched prompts only", () => {
    const markup = paint(worktree);
    expect(count(markup, "Edit from here</button>")).toBe(2);
    expect(markup).toContain("+41 −12 · 3 files");
    expect(markup).toContain("+18 −0 · 1 file");
    expect(markup).toContain('aria-label="Show the changes from this message: +41 −12 · 3 files"');
    expect(count(markup, 'class="conv-turn-changes is-action"')).toBe(2);
  });

  it("puts Fork from here at the end of each matched, finished turn", () => {
    const markup = paint(worktree);
    expect(count(markup, "Fork from here</button>")).toBe(2);
    expect(markup).toContain("a new thread with the first 1 message of this one");
    expect(markup).toContain("a new thread with the first 2 messages of this one");
  });

  it("gives no fork to a turn still running", () => {
    const running: ConversationThread = { ...worktree,
      turns: [TURNS[0], { ...TURNS[1], state: "running", hasEnd: false }] };
    expect(count(paint(running), "Fork from here</button>")).toBe(1);
  });

  it("drops the Undo control on a worktree thread, and keeps it on a local one", () => {
    expect(paint(worktree)).not.toContain("Undo · 3 files");
    const local: ConversationThread = { ...worktree, worktree: false,
      revertFilesBlocked: REVERT_LOCAL_BLOCKED };
    const markup = paint(local);
    expect(markup).toContain("Undo · 3 files");
    expect(count(markup, "Edit from here</button>")).toBe(2);
  });

  it("offers a plain chat Fork only", () => {
    const chat: ConversationThread = { turns: TURNS, project: false, worktree: false,
      revertFilesBlocked: REVERT_LOCAL_BLOCKED };
    const markup = paint(chat);
    expect(markup).not.toContain("Edit from here");
    expect(markup).not.toContain("conv-turn-changes is-action");
    expect(count(markup, "Fork from here</button>")).toBe(2);
  });

  it("draws nothing new on a pane that is not a thread", () => {
    const markup = paint(undefined);
    expect(markup).not.toContain("Edit from here");
    expect(markup).not.toContain("Fork from here");
    expect(markup).not.toContain("conv-turn-end");
    expect(markup).toContain("Undo · 3 files");
  });

  it("offers no diff for a turn whose start checkpoint was skipped", () => {
    const skipped: ConversationThread = { ...worktree,
      turns: [{ ...TURNS[0], hasStart: false }, TURNS[1]] };
    expect(count(paint(skipped), 'class="conv-turn-changes is-action"')).toBe(1);
  });

  it("draws Fork and Edit off while a fork or an edit from the pane is on its way", () => {
    // Review, 6 Oct 2026: a double click on Fork from here made two forks.
    const idle = paint(worktree);
    expect(count(idle, "Fork from here</button>")).toBe(2);
    expect(idle).not.toContain('title="Fork from here" disabled=""');
    expect(idle).not.toContain('title="Edit from here" disabled=""');
    const busy = paint({ ...worktree, busy: true });
    expect(count(busy, 'title="Fork from here" disabled=""')).toBe(2);
    expect(count(busy, 'title="Edit from here" disabled=""')).toBe(2);
    // The change summary only reads; it stays live.
    expect(count(busy, 'class="conv-turn-changes is-action"')).toBe(2);
    expect(busy).not.toMatch(/conv-turn-changes is-action"[^>]*disabled/u);
  });
});

/* ==========================================================================
   THE THREADS LAYOUT (owner, 2026-10-06 redesign): a pane in the Threads grid
   draws each finished turn as its prompt, one work line, the answer, a card of
   the files it changed and — once — its controls. The same pane without the
   flag is the transcript above, untouched.
   ========================================================================== */

const WORK: ConvItem[] = [
  { kind: "user", id: "user-0", text: "Fix the flaky login test" },
  { kind: "text", id: "text-1", text: "Looking at the retry.", streaming: false },
  { kind: "tool", id: "read-1", tool: "Read", summary: "/w/src/auth/retry.ts",
    input: { file_path: "/w/src/auth/retry.ts" }, result: { ok: true, summary: "read" } },
  { kind: "tool", id: "edit-1", tool: "Edit", summary: "/w/src/auth/retry.ts",
    input: { file_path: "/w/src/auth/retry.ts" }, result: { ok: true, summary: "Edited" } },
  { kind: "diff", id: "diff-1", path: "/w/src/auth/retry.ts", callId: "edit-1", added: 8, removed: 1 },
  { kind: "tool", id: "bash-1", tool: "Bash", summary: "npm test",
    input: { command: "npm test" }, result: { ok: false, summary: "3 failed" } },
  { kind: "text", id: "text-2", text: "The retry now backs off five times.", streaming: false },
  { kind: "user", id: "user-3", text: "Now add a regression test" },
  { kind: "tool", id: "read-2", tool: "Read", summary: "/w/test/login.test.ts",
    input: { file_path: "/w/test/login.test.ts" }, result: { ok: true, summary: "read" } },
  { kind: "text", id: "text-4", text: "Added `test/login.test.ts`.", streaming: false },
];
const SENT = 1_760_000_000_000;
const WORK_TURNS = [
  turn(0, "Fix the flaky login test", { sentAt: SENT, filesChanged: 3, added: 41, removed: 12 }),
  turn(1, "Now add a regression test", { sentAt: SENT + 60_000, filesChanged: 1, added: 18, removed: 0 }),
];

describe("the Threads layout", () => {
  // Remembered timings are the window's: each case starts with none.
  beforeEach(() => { forgetThreadTurnTimings(); });
  const local: ConversationThread = { turns: WORK_TURNS, project: true, worktree: false,
    revertFilesBlocked: REVERT_LOCAL_BLOCKED,
    lastRun: { startedAt: SENT + 60_000, endedAt: SENT + 101_000, outcome: "completed" } };
  const worktree: ConversationThread = { ...local, worktree: true, revertFilesBlocked: null };
  const POINTS_BOTH = new Map<string, TurnRestorePoint>([
    ["l:user-0", { description: "Restores 1 file.", label: "Undo · 1 file" }],
    ["l:user-3", { description: "Restores 1 file.", label: "Undo · 1 file" }]]);
  const layout = (thread: ConversationThread | undefined, over: Partial<ConversationProps> = {}): string =>
    paint(thread, { items: WORK, threadLayout: true, pathRoot: "/w", turnRestorePoints: POINTS_BOTH, ...over });

  it("marks the pane's root, and changes nothing on a pane without the flag", () => {
    expect(layout(local)).toContain('class="conv is-thread-layout"');
    const plain = paint(local, { items: WORK, pathRoot: "/w" });
    expect(plain).toContain('class="conv"');
    expect(plain).not.toMatch(/class="thread-|is-thread-layout/u);
    expect(count(plain, "Fork from here</button>")).toBe(2);
    expect(plain).toContain("conv-turn-changes is-action");
  });

  it("draws the prompt as a bare bubble, with Edit from here in the quiet line under it", () => {
    const markup = layout(worktree);
    expect(markup).not.toContain("conv-turn-changes");
    expect(markup).not.toContain("conv-turn-restore");
    expect(markup).not.toContain("conv-turn-edit");
    expect(count(markup, 'class="thread-user-meta"')).toBe(2);
    expect(count(markup, 'class="thread-act thread-edit"')).toBe(2);
    expect(count(markup, 'aria-label="Copy message"')).toBe(2);
    // The time it was sent, from Rust's record of the turn.
    expect(count(markup, '<time class="thread-time"')).toBeGreaterThanOrEqual(2);
    // A plain chat has no Edit from here; an unmatched prompt has no time.
    const chat = layout({ ...local, project: false });
    expect(chat).not.toContain("thread-edit");
    expect(layout(undefined)).not.toContain("thread-edit");
  });

  it("folds a finished turn's work into one line and keeps the answer", () => {
    const markup = layout(worktree);
    expect(count(markup, 'class="thread-fold"')).toBe(2);
    expect(markup).toContain('aria-expanded="false" title="Show the work"');
    // Turn 1 has no stamp of its own; turn 2 ran 41s by Rust's record. The
    // line's name is the whole sentence; it draws the chevron, the lead, then
    // a neutral chip per kind of work (the Threads look, owner, 2026-10-23:
    // a count, never a colour or a glyph), the sentence's tail as their title.
    expect(markup).toContain('aria-label="Worked · read 1 file, edited 1 file">'
      + '<svg class="thread-glyph thread-fold-chev" width="14" height="14"');
    expect(markup).toContain('<span class="thread-fold-lead">Worked</span>'
      + '<span class="thread-fold-chips" title="read 1 file, edited 1 file">'
      + '<span class="thread-work-chip is-read">read 1</span>'
      + '<span class="thread-work-chip is-edit">edited 1</span></span>');
    expect(markup).toContain('aria-label="Worked for 41s · read 1 file"><svg');
    expect(markup).toContain('<span class="thread-fold-lead">Worked for 41s</span>'
      + '<span class="thread-fold-chips" title="read 1 file">');
    expect(markup).not.toContain("Looking at the retry.");
    // The folded calls are not drawn at all (the card names the file it changed).
    expect(markup).not.toContain('title="/w/test/login.test.ts"');
    expect(markup).not.toContain('aria-label="Read, tool Read');
    expect(markup).toContain("The retry now backs off five times.");
    expect(markup).toContain("test/login.test.ts");
  });

  it("keeps a failure out of the fold, unfolded, in a tinted block", () => {
    const markup = layout(worktree);
    const failed = markup.slice(markup.indexOf('class="thread-failed"'));
    expect(failed).toContain('aria-label="Bash, tool Bash, npm test, failed"');
    expect(failed.slice(0, failed.indexOf("thread-answer"))).toContain(">3 failed</pre>");
    expect(markup.indexOf('class="thread-fold"')).toBeLessThan(markup.indexOf("thread-failed"));
  });

  it("shows the changes card under the answer, with Rust's counts and Review", () => {
    const markup = layout(worktree);
    expect(count(markup, 'class="thread-changes"')).toBe(2);
    expect(markup).toContain('aria-label="3 files changed +41 −12"');
    expect(markup).toContain('aria-label="Review the changes from this turn: 3 files changed +41 −12"');
    // The directory dimmed, the name in the text colour.
    expect(markup).toContain('<span class="thread-file-path"><span class="thread-file-dir">src/auth/</span>'
      + '<span class="thread-file-name">retry.ts</span></span>');
    expect(markup).toContain("+ 2 more");
    // The header's lines, then their five squares; each file row leads with
    // its type's badge, and what git would call the change is only spoken.
    expect(markup).toContain('<span class="conv-stat-del">−12</span></span><span class="diffstat-bar" '
      + 'aria-hidden="true"><i class="is-added"></i><i class="is-added"></i><i class="is-added"></i>'
      + '<i class="is-added"></i><i class="is-removed"></i></span><button type="button" class="thread-review"');
    expect(markup).toContain('<li class="thread-changes-file" title="/w/src/auth/retry.ts">'
      + '<span class="file-badge kind-ts" aria-hidden="true">TS</span>'
      + '<span class="conv-sr">modified</span><span class="thread-file-path">');
    expect(markup).not.toContain("thread-file-status");
    // A plain chat cannot show a turn's diff: the card, without Review.
    const chat = layout({ ...local, project: false });
    expect(count(chat, 'class="thread-changes"')).toBe(2);
    expect(chat).not.toContain("thread-review");
  });

  it("draws the turn's controls once: always under the latest answer, on hover under the others", () => {
    const markup = layout(local);
    expect(count(markup, 'class="thread-answer is-latest"')).toBe(1);
    expect(count(markup, 'class="thread-answer"')).toBe(1);
    expect(count(markup, 'class="thread-turn-foot is-latest"')).toBe(1);
    // Icons, each named and titled (the Threads look, owner, 2026-10-23).
    expect(count(markup, 'class="thread-act thread-fork"')).toBe(2);
    expect(count(markup, 'title="Fork from here"><svg class="thread-glyph" width="15"')).toBe(2);
    expect(markup).toContain("a new thread with the first 1 message of this one");
    expect(markup).toContain("a new thread with the first 2 messages of this one");
    expect(count(markup, '<button type="button" class="thread-act thread-undo" '
      + 'aria-label="Restore files to before this message" title="Undo · 1 file"><svg')).toBe(2);
    expect(count(markup, 'aria-label="Copy answer"')).toBe(2);
    // The floating Fork row and the lone turn clock are gone.
    expect(markup).not.toContain("conv-turn-end");
    expect(markup).not.toContain("conv-turn-clock");
    // The latest answer's time is when Rust stamped its run's end, after the
    // icons: Copy, Fork, Undo, then the time.
    const foot = markup.slice(markup.indexOf('class="thread-turn-foot is-latest"'));
    expect(foot).toContain(`dateTime="${new Date(SENT + 101_000).toISOString()}"`);
    const controls = foot.slice(0, foot.indexOf("</div>"));
    expect(controls.indexOf("Copy answer")).toBeLessThan(controls.indexOf("thread-fork"));
    expect(controls.indexOf("thread-fork")).toBeLessThan(controls.indexOf("thread-undo"));
    expect(controls.indexOf("thread-undo")).toBeLessThan(controls.indexOf('<time class="thread-time"'));
  });

  it("drops Undo turn on a worktree thread, and holds Fork and Edit while one is on its way", () => {
    expect(layout(worktree)).not.toContain("thread-undo");
    const busy = layout({ ...worktree, busy: true });
    expect(count(busy, 'title="Fork from here" disabled=""')).toBe(2);
    expect(count(busy, 'title="Edit from here" disabled=""')).toBe(2);
  });

  it("leaves the live turn unfolded, with no end block, and the turn before it latest", () => {
    const markup = layout(worktree, { thinking: true });
    expect(count(markup, 'class="thread-fold"')).toBe(1);
    expect(markup).toContain('title="/w/test/login.test.ts"');
    expect(count(markup, 'class="thread-changes"')).toBe(1);
    expect(count(markup, 'class="thread-turn-foot is-latest"')).toBe(1);
    expect(count(markup, 'class="thread-act thread-fork"')).toBe(1);
    // The run draws itself under its prompt (owner, 2026-10-07): no strip above the composer.
    expect(markup).not.toContain("conv-working");
    expect(count(markup, 'class="thread-live"')).toBe(1);
    expect(markup.indexOf("Now add a regression test")).toBeLessThan(markup.indexOf('class="thread-live"'));
    expect(markup).toContain('aria-label="Read test/login.test.ts, done" title="/w/test/login.test.ts"');
    // The live region hears the state, never the clock.
    expect(markup).toContain('<span class="conv-sr" role="status" aria-live="polite" aria-atomic="true">Working</span>');
    expect(markup).toMatch(/<span class="thread-live-clock" aria-hidden="true">\d+:\d\d(?::\d\d)?<\/span>/u);
  });

  it("clips the transcript's region, so nothing inside it can scroll the tile", () => {
    // Absolutely placed screen-reader spans in the list take .conv-scroll as
    // their containing block; overflowing, they let the browser scroll the
    // tile body mid-run and slide the pane up its tile (7 Oct, hands-free).
    const css = readFileSync(new URL("./components/ThreadTranscript.css", import.meta.url), "utf8");
    expect(css).toContain(".conv.is-thread-layout > .conv-scroll { overflow:clip; }");
  });

  it("keeps the bottom working strip everywhere but the live turn", () => {
    expect(paint(worktree, { items: WORK, thinking: true })).toContain('class="conv-working"');
    // A Threads pane with no prompt yet has no live turn to say it.
    expect(layout(worktree, { items: [], thinking: true })).toContain('class="conv-working"');
  });

  it("docks a waiting approval out of the transcript: the step waits, no card", () => {
    const asking: ConvItem[] = [...WORK.slice(0, 1),
      { kind: "approval", id: "ap", tool: "Bash", summary: "npm test -- auth",
        input: { command: "npm test -- auth" } }];
    const active = new Set(["ap"]);
    const transcript = (markup: string): string => markup.slice(markup.indexOf('class="conv-list"'),
      markup.indexOf('<div class="conv-sr" role="status"'));
    const whole = layout(worktree, { items: asking, thinking: true, activeApprovalIds: active });
    const markup = transcript(whole);
    expect(markup).not.toContain("conv-approval");
    expect(markup).toContain('<span class="thread-live-verb is-waiting">Waiting for you</span>');
    expect(markup).toContain('aria-label="Run npm test -- auth, needs your OK, waiting"');
    // The clock beside "Waiting for you" is the wait's own, without repeating the word.
    expect(markup).toMatch(/<span class="thread-live-clock is-waiting" aria-hidden="true">\d+:\d\d<\/span>/u);
    // Docked above the composer (owner, 2026-10-07), never in the transcript.
    expect(whole).toContain('<span class="thread-dock-title">Run a command?</span>');
    expect(whole.indexOf('class="thread-dock"')).toBeGreaterThan(whole.indexOf('class="conv-strip"'));
    expect(whole.slice(0, whole.indexOf('class="conv-strip"'))).not.toContain("conv-approval");
    const acts = whole.slice(whole.indexOf('class="thread-dock-acts"'));
    expect(acts.indexOf(">Deny<")).toBeLessThan(acts.indexOf(">Allow once<"));
    expect(whole).toContain("thread-dock-card is-pending");
    // Without the flag the card is what it was.
    const plain = paint(worktree, { items: asking, thinking: true, activeApprovalIds: active });
    expect(plain).toContain('<span class="conv-name">Approval needed</span>');
    expect(plain).toContain(">Allow<");
    expect(plain).not.toContain("Allow once");
    expect(plain).not.toContain("thread-dock");
    // An approval still waiting never folds, even in a turn the transcript has
    // closed: the step still waits in the transcript and the card stays docked.
    const waitingWhole = layout(worktree, { items: [...asking, ...WORK.slice(7)],
      activeApprovalIds: active });
    const waiting = transcript(waitingWhole);
    expect(waiting).toContain('aria-label="Run npm test -- auth, needs your OK, waiting"');
    expect(waiting).not.toContain("conv-approval");
    expect(waitingWhole).toContain("Run a command?");
  });

  it("opens the fold that holds the subagent the detail panel shows", () => {
    const agent: ConvItem = { kind: "subagent", id: "agent-1", subagentType: "Explore",
      description: "Find the retry", status: "done", children: [], toolCount: 3, subagentCount: 0 };
    const items = [...WORK.slice(0, 2), agent, ...WORK.slice(2)];
    expect(count(layout(worktree, { items }), 'aria-expanded="false" title="Show the work"')).toBe(2);
    const selected = layout(worktree, { items, selectedSubagentKey: "agent-1",
      onSelectSubagent: () => undefined });
    expect(count(selected, 'aria-expanded="true" title="Hide the work"')).toBe(1);
    expect(selected).toContain('data-agent-key="agent-1"');
  });

  it("opens every fold at Detailed, and keeps them folded at Balanced and Compact", () => {
    const transcript = (density: TranscriptDensity): string => renderToStaticMarkup(
      createElement(ConversationTranscript, {
        items: WORK, rows: WORK.map(item => ({ kind: "item" as const, key: `l:${item.id}`,
          origin: "live" as const, item })),
        thinking: false, live: true, sessionKey: "codex#t", threadLayout: true, pathRoot: "/w",
        config: { effort: null, models: [], canSetModel: true, effortLevels: [], showEffort: false,
          canSetEffort: true, pendingModel: false, pendingEffort: false, error: null },
        approvals: {}, activeApprovalIds: new Set<string>(), checkingApprovalIds: new Set<string>(),
        ownerApprovalIds: new Set<string>(), onApprove: () => undefined, onDeny: () => undefined,
        onStop: () => undefined, thread: worktree, onThreadForkAt: () => undefined,
        density, densityEpoch: 0, rootRef: { current: null }, controls: { current: null },
      }));
    const detailed = transcript("detailed");
    expect(count(detailed, 'aria-expanded="true" title="Hide the work"')).toBe(2);
    expect(detailed).toContain("Looking at the retry.");
    for (const folded of [transcript("balanced"), transcript("compact")]) {
      expect(count(folded, 'aria-expanded="false" title="Show the work"')).toBe(2);
      expect(folded).not.toContain("Looking at the retry.");
    }
  });
});

/* ==========================================================================
   THE LIVE TURN (owner, 2026-10-07): a running turn draws itself under its
   prompt — a header saying what the agent is doing now, a railed trail of its
   work with its prose between, a waiting step, a steer — and once finished it
   is drawn as every finished turn is, its steer folded in and its time kept.
   ========================================================================== */

describe("the live turn", () => {
  beforeEach(() => { forgetThreadTurnTimings(); });
  const prompt = "Now add a regression test";
  const running: ConversationThread = { project: true, worktree: true, revertFilesBlocked: null,
    turns: [WORK_TURNS[0], { ...WORK_TURNS[1], state: "running", hasEnd: false }],
    lastRun: { startedAt: SENT + 60_000 } };
  const edit = (result?: { ok: boolean; summary: string }): ConvItem => ({ kind: "tool", id: "edit-2",
    tool: "Edit", summary: "/w/test/login.test.ts", ...(result === undefined ? {} : { result }),
    input: { file_path: "/w/test/login.test.ts", old_string: "a",
      new_string: "describe(retry)\n\n  expect(calls).toBe(5)\n  done()\n  more()\n  hidden()" } });
  const LIVE: ConvItem[] = [...WORK.slice(0, 7),
    { kind: "user", id: "user-3", text: prompt },
    { kind: "tool", id: "read-2", tool: "Read", summary: "/w/test/login.test.ts",
      input: { file_path: "/w/test/login.test.ts" }, result: { ok: true, summary: "read" } },
    { kind: "text", id: "text-4", text: "Adding the test.", streaming: false },
    edit(),
    // Claude's mapper emits the diff from the call's input before any result.
    { kind: "diff", id: "diff-2", path: "/w/test/login.test.ts", callId: "edit-2", added: 6, removed: 1 },
  ];
  const live = (items: ConvItem[], over: Partial<ConversationProps> = {}): string =>
    paint(running, { items, threadLayout: true, pathRoot: "/w", thinking: true, ...over });

  it("says the newest running step under the prompt, verb then project-relative target", () => {
    const markup = live(LIVE);
    expect(markup).toContain('<span class="thread-live-verb">Editing</span>'
      + '<span class="thread-live-target is-mono">test/login.test.ts</span>');
    expect(markup).not.toContain('class="thread-live-target is-mono">/w/');
    // The prose sits where it was said: two trail segments around it.
    expect(count(markup, 'class="thread-trail"')).toBe(2);
    expect(markup.indexOf("Read test/login.test.ts, done")).toBeLessThan(markup.indexOf("Adding the test."));
    expect(markup.indexOf("Adding the test.")).toBeLessThan(markup.indexOf("Editing test/login.test.ts, running"));
  });

  it("prints a step's lines only once its call has a result, and peeks at a running edit", () => {
    const runningRow = live(LIVE).slice(live(LIVE).indexOf('aria-label="Editing test/login.test.ts, running"'));
    expect(runningRow).not.toContain("thread-step-stat");
    expect(runningRow).not.toContain("+6");
    // At most four lines of what the edit writes, blank ones skipped.
    const peek = runningRow.slice(runningRow.indexOf('class="thread-peek"'));
    expect(count(peek.slice(0, peek.indexOf("</div></div>") + 12), 'class="thread-peek-line"')).toBe(4);
    expect(peek).toContain(">describe(retry)<");
    expect(peek).not.toContain("hidden()");
    const finished = live([...LIVE.slice(0, -2), edit({ ok: true, summary: "Edited" }), LIVE[LIVE.length - 1]]);
    expect(finished).toContain('aria-label="Edited test/login.test.ts, 6 lines added, 1 removed, done"');
    expect(finished).toContain('<span class="conv-stat-add">+6</span><span class="conv-stat-del">−1</span>');
    expect(finished).not.toContain('class="thread-peek"');
  });

  it("says Thinking in its status line until the turn shows anything, with no skeleton", () => {
    // The two shimmering bars went with the Threads look (owner, 2026-10-23).
    const empty = live([...WORK.slice(0, 7), { kind: "user", id: "user-3", text: prompt }]);
    expect(empty).not.toContain("thread-live-bar");
    expect(empty).not.toContain("thread-live-skeleton");
    expect(empty).toContain('<div class="thread-live-inner"><div class="thread-live-status is-thinking">'
      + '<span class="thread-live-mark" aria-hidden="true"><svg class="thinking-mark thread-live-pair" '
      + 'width="16" height="16"');
    expect(empty).toContain('<span class="thread-live-verb">Thinking</span>');
    expect(live(LIVE)).not.toContain("thread-live-bar");
  });

  it("draws the status line under the trail, with the Pair mark and never the Wave", () => {
    const markup = live(LIVE);
    const turn = markup.slice(markup.indexOf('class="thread-live"'));
    expect(turn.indexOf('class="thread-live-status is-working"'))
      .toBeGreaterThan(turn.lastIndexOf('class="thread-trail"'));
    expect(turn.indexOf('class="thread-live-status is-working"'))
      .toBeGreaterThan(turn.indexOf("Editing test/login.test.ts, running"));
    expect(turn).toContain('<svg class="thinking-mark thread-live-pair" width="16" height="16"');
    expect(markup).not.toMatch(/working-indicator|working-wave/u);
    // Waiting, an amber dot takes the mark's place.
    const bash: ConvItem = { kind: "tool", id: "bash-2", tool: "Bash", summary: "npm test -- login",
      input: { command: "npm test -- login" } };
    const ask: ConvItem = { kind: "approval", id: "ap", tool: "Bash", summary: "npm test -- login",
      input: { command: "npm test -- login" } };
    const waiting = live([...LIVE, bash, ask], { activeApprovalIds: new Set(["ap"]) });
    expect(waiting).toContain('<div class="thread-live-status is-waiting"><span class="thread-live-mark" '
      + 'aria-hidden="true"><span class="thread-live-dot"></span></span>');
  });

  it("puts each step's tool glyph on the rail and its state on the right", () => {
    const markup = live(LIVE);
    const row = (label: string): string => {
      const at = markup.indexOf(`aria-label="${label}"`);
      return markup.slice(at, markup.indexOf("</button>", at));
    };
    // The read wears the read glyph and nothing on the right: done, no lines.
    const read = row("Read test/login.test.ts, done");
    expect(read).toContain('<span class="thread-step-node is-done" aria-hidden="true"><svg class="thread-glyph"');
    expect(read).toContain('<path d="M9 1.75H4.75');
    expect(read).not.toContain("thread-step-end");
    // The running edit wears the edit glyph, and its turning ring sits on the right.
    const editing = row("Editing test/login.test.ts, running");
    expect(editing).toContain('<path d="M10.25 3.1');
    expect(editing).toContain('<span class="thread-step-end"><span class="thread-step-ring is-running" '
      + 'aria-hidden="true"></span></span>');
    expect(editing.indexOf("thread-step-words")).toBeLessThan(editing.indexOf("thread-step-end"));
  });

  it("turns the call an approval is about into the waiting step, and never draws the card", () => {
    const bash: ConvItem = { kind: "tool", id: "bash-2", tool: "Bash", summary: "npm test -- login",
      input: { command: "npm test -- login" } };
    const ask: ConvItem = { kind: "approval", id: "ap", tool: "Bash", summary: "npm test -- login",
      input: { command: "npm test -- login" } };
    const markup = live([...LIVE, bash, ask], { activeApprovalIds: new Set(["ap"]) });
    expect(markup).toContain('<span class="thread-live-verb is-waiting">Waiting for you</span>');
    expect(count(markup, '<span class="thread-step-meta">needs your OK</span>')).toBe(1);
    expect(markup).toContain('aria-label="Run npm test -- login, needs your OK, waiting"');
    expect(markup).not.toContain("conv-approval");
    // Answered, the request is a quiet done row and the call runs on.
    const allowed = live([...LIVE, bash, { ...ask, resolved: "allow" } as ConvItem]);
    expect(allowed).toContain('aria-label="Allowed Run npm test -- login, done"');
    expect(allowed).toContain('<span class="thread-live-verb">Running</span>');
  });

  it("draws a steer inside the run it arrived in, never as a turn of its own", () => {
    const steered: ConvItem[] = [...LIVE, { kind: "user", id: "steer-5", text: "Use five retries, not three" },
      { kind: "text", id: "text-6", text: "Switching to five.", streaming: false }];
    const markup = live(steered);
    expect(markup).toContain('<span class="thread-steer-label">You steered</span>'
      + '<span class="thread-steer-text" title="Use five retries, not three">Use five retries, not three</span>');
    expect(count(markup, 'class="thread-user"')).toBe(2);
    expect(count(markup, 'class="thread-live"')).toBe(1);
    // Finished, the steer folds into the work line, counted once with the rest.
    const done: ConversationThread = { ...running, turns: [WORK_TURNS[0], WORK_TURNS[1]],
      lastRun: { startedAt: SENT + 60_000, endedAt: SENT + 94_000, outcome: "completed" } };
    const finished = paint(done, { items: [...steered.slice(0, -4), edit({ ok: true, summary: "Edited" }),
      ...steered.slice(-3)], threadLayout: true, pathRoot: "/w" });
    expect(count(finished, 'class="thread-fold"')).toBe(2);
    expect(finished).toContain('aria-label="Worked for 34s · read 1 file, edited 1 file, steered once"');
    expect(finished).toContain('<span class="thread-work-chip is-steer">steered 1</span>');
    expect(finished).not.toContain("Use five retries, not three");
    expect(finished).toContain("Switching to five.");
  });

  it("keeps a finished turn's time once the next turn is sent", () => {
    const first: ConversationThread = { project: true, worktree: true, revertFilesBlocked: null,
      turns: [WORK_TURNS[0]], lastRun: { startedAt: SENT, endedAt: SENT + 34_000, outcome: "completed" } };
    const before = paint(first, { items: WORK.slice(0, 7), threadLayout: true, pathRoot: "/w" });
    expect(before).toContain('<span class="thread-fold-lead">Worked for 34s</span>');
    // Turn 2 is sent: Rust's one lastRun is now turn 2's, and turn 1 keeps its figure.
    const after = paint({ ...first, turns: WORK_TURNS, lastRun: { startedAt: SENT + 60_000 } },
      { items: WORK, threadLayout: true, pathRoot: "/w" });
    expect(after).toContain('<span class="thread-fold-lead">Worked for 34s</span>');
    expect(after).toContain(`dateTime="${new Date(SENT + 34_000).toISOString()}"`);
  });
});

describe("the Edit from here dialog", () => {
  it("offers both choices, and disables restoring the files with the reason", () => {
    const open = renderToStaticMarkup(createElement(ThreadEditDialog, {
      ordinal: 2, revertFilesBlocked: null, onChoose: () => Promise.resolve(), onCancel: () => undefined,
    }));
    expect(open).toContain(">Edit from here</h2>");
    expect(open).toContain(threadEditDescription());
    expect(open).toContain(`>${KEEP_FILES_LABEL}<`);
    expect(KEEP_FILES_LABEL).toBe("Keep the current files");
    expect(RESTORE_FILES_LABEL).toBe("Restore the files to before this message");
    expect(open).toMatch(new RegExp(`<button type="button" class="confirm-dialog-button confirm-dialog-danger" title="[^"]+">${RESTORE_FILES_LABEL}<`, "u"));
    const blocked = renderToStaticMarkup(createElement(ThreadEditDialog, {
      ordinal: 2, revertFilesBlocked: REVERT_LOCAL_BLOCKED, onChoose: () => Promise.resolve(),
      onCancel: () => undefined,
    }));
    expect(blocked).toContain(REVERT_LOCAL_BLOCKED);
    expect(blocked).toMatch(/confirm-dialog-danger" disabled="" title="This thread works in the project/u);
  });

  it("says a new thread starts from the earlier message and this one is settled", () => {
    expect(threadEditDescription()).toContain("A new thread starts from just before this message");
    expect(threadEditDescription()).toContain("This thread is settled");
    const later = renderToStaticMarkup(createElement(ThreadEditDialog, {
      ordinal: 1, revertFilesBlocked: null, onChoose: () => Promise.resolve(), onCancel: () => undefined,
    }));
    expect(threadEditCardNote(1)).toBeNull();
    expect(later).not.toContain("Un-settle");
  });

  it("says the first message opens the card, and how to un-settle if it is closed", () => {
    const first = renderToStaticMarkup(createElement(ThreadEditDialog, {
      ordinal: 0, revertFilesBlocked: null, onChoose: () => Promise.resolve(), onCancel: () => undefined,
    }));
    const note = threadEditCardNote(0)!;
    expect(note).toContain("New thread card opens on the same workspace");
    expect(note).toContain("this thread stays settled; Un-settle it from its menu");
    expect(first).toContain(note);
    expect(first).toContain(threadEditDescription());
  });
});

describe("the thread pane head", () => {
  it("shows the worktree branch, or that the worktree is gone", () => {
    const chip = renderToStaticMarkup(createElement(ThreadPaneHead, { workspace: worktreeThread.workspace }));
    expect(chip).toContain("iface/fix-the-flaky-login-test-3f9a2c");
    // One plain line (owner, 2026-11-15): "on <branch>", then where edits
    // land in words, each after a quiet dot; no glyph, no chip.
    expect(chip).toContain('on <span class="thread-head-branch-name">iface/fix-the-flaky-login-test-3f9a2c</span></span>'
      + '<span class="thread-head-dot" aria-hidden="true">·</span>'
      + `<span class="thread-head-where" title="iface/fix-the-flaky-login-test-3f9a2c · Worktree · `);
    expect(chip).toContain(`>${WORKTREE_WHERE_WORDS}</span>`);
    expect(chip).not.toContain("thread-glyph");
    // A worktree Rust recorded no branch for is named by its folder.
    const unnamed = renderToStaticMarkup(createElement(ThreadPaneHead, {
      workspace: { ...worktreeThread.workspace!, branch: undefined } }));
    expect(unnamed).not.toContain("thread-head-branch-name");
    expect(unnamed).toContain(">edits land in the worktree fix-the-flaky-login-test-3f9a2c</span>");
    const gone = renderToStaticMarkup(createElement(ThreadPaneHead, {
      workspace: { ...worktreeThread.workspace!, missing: true } }));
    expect(gone).toContain(MISSING_WORKTREE_TEXT);
    expect(gone).not.toContain("thread-head-branch-name");
    expect(gone).not.toContain(WORKTREE_WHERE_WORDS);
  });

  it("names the thread it was forked from, and where a local thread works", () => {
    const forked = renderToStaticMarkup(createElement(ThreadPaneHead, {
      workspace: localThread.workspace,
      lineage: { relation: "fork", title: "Fix the flaky login test", onSelect: () => undefined },
    }));
    expect(forked).toContain("Forked from ");
    expect(forked).toContain("Fix the flaky login test");
    expect(forked.indexOf(LOCAL_WHERE_WORDS)).toBeLessThan(forked.indexOf("Forked from "));
    // Every project thread says where its edits land (owner, 2026-11-15): a
    // local one in the project folder, after "on <branch>" when Rust recorded one.
    const local = renderToStaticMarkup(createElement(ThreadPaneHead, { workspace: localThread.workspace }));
    expect(local).toContain(`<span class="thread-head-where" title="Local">${LOCAL_WHERE_WORDS}</span>`);
    expect(local).not.toContain("thread-head-branch-name");
    expect(local).not.toContain(">Local<");
    const named = renderToStaticMarkup(createElement(ThreadPaneHead, {
      workspace: { ...localThread.workspace!, branch: "main" } }));
    expect(named).toContain('<span class="thread-head-branch" title="main · Local">on '
      + '<span class="thread-head-branch-name">main</span></span>');
    expect(named.indexOf("thread-head-branch-name")).toBeLessThan(named.indexOf(LOCAL_WHERE_WORDS));
    // A plain chat says it edits no files.
    const chat = renderToStaticMarkup(createElement(ThreadPaneHead, {
      workspace: { mode: "none", projectRoot: "", owned: false } }));
    expect(chat).toContain(`>${CHAT_WHERE_WORDS}</span>`);
    expect(threadHeadWhere({ ...localThread.workspace!, branch: " main " }))
      .toEqual({ branch: "main", where: LOCAL_WHERE_WORDS });
    expect(threadHeadWhere(undefined)).toBeNull();
    // A Local thread whose record names no branch takes the one Rust last
    // read its checkout on; a recorded branch wins.
    const checkedOut = renderToStaticMarkup(createElement(ThreadPaneHead, {
      workspace: localThread.workspace, checkoutBranch: "main" }));
    expect(checkedOut).toContain('on <span class="thread-head-branch-name">main</span>');
    expect(threadHeadWhere({ ...localThread.workspace!, branch: "feature" }, "main"))
      .toEqual({ branch: "feature", where: LOCAL_WHERE_WORDS });
    expect(threadWorkspaceLabel({ ...localThread.workspace!, branch: "main" })).toBe("main · Local");
    expect(threadWorkspaceLabel({ ...localThread.workspace!, branch: undefined })).toBe("Local");
    expect(threadWorkspaceLabel({ mode: "none", projectRoot: "", owned: false })).toBeNull();
    expect(threadWorkspaceLabel(undefined)).toBeNull();
  });

  it("totals the thread's measured changes and never prints an unknown as zero", () => {
    expect(threadChangeTotals({ turns: [] })).toBeNull();
    expect(threadChangeTotals({ turns: [turn(0, "a")] })).toBeNull();
    expect(threadChangeTotals({ turns: [
      turn(0, "a", { added: 8, removed: 1 }), turn(1, "b"), turn(2, "c", { added: 16, removed: 0 }),
    ] })).toEqual({ added: 24, removed: 1 });
  });

  it("draws the head's Changes totals with their five squares, and neither when unknown", () => {
    const toggle = (totals?: { added: number; removed: number } | null): string =>
      renderToStaticMarkup(createElement(ThreadDiffToggle, { open: false, onToggle: () => undefined, totals }));
    expect(toggle({ added: 26, removed: 1 })).toContain('<span class="thread-head-removed">−1</span></span>'
      + '<span class="diffstat-bar" aria-hidden="true"><i class="is-added"></i><i class="is-added"></i>'
      + '<i class="is-added"></i><i class="is-added"></i><i class="is-removed"></i></span></button>');
    expect(toggle(null)).not.toContain("diffstat-bar");
    expect(toggle()).not.toContain("diffstat-bar");
  });

  it("shows the pull request as a chip that opens it through the external-link handler", () => {
    const pullRequest = worktreeThread.pullRequest!;
    const chip = renderToStaticMarkup(createElement(ThreadPaneHead, {
      workspace: worktreeThread.workspace, pullRequest }));
    expect(threadPullRequestLabel(pullRequest)).toBe("PR #482 · open");
    // An anchor with target _blank is what the main window's open handler
    // validates and hands to the OS; the renderer never calls window.open.
    expect(chip).toContain(`<a class="thread-head-pr is-open" href="${pullRequest.url}" target="_blank" rel="noreferrer noopener"`);
    expect(chip).toContain("<span>PR #482 · open</span></a>");
    expect(chip.indexOf("thread-head-branch")).toBeLessThan(chip.indexOf("thread-head-pr"));
    // A local thread with a pull request still shows it; merged and closed say so.
    const merged = renderToStaticMarkup(createElement(ThreadPaneHead, {
      workspace: localThread.workspace, pullRequest: { ...pullRequest, state: "merged" } }));
    expect(merged).toContain("PR #482 · merged");
    // Anything but a web address is words, not a link.
    expect(pullRequestHref("javascript:alert(1)")).toBeUndefined();
    expect(pullRequestHref("file:///etc/passwd")).toBeUndefined();
    expect(pullRequestHref("https://user:secret@github.com/o/r/pull/1")).toBeUndefined();
    expect(pullRequestHref(" https://github.com/o/r/pull/1 ")).toBe("https://github.com/o/r/pull/1");
    const unsafe = renderToStaticMarkup(createElement(ThreadPaneHead, {
      pullRequest: { ...pullRequest, url: "javascript:alert(1)", state: "closed" } }));
    expect(unsafe).toContain('<span class="thread-head-pr is-closed" title="PR #482 · closed">');
    expect(unsafe).not.toContain("<a ");
    const head = readFileSync(new URL("./components/ThreadPaneHead.tsx", import.meta.url), "utf8");
    expect(head).not.toContain("window.open");
  });

  it("sits in the tile head after the title, with the Diff toggle among the actions", () => {
    const markup = renderToStaticMarkup(createElement(AgentTile, {
      session: { engine: "codex", sessionId: "t", title: "Fix the flaky login test", cwd: "/w",
        status: "idle", cloud: false, lastActivity: "", firstSeen: "", archived: false },
      selected: true, maximized: false, live: true, engineLabel: "Codex",
      onFocus: () => undefined, onToggleMaximize: () => undefined, onClose: () => undefined,
      headDetail: createElement(ThreadPaneHead, { workspace: worktreeThread.workspace }),
      headActions: createElement(ThreadDiffToggle, { open: true, onToggle: () => undefined }),
      children: "body",
    }));
    expect(markup.indexOf("workspace-tile-name")).toBeLessThan(markup.indexOf("thread-head-branch"));
    expect(markup).toContain('aria-pressed="true"');
    expect(markup).toContain('aria-label="Hide changes"');
  });
});

describe("thread pane commands", () => {
  function port(answer: Partial<ThreadControlResponse> | Error,
      modes: Record<string, string> = {},
      options: { working?: boolean; refuse?: ThreadCommand["kind"] } = {}) {
    const calls: unknown[] = [];
    const drafts: [string, string][] = [];
    const selected: string[] = [];
    const cards: unknown[] = [];
    const applied: [string, string][] = [];
    const closed: string[] = [];
    const notices: string[] = [];
    const steps: string[] = [];
    const value: ThreadPanePort = {
      async request(command) {
        calls.push(command);
        steps.push(command.kind);
        // An edit closes the original's session, which drops its mode.
        if (command.kind === "editFrom") delete modes[command.key];
        if (options.refuse === command.kind) throw new Error(`Rust refused ${command.kind}.`);
        if (answer instanceof Error) throw answer;
        return { view, ...answer } as ThreadControlResponse;
      },
      select: key => { selected.push(key); steps.push("select"); },
      setDraft: (key, text) => { drafts.push([key, text]); },
      openCard: (project, workspace, prompt, note) => {
        cards.push({ project, workspace, prompt, ...note === undefined ? {} : { note } });
        steps.push("card");
      },
      permissionModeOf: key => modes[key],
      setPermissionMode: (key, mode) => { applied.push([key, mode]); },
      working: () => options.working === true,
      closeSettled: key => { closed.push(key); steps.push("close"); },
      notify: message => { notices.push(message); steps.push("notify"); },
    };
    return { value, calls, drafts, selected, cards, applied, closed, notices, steps };
  }

  it("edits from a turn: editFrom, then the new thread with the prompt in its draft", async () => {
    const fake = port({ key: "codex#new", prompt: "Now add a regression test",
      restore: { restored: 2, recoveryRef: "refs/interface/threads/x/recovery/7" } });
    await editThreadFrom(fake.value, worktreeThread, 1, true, "typed text");
    expect(fake.calls).toEqual([{ kind: "editFrom", key: worktreeThread.key, ordinal: 1, restoreFiles: true }]);
    expect(fake.drafts).toEqual([["codex#new", "Now add a regression test"]]);
    expect(fake.selected).toEqual(["codex#new"]);
    // Rust restored the files: the owner is told where the old ones are kept.
    expect(fake.notices).toEqual(["Restored 2 files to before message 2. "
      + "Recovery point saved: refs/interface/threads/x/recovery/7"]);
    // Rust settles and closes the original itself for a later turn.
    expect(fake.closed).toEqual([]);
  });

  it("edits from turn 1: settles this thread, closes its session, then opens the card on its worktree", async () => {
    const fake = port({});
    await editThreadFrom(fake.value, worktreeThread, 0, false, "Fix the flaky login test");
    expect(fake.calls).toEqual([{ kind: "settle", key: worktreeThread.key }]);
    expect(fake.steps).toEqual(["settle", "close", "card"]);
    expect(fake.closed).toEqual([worktreeThread.key]);
    expect(fake.cards).toEqual([{ project: "/Users/owner/Developer/Projects/atlas",
      workspace: { mode: "existing", worktreePath: worktreeThread.workspace!.worktreePath },
      prompt: "Fix the flaky login test" }]);
    expect(fake.notices).toEqual([]);
  });

  it("says in the New thread card that turn 1's files were restored, since the card covers the pane", async () => {
    // Hands-free run, 6 Oct 2026: the recovery notice sat behind the card.
    const restored = port({ restore: { restored: 3, recoveryRef: "refs/interface/threads/x/recovery/2" } });
    await editThreadFrom(restored.value, worktreeThread, 0, true, "p");
    expect(restored.cards).toMatchObject([{ note: "Files restored to before this message — recovery point saved" }]);
    const kept = port({});
    await editThreadFrom(kept.value, worktreeThread, 0, false, "p");
    expect(kept.cards).toEqual([expect.not.objectContaining({ note: expect.anything() })]);
  });

  it("restores the files before turn 1 first, saying where the recovery point is", async () => {
    // Hands-free run, 6 Oct 2026: "Revert files too" on a thread's first
    // prompt opened the card and left the files as they were.
    const fake = port({ restore: { restored: 1, recoveryRef: "refs/interface/threads/x/recovery/1" } });
    await editThreadFrom(fake.value, worktreeThread, 0, true, "Fix the flaky login test");
    expect(fake.calls).toEqual([{ kind: "restoreTurn", key: worktreeThread.key, ordinal: 0 },
      { kind: "settle", key: worktreeThread.key }]);
    expect(fake.steps).toEqual(["restoreTurn", "notify", "settle", "close", "card"]);
    expect(fake.notices).toEqual(["Restored 1 file to before message 1. "
      + "Recovery point saved: refs/interface/threads/x/recovery/1"]);
    expect(fake.cards).toEqual([{ project: "/Users/owner/Developer/Projects/atlas",
      workspace: { mode: "existing", worktreePath: worktreeThread.workspace!.worktreePath },
      prompt: "Fix the flaky login test", note: THREAD_FIRST_EDIT_RESTORED }]);
    // Refused: Rust's sentence; nothing settled, closed or opened.
    const refused = port(new Error("Another session uses this worktree."));
    await expect(editThreadFrom(refused.value, worktreeThread, 0, true, "x"))
      .rejects.toThrow("Another session uses this worktree.");
    expect(refused.steps).toEqual(["restoreTurn"]);
    expect(refused.cards).toEqual([]);
  });

  it("opens no card when this thread cannot be settled, and says the files were restored", async () => {
    const fake = port({ restore: { restored: 1, recoveryRef: "refs/r" } }, {}, { refuse: "settle" });
    await expect(editThreadFrom(fake.value, worktreeThread, 0, true, "x"))
      .rejects.toThrow("The files were restored, but this thread was not settled: Rust refused settle.");
    expect(fake.cards).toEqual([]);
    expect(fake.closed).toEqual([]);
    const kept = port({}, {}, { refuse: "settle" });
    await expect(editThreadFrom(kept.value, worktreeThread, 0, false, "x")).rejects.toThrow("Rust refused settle.");
  });

  it("restores a worktree turn's files only, saying where the recovery point is", async () => {
    const fake = port({ restore: { restored: 3, recoveryRef: "refs/interface/threads/x/recovery/4" } });
    await restoreThreadFiles(fake.value, worktreeThread, 1);
    expect(fake.calls).toEqual([{ kind: "restoreTurn", key: worktreeThread.key, ordinal: 1 }]);
    expect(fake.steps).toEqual(["restoreTurn", "notify"]);
    expect(fake.notices).toEqual(["Restored 3 files to before message 2. "
      + "Recovery point saved: refs/interface/threads/x/recovery/4"]);
    // Nothing settled, closed, opened or selected: the thread goes on as it was.
    expect(fake.closed).toEqual([]);
    expect(fake.cards).toEqual([]);
    expect(fake.selected).toEqual([]);
    // Refused: Rust's sentence, and no notice claims a restore happened.
    const refused = port(new Error("Another session uses this worktree."));
    await expect(restoreThreadFiles(refused.value, worktreeThread, 1))
      .rejects.toThrow("Another session uses this worktree.");
    expect(refused.steps).toEqual(["restoreTurn"]);
    expect(refused.notices).toEqual([]);
  });

  it("promises only what a worktree files restore guarantees", () => {
    expect(threadFilesRestorePoint(1)).toEqual({
      description: "Restores this worktree's files to before message 2. "
        + "The current files are saved to a recovery point first. The conversation is not changed.",
    });
  });

  it("refuses turn 1 while the agent is still working, as Rust does for later turns", async () => {
    const fake = port({}, {}, { working: true });
    await expect(editThreadFrom(fake.value, worktreeThread, 0, true, "x")).rejects.toThrow(THREAD_STILL_WORKING);
    expect(fake.steps).toEqual([]);
  });

  it("starts turn 1 again in a new worktree, never Local, when the worktree is gone", async () => {
    const gone: ThreadView = { ...worktreeThread, workspace: { ...worktreeThread.workspace!, missing: true } };
    const fake = port({});
    await editThreadFrom(fake.value, gone, 0, false, "Fix the flaky login test");
    expect(fake.cards).toEqual([{ project: "/Users/owner/Developer/Projects/atlas",
      workspace: { mode: "worktree", baseRef: "main" }, prompt: "Fix the flaky login test" }]);
  });

  it("rejects with Rust's sentence and opens nothing when refused", async () => {
    const fake = port(new Error("Another thread uses this worktree."));
    await expect(editThreadFrom(fake.value, worktreeThread, 1, true, "x"))
      .rejects.toThrow("Another thread uses this worktree.");
    expect(fake.selected).toEqual([]);
    expect(fake.drafts).toEqual([]);
  });

  it("starts a fork or an edit in the parent's permission mode, read before the command", async () => {
    // 6 Oct 2026: both started an auto-mode thread's fork in the default mode.
    const edit = port({ key: "codex#new", prompt: "p" }, { [worktreeThread.key]: "auto" });
    await editThreadFrom(edit.value, worktreeThread, 1, false, "p");
    expect(edit.applied).toEqual([["codex#new", "auto"]]);
    const fork = port({ key: "codex#fork" }, { [worktreeThread.key]: "plan" });
    await forkThreadAt(fork.value, worktreeThread.key, 2);
    expect(fork.applied).toEqual([["codex#fork", "plan"]]);
    // The provider default needs nothing applied; nor does a refusal.
    for (const modes of [{}, { [worktreeThread.key]: "default" }]) {
      const plain = port({ key: "codex#fork" }, modes);
      await forkThreadAt(plain.value, worktreeThread.key, 2);
      expect(plain.applied).toEqual([]);
    }
    const refused = port(new Error("Grok cannot fork."), { [worktreeThread.key]: "auto" });
    await expect(forkThreadAt(refused.value, worktreeThread.key, 1)).rejects.toThrow();
    expect(refused.applied).toEqual([]);
  });

  it("forks at a turn and selects the new thread", async () => {
    const fake = port({ key: "codex#fork" });
    await expect(forkThreadAt(fake.value, worktreeThread.key, 2)).resolves.toBe("codex#fork");
    expect(fake.calls).toEqual([{ kind: "forkAt", key: worktreeThread.key, keepTurns: 2 }]);
    expect(fake.selected).toEqual(["codex#fork"]);
    await expect(forkThreadAt(port(new Error("Grok cannot fork.")).value, "k", 1))
      .rejects.toThrow("Grok cannot fork.");
  });
});

describe("thread pane rules", () => {
  it("offers Revert files too only to a thread that owns its worktree", () => {
    expect(conversationThread(worktreeThread).revertFilesBlocked).toBeNull();
    expect(conversationThread({ ...worktreeThread,
      workspace: { ...worktreeThread.workspace!, owned: false } }).revertFilesBlocked)
      .toBe(REVERT_SHARED_BLOCKED);
    expect(conversationThread(localThread).revertFilesBlocked).toBe(REVERT_LOCAL_BLOCKED);
    expect(conversationThread(localThread)).toMatchObject({ project: true, worktree: false });
  });

  it("keeps an unchanged thread's object across a new view", () => {
    const first = retainThreadIndex(new Map(), view);
    const again = retainThreadIndex(first, structuredClone(view));
    expect(again).toBe(first);
    const changed = structuredClone(view);
    changed.threads[0].unread = true;
    const next = retainThreadIndex(first, changed);
    expect(next).not.toBe(first);
    expect(next.get(view.threads[1].key)).toBe(first.get(view.threads[1].key));
  });

  it("starts the same workspace again: the worktree when it is there, a new one when it is gone", () => {
    expect(sameWorkspaceAs(worktreeThread).workspace.mode).toBe("existing");
    const gone: ThreadView = { ...worktreeThread, workspace: { ...worktreeThread.workspace!, missing: true } };
    expect(sameWorkspaceAs(gone)).toEqual({ project: "/Users/owner/Developer/Projects/atlas",
      workspace: { mode: "worktree", baseRef: "main" } });
    const noBase: ThreadView = { ...worktreeThread,
      workspace: { ...worktreeThread.workspace!, missing: true, baseRef: undefined } };
    expect(sameWorkspaceAs(noBase).workspace).toEqual({ mode: "worktree" });
    expect(sameWorkspaceAs(localThread).workspace).toEqual({ mode: "local" });
  });

  it("shows a diff panel hidden behind a subagent detail instead of closing it", () => {
    // Review, 6 Oct 2026: ⌘D closed a panel the owner could not see.
    expect(threadDiffToggleAction("codex#a", "codex#a", true)).toBe("show");
    expect(threadDiffToggleAction("codex#a", "codex#a", false)).toBe("close");
    expect(threadDiffToggleAction("codex#b", "codex#a", false)).toBe("open");
    expect(threadDiffToggleAction(null, "codex#a", true)).toBe("open");
  });

  it("names the recovery point a restore saved", () => {
    expect(threadRecoveryNotice({ restored: 3, recoveryRef: "refs/x" }, 0))
      .toBe("Restored 3 files to before message 1. Recovery point saved: refs/x");
  });

  it("hands a busy pane its own cached value", () => {
    const idle = conversationThread(worktreeThread);
    const busy = conversationThread(worktreeThread, true);
    expect(idle.busy).toBeUndefined();
    expect(busy.busy).toBe(true);
    expect(conversationThread(worktreeThread)).toBe(idle);
    expect(conversationThread(worktreeThread, true)).toBe(busy);
  });

  it("lets one fork or edit run per pane until it answers", async () => {
    const changes: string[][] = [];
    const lock = new ThreadPaneLock(held => { changes.push([...held]); });
    let finish: (value: string) => void = () => undefined;
    const first = lock.run("a", () => new Promise<string>(resolve => { finish = resolve; }));
    expect(first).not.toBeNull();
    expect(lock.run("a", () => Promise.resolve("again"))).toBeNull();
    // Another pane is not held up.
    await expect(lock.run("b", () => Promise.resolve("b"))).resolves.toBe("b");
    expect(lock.holds("a")).toBe(true);
    finish("done");
    await expect(first).resolves.toBe("done");
    expect(lock.holds("a")).toBe(false);
    // A refusal frees the pane too, and still rejects for its caller.
    await expect(lock.run("a", () => Promise.reject(new Error("refused")))).rejects.toThrow("refused");
    expect(lock.holds("a")).toBe(false);
    await expect(lock.run("a", () => { throw new Error("thrown"); })).rejects.toThrow("thrown");
    expect(changes.at(-1)).toEqual([]);
    expect(changes[0]).toEqual(["a"]);
  });

  it("re-reads the open diff when a turn ends", () => {
    const ended: ThreadView = { ...worktreeThread, lastRun: { startedAt: 1, endedAt: 2, outcome: "completed" } };
    expect(threadDiffSignal(ended)).not.toBe(threadDiffSignal(worktreeThread));
  });
});
