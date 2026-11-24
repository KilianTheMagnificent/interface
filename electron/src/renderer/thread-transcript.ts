import type { ThreadRun, ThreadTurn } from "../shared/threads";
import { displayRowKeys, type ConversationDisplayRow } from "./conversation-display";
import type { ConvItem } from "./conversation-reducer";
import { formatDiffStat, type DiffStat } from "./diff-stats";
import { displayPath } from "./display-paths";
import { formatTurnElapsed } from "./session-activity";
import { isCliWrittenPrompt } from "./thread-turns";
import {
  appliedDiffs, editedFilesStat, fileChangeVerbs, pathFromToolInput, producedDiffs,
  toolActivityKind,
  type FileChangeVerb,
} from "./tool-activity";
import type { TranscriptRow } from "./transcript-merge";

/**
 * THE THREADS LAYOUT'S TURNS (owner, 2026-10-06 redesign). A Threads pane draws
 * each finished turn as the prompt, one work line standing for everything the
 * agent did on the way, the final answer, a card of the files it changed and,
 * once, the turn's controls. This module decides all of it from the merged
 * transcript rows; the transcript only draws the answer.
 *
 * A TURN is a genuine user row and every row after it up to the next genuine
 * user row. A row the provider's CLI wrote itself (a stop's marker, a `!` bash
 * echo) opens no turn, the same rule thread-turns.ts keeps for turn controls.
 * Nor does a STEER (owner, 2026-10-07): a message the owner sent into a run
 * already going. Rust records no turn for it (the run tap begins one per
 * send, never per steer), so treating it as one folded the run's first half
 * early and counted the run twice. See threadSteerKeys.
 *
 * WHAT FOLDS is everything between the prompt and the turn's final assistant
 * message, except what must never be hidden: an error, a failed or unfinished
 * call, a failed subagent, a failed compaction, an approval still waiting on
 * the owner, a row the CLI wrote, and the live seam. Those stay where they are, below the work
 * line. A turn with no final message folds all of its work. Whether a turn is
 * still running is the caller's to say: the live turn never folds.
 *
 * UNKNOWN IS NEVER ZERO. A count of nothing is left out of the work line, a
 * duration nobody measured is left out of it too, and a file whose lines no
 * diff counted prints no stat.
 */

/** What the folded work did, by kind of call. Zero means "none of these". */
export interface ThreadWorkCounts {
  read: number;
  search: number;
  list: number;
  edit: number;
  command: number;
  subagent: number;
  other: number;
  /** Messages the owner sent into the run (Steer), which opened no turn of their own. */
  steer: number;
}

/** One file a turn changed: the kind of change and its lines, where a diff counted them. */
export interface ThreadChangedFile {
  path: string;
  verb: FileChangeVerb;
  stat: DiffStat | null;
}

export interface ThreadTurnLayout {
  /** The genuine user row that opens the turn. */
  userKey: string;
  /** The turn's final assistant message, or null when it has none yet. */
  answerKey: string | null;
  /** That message's text, for Copy. "" when there is none. */
  answerText: string;
  /** The turn's last item row, or null when nothing came after the prompt. */
  endKey: string | null;
  /** Anything after the prompt other than a row the CLI wrote: a fork has an answer to keep. */
  replied: boolean;
  /** How many rows fold behind the work line. */
  folded: number;
  work: ThreadWorkCounts;
  /** Every file the turn's edits touched, in the order first touched. */
  files: readonly ThreadChangedFile[];
  /** Their lines summed, or null when no diff counted any. */
  stat: DiffStat | null;
}

export interface ThreadTranscriptPlan {
  turns: readonly ThreadTurnLayout[];
  /** User row keys that are steers: part of the turn they arrived in, never one of their own. */
  steers: ReadonlySet<string>;
  byUser: ReadonlyMap<string, ThreadTurnLayout>;
  /** Row key → the user key of the turn whose work line it folds behind. */
  foldedIn: ReadonlyMap<string, string>;
  /** Row keys that are a failure: never folded, and drawn with the failed tint. */
  failed: ReadonlySet<string>;
}

const NO_APPROVALS: ReadonlySet<string> = new Set();
const NO_STEERS: ReadonlySet<string> = new Set();

const isDeclined = (result: { ok: boolean; summary: string } | undefined): boolean =>
  result !== undefined && !result.ok && result.summary.trim().toLowerCase() === "declined";

/** A row whose whole point is that something went wrong. A refusal is the owner's decision, not one. */
export function isThreadFailure(item: ConvItem): boolean {
  switch (item.kind) {
    case "tool": return item.result?.ok === false && !isDeclined(item.result);
    case "notice": return item.tone === "error";
    case "compact": return item.ok === false;
    case "subagent": return item.status === "failed";
    default: return false;
  }
}

/** May this row fold behind a finished turn's work line? A steer does: it is part of the work. */
function foldable(item: ConvItem, waiting: ReadonlySet<string>, steer: boolean): boolean {
  switch (item.kind) {
    case "text": case "peer": case "diff": return true;
    // Finished and fine only: a failure is the reader's business, and a call
    // with no result in a finished turn is one nobody saw end.
    case "tool": return item.result?.ok === true;
    case "subagent": return item.status === "done" || item.status === "ended";
    case "approval": return !waiting.has(item.id);
    case "notice": return item.tone !== "error";
    case "compact": return item.ok !== false;
    case "user": return steer;
    // A page the agent showed stays above its reply, never behind the work line.
    case "visual": return false;
  }
}

const noWork = (): ThreadWorkCounts =>
  ({ read: 0, search: 0, list: 0, edit: 0, command: 0, subagent: 0, other: 0, steer: 0 });

/** A later change to a path wins only when it is a deletion or says more than the earlier one. */
function mergeVerb(previous: FileChangeVerb | undefined, next: FileChangeVerb): FileChangeVerb {
  if (previous === undefined || next === "delete") return next;
  return previous === "unknown" ? next : previous;
}

function layoutTurn(rows: readonly TranscriptRow[], start: number, end: number,
    waiting: ReadonlySet<string>, steers: ReadonlySet<string>, foldedIn: Map<string, string>): ThreadTurnLayout {
  const userKey = rows[start].key;
  let answer = -1;
  let endKey: string | null = null;
  let replied = false;
  for (let index = start + 1; index < end; index += 1) {
    const row = rows[index];
    if (row.kind !== "item") continue;
    endKey = row.key;
    if (row.item.kind === "text") answer = index;
    if (row.item.kind !== "user") replied = true;
  }
  const foldEnd = answer >= 0 ? answer : end;
  const counts = noWork();
  const readPaths = new Set<string>();
  const editPaths = new Set<string>();
  let readCalls = 0;
  let editCalls = 0;
  let folded = 0;
  for (let index = start + 1; index < foldEnd; index += 1) {
    const row = rows[index];
    // The seam stays drawn: where this window's live stream began is a fact
    // about the transcript, not part of the turn's work.
    if (row.kind === "mark") continue;
    const item = row.item;
    if (!foldable(item, waiting, steers.has(row.key))) continue;
    foldedIn.set(row.key, userKey);
    folded += 1;
    if (item.kind === "user") counts.steer += 1;
    else if (item.kind === "diff") editPaths.add(item.path);
    else if (item.kind === "subagent") counts.subagent += 1;
    else if (item.kind === "tool") {
      const kind = toolActivityKind(item.tool);
      const path = pathFromToolInput(item.input);
      if (kind === "read") {
        if (path === undefined) readCalls += 1;
        else readPaths.add(path);
      } else if (kind === "edit") {
        editCalls += 1;
        if (path !== undefined) editPaths.add(path);
      } else counts[kind] += 1;
    }
  }
  counts.read = readPaths.size + readCalls;
  counts.edit = editPaths.size > 0 ? editPaths.size : editCalls;
  const answerRow = answer >= 0 ? rows[answer] : undefined;
  const answerText = answerRow?.kind === "item" && answerRow.item.kind === "text"
    ? answerRow.item.text : "";
  return {
    userKey, answerKey: answerRow?.key ?? null, answerText, endKey, replied, folded,
    work: counts, ...changedFiles(rows, start, end),
  };
}

/**
 * A whole-file write. Its diff row carries only the new text — the provider
 * never says what the file held before — so its line counts are not the
 * file's change (a write that replaced eight lines with one reads +1 −0).
 */
const WHOLE_FILE_WRITE = "Write";

/** The ids of the whole-file writes among a subagent's children, at any depth. */
function wholeFileWrites(items: readonly ConvItem[], into = new Set<string>()): Set<string> {
  for (const item of items) {
    if (item.kind === "tool" && item.tool === WHOLE_FILE_WRITE) into.add(item.id);
    else if (item.kind === "subagent") wholeFileWrites(item.children, into);
  }
  return into;
}

/**
 * The files a turn's edits touched. A diff row says which file and how many
 * lines — except a whole-file write's, which names its file without a stat; a
 * refused call's diffs changed nothing and are skipped (`appliedDiffs`, the
 * rule the Chat layout's counts share); an edit call that
 * produced no diff row still names its file from its own input, without a
 * stat. A subagent's edits are the turn's own, as turnChangeSummaries holds.
 */
function changedFiles(rows: readonly TranscriptRow[], start: number, end: number):
    { files: ThreadChangedFile[]; stat: DiffStat | null } {
  const calls = new Map<string, Extract<ConvItem, { kind: "tool" }>>();
  const withDiffs = new Set<string>();
  const turn: ConvItem[] = [];
  for (let index = start + 1; index < end; index += 1) {
    const row = rows[index];
    if (row.kind !== "item") continue;
    turn.push(row.item);
    if (row.item.kind === "tool") calls.set(row.item.id, row.item);
    else if (row.item.kind === "diff") withDiffs.add(row.item.callId);
  }
  const applied = new Set(appliedDiffs(turn));
  const byPath = new Map<string, { verb: FileChangeVerb; diffs: { added?: number; removed?: number }[] }>();
  const all: { added?: number; removed?: number }[] = [];
  const touch = (path: string, verb: FileChangeVerb, diff?: { added?: number; removed?: number }): void => {
    const known = byPath.get(path);
    const entry = known ?? { verb, diffs: [] };
    entry.verb = mergeVerb(known?.verb, verb);
    if (diff !== undefined) {
      entry.diffs.push(diff);
      all.push(diff);
    }
    if (known === undefined) byPath.set(path, entry);
  };
  for (let index = start + 1; index < end; index += 1) {
    const row = rows[index];
    if (row.kind !== "item") continue;
    const item = row.item;
    if (item.kind === "diff") {
      if (!applied.has(item)) continue;
      const call = calls.get(item.callId);
      touch(item.path, call === undefined ? "unknown"
        : fileChangeVerbs(call, [item.path]).get(item.path) ?? "unknown",
        call?.tool === WHOLE_FILE_WRITE ? undefined : item);
    } else if (item.kind === "subagent") {
      const writes = wholeFileWrites(item.children);
      for (const diff of producedDiffs(item)) {
        if (!applied.has(diff)) continue;
        touch(diff.path, "unknown", writes.has(diff.callId) ? undefined : diff);
      }
    } else if (item.kind === "tool" && item.result?.ok === true && !withDiffs.has(item.id)
        && toolActivityKind(item.tool) === "edit") {
      const path = pathFromToolInput(item.input);
      if (path !== undefined) touch(path, fileChangeVerbs(item, [path]).get(path) ?? "unknown");
    }
  }
  return {
    files: [...byPath].map(([path, entry]) => ({ path, verb: entry.verb,
      stat: editedFilesStat(entry.diffs) })),
    stat: editedFilesStat(all),
  };
}

/** What the plan knows of Rust's turns, so a steer is not taken for a turn. */
export interface ThreadTurnMatch {
  /** User row key → the Rust turn it is (matchThreadTurns). */
  matched: ReadonlyMap<string, Pick<ThreadTurn, "ordinal" | "state">>;
  /** The newest ordinal Rust holds, or -1 when it holds none. */
  newest: number;
}

/**
 * PURE. Which user rows are steers: a genuine user row with no Rust turn of
 * its own that follows a matched turn, where no Rust turn is free for it.
 *
 * Rust records one turn per send and none per steer, so between matched turns
 * k and k+1 there is no ordinal an unmatched row could be: it arrived mid-run
 * (a Steer's echo, or a message the provider stored without a turn). Where an
 * ordinal IS free — turn k+1 exists but matched no row — the row may be that
 * turn, its prompt stored differently from what Interface recorded, so it
 * keeps opening a turn: a missing fold is the safe failure, a wrong one is
 * not. And the newest row, with nothing after it, under a turn Rust says has
 * finished, is a send Rust has not recorded yet rather than a steer: that
 * window lasts one round trip, and the next view matches it.
 *
 * With no matched row at all nothing is a steer, so a thread whose prompts
 * Rust never recorded draws exactly what it always drew.
 */
export function threadSteerKeys(rows: readonly TranscriptRow[], match: ThreadTurnMatch | undefined):
    ReadonlySet<string> {
  if (match === undefined || match.matched.size === 0) return NO_STEERS;
  const prompts: { key: string; index: number }[] = [];
  for (let index = 0; index < rows.length; index += 1) {
    const row = rows[index];
    if (row.kind === "item" && row.item.kind === "user" && !isCliWrittenPrompt(row.item.text)) {
      prompts.push({ key: row.key, index });
    }
  }
  // The ordinal of the next matched row below each prompt, if any.
  const nextOrdinal = new Array<number | undefined>(prompts.length);
  let next: number | undefined;
  for (let at = prompts.length - 1; at >= 0; at -= 1) {
    nextOrdinal[at] = next;
    const turn = match.matched.get(prompts[at].key);
    if (turn !== undefined) next = turn.ordinal;
  }
  const steers = new Set<string>();
  let open: Pick<ThreadTurn, "ordinal" | "state"> | undefined;
  prompts.forEach((prompt, at) => {
    const turn = match.matched.get(prompt.key);
    if (turn !== undefined) {
      open = turn;
      return;
    }
    if (open === undefined) return;
    const ceiling = nextOrdinal[at] ?? match.newest + 1;
    if (ceiling - open.ordinal > 1) {
      open = undefined;
      return;
    }
    const newest = at === prompts.length - 1;
    if (newest && open.state !== "running"
        && !rows.slice(prompt.index + 1).some(row => row.kind === "item")) {
      open = undefined;
      return;
    }
    steers.add(prompt.key);
  });
  return steers;
}

/**
 * PURE. Every turn of a Threads transcript and what folds in each. `waiting`
 * is the approvals the owner can still answer: they never fold, whichever turn
 * they are in. `match` is Rust's turns, which tell a steer from a turn (see
 * threadSteerKeys); without it every genuine user row opens a turn. Run once
 * per rows change, O(rows).
 */
export function threadTranscriptPlan(rows: readonly TranscriptRow[],
    waiting: ReadonlySet<string> = NO_APPROVALS, match?: ThreadTurnMatch): ThreadTranscriptPlan {
  const steers = threadSteerKeys(rows, match);
  const turns: ThreadTurnLayout[] = [];
  const foldedIn = new Map<string, string>();
  const failed = new Set<string>();
  let start = -1;
  for (let index = 0; index < rows.length; index += 1) {
    const row = rows[index];
    if (row.kind !== "item") continue;
    if (isThreadFailure(row.item)) failed.add(row.key);
    if (row.item.kind !== "user" || isCliWrittenPrompt(row.item.text) || steers.has(row.key)) continue;
    if (start >= 0) turns.push(layoutTurn(rows, start, index, waiting, steers, foldedIn));
    start = index;
  }
  if (start >= 0) turns.push(layoutTurn(rows, start, rows.length, waiting, steers, foldedIn));
  return { turns, steers, byUser: new Map(turns.map(turn => [turn.userKey, turn])), foldedIn, failed };
}

/** The key the transcript gives a display row's own element. */
export function threadDisplayKey(display: ConversationDisplayRow): string {
  return display.kind === "row" ? display.row.key : display.key;
}

export interface ThreadDisplayPlan {
  /** Display key → the turn whose work line it folds behind. */
  foldedBy: ReadonlyMap<string, string>;
  /** Display key → the turn whose changes card and controls hang under it. */
  endOf: ReadonlyMap<string, string>;
  /** Display keys drawn with the failed tint. */
  failed: ReadonlySet<string>;
}

/**
 * PURE. The plan, over what the pane draws. A display row folds when every row
 * it stands for folds in one turn — a run of finished calls always does, and a
 * merged file change whose call failed never does. A turn's end block hangs
 * under its final message, or under its last row when it has none.
 */
export function threadDisplayPlan(display: readonly ConversationDisplayRow[],
    plan: ThreadTranscriptPlan): ThreadDisplayPlan {
  const ends = new Map<string, string>();
  for (const turn of plan.turns) {
    const at = turn.answerKey ?? turn.endKey;
    if (at !== null) ends.set(at, turn.userKey);
  }
  const foldedBy = new Map<string, string>();
  const endOf = new Map<string, string>();
  const failed = new Set<string>();
  for (const entry of display) {
    const key = threadDisplayKey(entry);
    const keys = displayRowKeys(entry);
    const turn = plan.foldedIn.get(keys[0] ?? "");
    if (turn !== undefined && keys.every(candidate => plan.foldedIn.get(candidate) === turn)) {
      foldedBy.set(key, turn);
    }
    for (const candidate of keys) {
      if (plan.failed.has(candidate)) failed.add(key);
      const end = ends.get(candidate);
      if (end !== undefined) endOf.set(key, end);
    }
  }
  return { foldedBy, endOf, failed };
}

/**
 * PURE. What the pane draws, one entry per painted line, for the jump
 * capsule's count: a closed work line is one line however many rows it holds.
 */
export function threadDrawnKeys(display: readonly ConversationDisplayRow[],
    foldedBy: ReadonlyMap<string, string>, closed: (turnKey: string) => boolean): string[][] {
  const drawn: string[][] = [];
  const lines = new Map<string, string[]>();
  for (const entry of display) {
    const keys = displayRowKeys(entry);
    const turn = foldedBy.get(threadDisplayKey(entry));
    if (turn === undefined || !closed(turn)) {
      drawn.push(keys);
      continue;
    }
    const line = lines.get(turn);
    if (line !== undefined) line.push(...keys);
    else {
      const fresh = [...keys];
      lines.set(turn, fresh);
      drawn.push(fresh);
    }
  }
  return drawn;
}

/** "read 3 files, edited 2, ran 1 command" — each kind only where some was done. */
export function threadWorkSummary(work: ThreadWorkCounts): string {
  const parts: string[] = [];
  const plural = (count: number, one: string, many: string): string =>
    `${count} ${count === 1 ? one : many}`;
  if (work.read > 0) parts.push(`read ${plural(work.read, "file", "files")}`);
  if (work.search > 0) parts.push(work.search === 1 ? "searched once" : `searched ${work.search} times`);
  if (work.list > 0) parts.push(`listed ${plural(work.list, "folder", "folders")}`);
  if (work.edit > 0) parts.push(`edited ${plural(work.edit, "file", "files")}`);
  if (work.command > 0) parts.push(`ran ${plural(work.command, "command", "commands")}`);
  if (work.subagent > 0) parts.push(`used ${plural(work.subagent, "subagent", "subagents")}`);
  if (work.other > 0) parts.push(`called ${plural(work.other, "tool", "tools")}`);
  if (work.steer > 0) parts.push(work.steer === 1 ? "steered once" : `steered ${work.steer} times`);
  return parts.join(", ");
}

/** A kind of work the work line counts: one of ThreadWorkCounts' keys. */
export type ThreadWorkKind = keyof ThreadWorkCounts;

/** One chip on the work line: its kind (the chip's colour and glyph) and its short words. */
export interface ThreadWorkChip {
  kind: ThreadWorkKind;
  count: number;
  /** "read 3", "edited 2", "ran 1", "2 subagents", "steered 1". */
  label: string;
}

/**
 * PURE. The work line's chips (owner, 2026-10-06; one neutral tally each since
 * the Threads look, 2026-10-23): one per kind of work done, in
 * threadWorkSummary's order, and none for a kind with nothing — unknown is
 * never zero. A chip keeps the verb and the count; the full sentence stays
 * threadWorkSummary's, for the line's accessible name and title.
 */
export function threadWorkChips(work: ThreadWorkCounts): ThreadWorkChip[] {
  const plural = (count: number, one: string, many: string): string =>
    `${count} ${count === 1 ? one : many}`;
  const all: [ThreadWorkKind, string][] = [
    ["read", `read ${work.read}`],
    ["search", `searched ${work.search}`],
    ["list", `listed ${work.list}`],
    ["edit", `edited ${work.edit}`],
    ["command", `ran ${work.command}`],
    ["subagent", plural(work.subagent, "subagent", "subagents")],
    ["other", plural(work.other, "tool", "tools")],
    ["steer", `steered ${work.steer}`],
  ];
  return all.filter(([kind]) => work[kind] > 0)
    .map(([kind, label]) => ({ kind, count: work[kind], label }));
}

/** The work line's accessible name: "Worked for 41s · read 3 files, edited 2 files". */
export function threadWorkSentence(lead: string, work: ThreadWorkCounts): string {
  const detail = threadWorkSummary(work);
  return detail === "" ? lead : `${lead} · ${detail}`;
}

/** The work line's lead: "Worked for 41s", or "Worked" when nobody measured the turn. */
export function threadWorkLead(durationMs: number | undefined): string {
  if (durationMs === undefined) return "Worked";
  return durationMs < 1000 ? "Worked for <1s" : `Worked for ${formatTurnElapsed(durationMs)}`;
}

/**
 * How far apart Rust stamps a turn's send and its run's start. One send writes
 * both, but from two places a few milliseconds apart (47ms measured hands-free),
 * so exact equality never held. Ten seconds still tells this turn's run from
 * the previous one: the run tap stamps a start only for a send.
 */
export const THREAD_RUN_MATCH_MS = 10_000;

/**
 * The run Rust recorded for this turn: the thread's last run, and only for its
 * newest turn — Rust keeps one run, so an older turn has none.
 */
export function threadRunFor(turn: ThreadTurn | undefined,
    thread: { turns: readonly ThreadTurn[]; lastRun?: ThreadRun } | undefined): ThreadRun | undefined {
  if (turn === undefined || thread === undefined) return undefined;
  return thread.turns[thread.turns.length - 1] === turn ? thread.lastRun : undefined;
}

/** The turn's own run: the one that began at its `sentAt` (one `begin_turn` writes both). */
function runOfTurn(turn: Pick<ThreadTurn, "sentAt">, run: ThreadRun | undefined): ThreadRun | undefined {
  return run !== undefined && Math.abs(run.startedAt - turn.sentAt) <= THREAD_RUN_MATCH_MS ? run : undefined;
}

/**
 * How long a finished turn ran, from Rust's own stamps: while the thread's
 * last run is this turn's, from that run's start to its end; otherwise from
 * the turn's send to the end Rust kept on the turn itself (protocol 36), so
 * it survives the next send and a restart. Undefined where neither was
 * stamped — the next prompt's `sentAt` is when the owner typed again, not
 * when this turn stopped working.
 */
export function threadTurnDurationMs(turn: Pick<ThreadTurn, "sentAt" | "state" | "endedAt"> | undefined,
    run: ThreadRun | undefined): number | undefined {
  const ended = threadTurnEndedAt(turn, run);
  if (turn === undefined || ended === undefined) return undefined;
  const own = runOfTurn(turn, run);
  const started = own?.endedAt === undefined ? turn.sentAt : own.startedAt;
  return Math.max(0, ended - started);
}

/** When that turn ended, or undefined where its end was never stamped. */
export function threadTurnEndedAt(turn: Pick<ThreadTurn, "sentAt" | "state" | "endedAt"> | undefined,
    run: ThreadRun | undefined): number | undefined {
  if (turn === undefined || turn.state === "running") return undefined;
  return runOfTurn(turn, run)?.endedAt ?? turn.endedAt;
}

/** How long a finished turn ran and when it ended, once someone measured it. */
export interface ThreadTurnTiming { durationMs: number; endedAt: number }

/** How many turns' timings the window keeps; the oldest read goes first. */
export const THREAD_TIMING_LIMIT = 2_000;

/**
 * TIMINGS SURVIVE THE NEXT SEND (owner, 2026-10-07). Rust keeps one `lastRun`
 * per thread and replaces it when the next turn begins, so turn 1's "Worked
 * for 34s" and its end time vanished the moment turn 2 was sent. Since
 * protocol 36 Rust keeps each turn's own end (`ThreadTurn.endedAt`, owner
 * 2026-10-14), which survives a restart too; this window still remembers
 * what it saw, for turns recorded before that: Rust's own stamps for a turn
 * while they were that turn's, else this window's measure of a turn it
 * watched end, which Rust's later replace.
 *
 * Keyed by session, ordinal and send time, so a re-sent prompt or another
 * thread never reads a stranger's figure. Bounded and least-recently-read
 * first out, so a window open for weeks holds a fixed amount.
 */
const TIMINGS = new Map<string, ThreadTurnTiming & { rust: boolean }>();

const timingKey = (session: string, turn: Pick<ThreadTurn, "ordinal" | "sentAt">): string =>
  `${session}\u0000${String(turn.ordinal)}\u0000${String(turn.sentAt)}`;

/** Keep a turn's timing. Rust's stamps replace a window's measure; a window's never replace Rust's. */
export function rememberThreadTurnTiming(session: string, turn: Pick<ThreadTurn, "ordinal" | "sentAt">,
    timing: ThreadTurnTiming, source: "rust" | "window"): void {
  const key = timingKey(session, turn);
  const known = TIMINGS.get(key);
  if (known !== undefined && ((known.rust && source === "window")
      || (known.durationMs === timing.durationMs && known.endedAt === timing.endedAt))) return;
  TIMINGS.delete(key);
  TIMINGS.set(key, { durationMs: timing.durationMs, endedAt: timing.endedAt, rust: source === "rust" });
  if (TIMINGS.size > THREAD_TIMING_LIMIT) {
    const oldest = TIMINGS.keys().next().value;
    if (oldest !== undefined) TIMINGS.delete(oldest);
  }
}

/** A turn's remembered timing, or undefined where nobody measured it in this window. */
export function rememberedThreadTurnTiming(session: string,
    turn: Pick<ThreadTurn, "ordinal" | "sentAt">): ThreadTurnTiming | undefined {
  const key = timingKey(session, turn);
  const known = TIMINGS.get(key);
  if (known === undefined) return undefined;
  TIMINGS.delete(key);
  TIMINGS.set(key, known);
  return { durationMs: known.durationMs, endedAt: known.endedAt };
}

/** For tests: forget every remembered timing. */
export function forgetThreadTurnTimings(): void {
  TIMINGS.clear();
}

/**
 * A finished turn's duration and end: Rust's own stamps while they are this
 * turn's (remembered as they are read), else what this window remembered.
 * Nothing for a turn still running, and nothing anybody did not measure.
 */
export function threadTurnTiming(session: string | undefined, turn: ThreadTurn | undefined,
    thread: { turns: readonly ThreadTurn[]; lastRun?: ThreadRun } | undefined): Partial<ThreadTurnTiming> {
  if (turn === undefined || turn.state === "running") return {};
  const run = threadRunFor(turn, thread);
  const durationMs = threadTurnDurationMs(turn, run);
  const endedAt = threadTurnEndedAt(turn, run);
  if (durationMs !== undefined && endedAt !== undefined) {
    if (session !== undefined) rememberThreadTurnTiming(session, turn, { durationMs, endedAt }, "rust");
    return { durationMs, endedAt };
  }
  return session === undefined ? {} : rememberedThreadTurnTiming(session, turn) ?? {};
}

/** The status letter a changes card prints: what git would, or nothing it cannot say. */
export function threadFileLetter(verb: FileChangeVerb): string {
  return verb === "add" ? "A" : verb === "delete" ? "D" : verb === "update" ? "M" : "";
}

export interface ThreadChangesRow {
  path: string;
  /** Shortened against the pane's root. */
  shown: string;
  letter: string;
  verb: FileChangeVerb;
  stat: DiffStat | null;
}

export interface ThreadChangesView {
  files: number;
  stat: DiffStat | null;
  rows: readonly ThreadChangesRow[];
  /** Files counted but not listed. */
  more: number;
}

/** How many file rows a changes card lists before "+ N more". */
export const THREAD_CHANGES_ROWS = 6;

/**
 * PURE. A finished turn's changes card, or null when it changed nothing.
 *
 * The counts are Rust's measure of the turn when Rust has one, and otherwise
 * what the turn's own edit rows say. The file rows are always the edit rows':
 * Rust's record counts files but does not name them. "+ N more" counts what is
 * known to exist and is not listed, and is never drawn under an empty list.
 */
export function threadChangesView(turn: Pick<ThreadTurnLayout, "files" | "stat">,
    measured: Pick<ThreadTurn, "filesChanged" | "added" | "removed"> | undefined,
    root?: string): ThreadChangesView | null {
  const files = measured?.filesChanged ?? turn.files.length;
  if (files <= 0) return null;
  const stat = measured?.added !== undefined && measured.removed !== undefined
    ? { added: measured.added, removed: measured.removed } : turn.stat;
  // Per-file counts come from the edit rows, the header's from Rust's measure.
  // Where the rows name every file Rust counted yet their lines add up to a
  // different total (a whole-file write reports only its new lines), the rows
  // keep their paths and drop their counts rather than contradict the header.
  const rowsAgree = stat === turn.stat || stat === null || turn.files.length !== files
    || (turn.files.reduce((sum, file) => sum + (file.stat?.added ?? 0), 0) === stat.added
      && turn.files.reduce((sum, file) => sum + (file.stat?.removed ?? 0), 0) === stat.removed);
  const rows = turn.files.slice(0, THREAD_CHANGES_ROWS).map(file => ({
    path: file.path, shown: displayPath(file.path, root), letter: threadFileLetter(file.verb),
    verb: file.verb, stat: rowsAgree ? file.stat : null,
  }));
  return { files, stat, rows,
    more: rows.length === 0 ? 0 : Math.max(files, turn.files.length) - rows.length };
}

/** "3 files changed +41 −12", for a screen reader and the card's title. */
export function threadChangesLabel(view: Pick<ThreadChangesView, "files" | "stat">): string {
  const delta = formatDiffStat(view.stat);
  const head = `${view.files} ${view.files === 1 ? "file" : "files"} changed`;
  return delta === "" ? head : `${head} ${delta}`;
}

/** A clock time for a turn's stamp: "2:16 PM" in the reader's own locale. */
export function threadClockText(at: number): string {
  return new Date(at).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
}

/** A prompt is stamped again once this long has passed since the last stamp. */
export const THREAD_STAMP_GAP_MS = 60 * 60 * 1000;

/**
 * The prompts that carry a centred stamp above them (the TN Code look, owner
 * 2026-11-16): the first one Rust timed, then each sent an hour or more after
 * the last stamped one, so a quick exchange is not interrupted by the clock.
 * A prompt Rust recorded no time for is never stamped.
 */
export function threadStampKeys(turns: Iterable<readonly [string, { sentAt?: number }]>): Set<string> {
  const keys = new Set<string>();
  let last: number | null = null;
  for (const [key, turn] of turns) {
    const at = turn.sentAt;
    if (at === undefined || at <= 0) continue;
    if (last !== null && at - last < THREAD_STAMP_GAP_MS) continue;
    keys.add(key);
    last = at;
  }
  return keys;
}

/**
 * A stamp's two parts: the day in a word ("Today", "Yesterday", the weekday
 * within the last week, else a short date) and the clock time.
 */
export function threadStampParts(at: number, now: number): { day: string; time: string } {
  const startOfDay = (value: number): number => {
    const date = new Date(value);
    return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
  };
  const days = Math.round((startOfDay(now) - startOfDay(at)) / 86_400_000);
  const day = days === 0 ? "Today" : days === 1 ? "Yesterday"
    : days > 1 && days < 7 ? new Date(at).toLocaleDateString([], { weekday: "long" })
      : new Date(at).toLocaleDateString([], { month: "short", day: "numeric" });
  return { day, time: threadClockText(at) };
}
