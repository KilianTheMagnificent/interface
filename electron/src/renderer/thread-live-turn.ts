import type { ConvItem } from "./conversation-reducer";
import type { DiffStat } from "./diff-stats";
import { editedFilesStat, pathFromToolInput } from "./tool-activity";
import {
  THREAD_THINKING, threadStepLine, threadStepWords, type ThreadStepState, type ThreadStepWords,
} from "./thread-live";
import { isCliWrittenPrompt } from "./thread-turns";
import type { TranscriptRow } from "./transcript-merge";

/**
 * A RUNNING THREADS TURN, AS IT DRAWS ITSELF (owner, 2026-10-07, "a live
 * turn"). Under the prompt: a header that says what the agent is doing now,
 * then a railed trail of everything it has done so far, with its prose
 * interleaved where it was said. This module decides all of it from the turn's
 * merged rows; ThreadLiveTurn.tsx only draws the answer.
 *
 * Every step's words come from thread-live.ts, the one vocabulary the pane and
 * the inbox share. Unknown is never zero: a step's `+N −M` exists only once its
 * call has a result and a diff counted its lines — never while it runs, which
 * is when Claude's mapper has already emitted a diff from the call's input.
 *
 * An approval carries no call id. The running call it is about is found by
 * tool and summary; with none, the approval is the waiting step itself. The
 * interactive card is never drawn here: the composer docks every active
 * approval, and the trail only says which step is waiting on the owner.
 */

/** One step of work on the trail. */
export interface ThreadTrailStep {
  kind: "step";
  /** The row the step stands at: its call, subagent, approval or file change. */
  key: string;
  /** Every row the step stands for (its diffs, a matched approval), for find. */
  keys: readonly string[];
  /** What its detail draws when the row is opened. */
  item: ConvItem;
  state: ThreadStepState;
  /** "Allowed" / "Denied" before an answered approval's words, else "". */
  prefix: string;
  words: ThreadStepWords;
  /** The row's title: the absolute path or the engine's own summary. */
  title: string;
  /** A short note after the words: "needs your OK", "checking", "Explore". */
  meta: string;
  /** Its lines, once the call has a result and a diff counted them; else null. */
  stat: DiffStat | null;
  /** A running edit's first lines, from its input; empty otherwise. */
  peek: readonly string[];
  /** The approval this step waits on, when it waits on one. */
  approvalId?: string;
}

/** A message the owner sent while the run was going (Steer, or one with no turn of its own). */
export interface ThreadTrailSteer {
  kind: "steer";
  key: string;
  /** The message's first line, bounded. */
  text: string;
}

export type ThreadTrailRow = ThreadTrailStep | ThreadTrailSteer;

/** The turn in transcript order: runs of trail rows, broken by the rows drawn as they are. */
export type ThreadLiveSegment =
  | { kind: "trail"; key: string; rows: readonly ThreadTrailRow[] }
  /** Prose, a notice, the seam: drawn by the transcript's own row. */
  | { kind: "row"; key: string; row: TranscriptRow };

export type ThreadLiveState = "thinking" | "working" | "waiting";

export interface ThreadLiveHeader {
  state: ThreadLiveState;
  /** The newest running step's words, or null while thinking or waiting. */
  words: ThreadStepWords | null;
  /** The whole line: "Editing src/a.ts", "Thinking", "Waiting for you". */
  line: string;
  /** The same for a narrow place (the live pill): a path's last segment. */
  short: string;
}

export interface ThreadLiveTurnModel {
  segments: readonly ThreadLiveSegment[];
  header: ThreadLiveHeader;
  /** Something after the prompt is on screen, so the thinking placeholder gives way. */
  started: boolean;
  /** The newest running step, for the finish's pop. */
  runningKey: string | null;
  /** The approval the waiting step is on, for its clock. */
  waitingId: string | null;
}

export interface ThreadLiveOptions {
  /** Approvals the owner can answer right now (live panes only). */
  waiting?: ReadonlySet<string>;
  /** Approvals the agent policy is still deciding. */
  checking?: ReadonlySet<string>;
  /** The verdict this window sent for an approval, if it sent one. */
  verdict?: (approvalId: string) => "allow" | "deny" | undefined;
  /** Row keys that are steers (threadTranscriptPlan's). */
  steers?: ReadonlySet<string>;
  root?: string;
  /** Draw only the rows this accepts (the finish collapses only what folds). */
  only?: (key: string) => boolean;
}

export const THREAD_WAITING_LINE = "Waiting for you";
/** The longest line a steer row prints; the transcript keeps the rest. */
export const THREAD_STEER_MAX = 160;
/** A running edit's peek: at most this many lines, each at most this long. */
export const THREAD_PEEK_LINES = 4;
export const THREAD_PEEK_CHARS = 160;

const NONE: ReadonlySet<string> = new Set();

type ToolItem = Extract<ConvItem, { kind: "tool" }>;
type ApprovalItem = Extract<ConvItem, { kind: "approval" }>;
type DiffItem = Extract<ConvItem, { kind: "diff" }>;

const toolKey = (tool: string): string => tool.trim().toLowerCase().replace(/[\s_-]+/gu, "");
const isDeclined = (result: ToolItem["result"]): boolean =>
  result !== undefined && !result.ok && result.summary.trim().toLowerCase() === "declined";

function cut(line: string, max: number): string {
  return line.length > max ? `${line.slice(0, max - 1).trimEnd()}…` : line;
}

/** Up to `count` non-blank lines of `text`, scanned without splitting all of it. */
function firstLines(text: string, count: number): string[] {
  const lines: string[] = [];
  let at = 0;
  while (at <= text.length && lines.length < count) {
    const end = text.indexOf("\n", at);
    const line = (end < 0 ? text.slice(at) : text.slice(at, end)).replace(/\r$/u, "");
    if (line.trim() !== "") lines.push(cut(line, THREAD_PEEK_CHARS));
    if (end < 0) break;
    at = end + 1;
  }
  return lines;
}

/** Past this much text an edit's peek reads its new text from the top. */
const PEEK_DIFF_CHARS = 400_000;

/**
 * The lines an edit adds: its new text without the lines it shares with the
 * old text at either end, so an append ("whole file" → "whole file + one
 * line") peeks at the one line, never the file it kept. A pure deletion adds
 * nothing and peeks at nothing.
 */
function addedLines(before: unknown, after: string, count: number): string[] {
  if (typeof before !== "string" || before.length + after.length > PEEK_DIFF_CHARS) {
    return firstLines(after, count);
  }
  const old = before.split("\n");
  const next = after.split("\n");
  let start = 0;
  while (start < old.length && start < next.length && old[start] === next[start]) start += 1;
  let oldEnd = old.length;
  let nextEnd = next.length;
  while (oldEnd > start && nextEnd > start && old[oldEnd - 1] === next[nextEnd - 1]) {
    oldEnd -= 1;
    nextEnd -= 1;
  }
  return firstLines(next.slice(start, nextEnd).join("\n"), count);
}

const PEEKS = new WeakMap<ConvItem, readonly string[]>();
const NO_PEEK: readonly string[] = [];

/**
 * PURE, memoized per item. What a running Edit, MultiEdit or Write is about to
 * put in the file, from its own input: Edit's new text, MultiEdit's new texts
 * in order, Write's opening lines. Bounded however large the input is.
 */
export function threadPeekLines(item: ToolItem): readonly string[] {
  const known = PEEKS.get(item);
  if (known !== undefined) return known;
  const input = typeof item.input === "object" && item.input !== null
    ? item.input as Record<string, unknown> : {};
  const name = toolKey(item.tool);
  let lines: readonly string[] = NO_PEEK;
  if (name === "edit" || name === "strreplace" || name === "editfile") {
    if (typeof input.new_string === "string") {
      lines = addedLines(input.old_string, input.new_string, THREAD_PEEK_LINES);
    }
  } else if (name === "multiedit" && Array.isArray(input.edits)) {
    const gathered: string[] = [];
    for (const edit of input.edits) {
      if (gathered.length >= THREAD_PEEK_LINES) break;
      const record = typeof edit === "object" && edit !== null ? edit as Record<string, unknown> : {};
      const next = record.new_string;
      if (typeof next === "string") {
        gathered.push(...addedLines(record.old_string, next, THREAD_PEEK_LINES - gathered.length));
      }
    }
    lines = gathered;
  } else if ((name === "write" || name === "writefile") && typeof input.content === "string") {
    lines = firstLines(input.content, THREAD_PEEK_LINES);
  }
  PEEKS.set(item, lines);
  return lines;
}

/** The running call an active approval is about: same tool and summary, the nearest one before it. */
function matchApprovals(rows: readonly TranscriptRow[], waiting: ReadonlySet<string>):
    { onTool: Map<string, ApprovalItem & { rowKey: string }>; hidden: Set<string> } {
  const onTool = new Map<string, ApprovalItem & { rowKey: string }>();
  const hidden = new Set<string>();
  if (waiting.size === 0) return { onTool, hidden };
  for (let index = 0; index < rows.length; index += 1) {
    const row = rows[index];
    if (row.kind !== "item" || row.item.kind !== "approval") continue;
    const approval = row.item;
    if (approval.resolved !== undefined || !waiting.has(approval.id)) continue;
    // Nearest first: the call is emitted just before the request it raises.
    for (let back = rows.length - 1; back >= 0; back -= 1) {
      const candidate = rows[back];
      if (candidate.kind !== "item" || candidate.item.kind !== "tool") continue;
      const call = candidate.item;
      if (call.result !== undefined || onTool.has(candidate.key)) continue;
      if (call.tool !== approval.tool || call.summary.trim() !== approval.summary.trim()) continue;
      onTool.set(candidate.key, { ...approval, rowKey: row.key });
      hidden.add(row.key);
      break;
    }
  }
  return { onTool, hidden };
}

function toolStep(row: TranscriptRow & { kind: "item" }, call: ToolItem, diffs: readonly { key: string;
    item: DiffItem }[], approval: (ApprovalItem & { rowKey: string }) | undefined,
    root?: string): ThreadTrailStep {
  const declined = isDeclined(call.result);
  const state: ThreadStepState = approval !== undefined ? "waiting"
    : call.result === undefined ? "running"
      : call.result.ok ? "done" : declined ? "done" : "failed";
  const words = threadStepWords(call, declined ? "waiting" : state, root);
  const path = pathFromToolInput(call.input);
  const keys = [row.key, ...diffs.map(diff => diff.key)];
  if (approval !== undefined) keys.push(approval.rowKey);
  return {
    kind: "step", key: row.key, keys, item: call, state, prefix: "", words,
    title: path ?? (call.summary.trim() !== "" ? call.summary : call.tool),
    meta: approval !== undefined ? (approval.questions !== undefined ? "needs your answer" : "needs your OK")
      : declined ? "declined" : state === "failed" ? "failed" : "",
    // Only a finished, fine call has lines to show: a running one's diff is
    // its input, and a failed or refused one changed nothing it can vouch for.
    stat: call.result?.ok === true ? editedFilesStat(diffs.map(diff => diff.item)) : null,
    peek: state === "running" ? threadPeekLines(call) : NO_PEEK,
    ...(approval === undefined ? {} : { approvalId: approval.id }),
  };
}

function approvalStep(key: string, approval: ApprovalItem, options: ThreadLiveOptions): ThreadTrailStep {
  const waiting = approval.resolved === undefined && (options.waiting ?? NONE).has(approval.id);
  const checking = !waiting && approval.resolved === undefined && (options.checking ?? NONE).has(approval.id);
  const verdict = approval.resolved ?? options.verdict?.(approval.id);
  const state: ThreadStepState = waiting ? "waiting" : checking ? "running" : "done";
  const question = approval.questions !== undefined;
  const prefix = waiting || checking || verdict === undefined ? ""
    : verdict === "allow" ? (question ? "Answered" : "Allowed") : (question ? "Declined" : "Denied");
  return {
    kind: "step", key, keys: [key], item: approval, state, prefix,
    words: threadStepWords(approval, "waiting", options.root),
    title: approval.summary.trim() !== "" ? approval.summary : approval.tool,
    meta: waiting ? (question ? "needs your answer" : "needs your OK") : checking ? "checking" : "",
    stat: null, peek: NO_PEEK,
    ...(waiting ? { approvalId: approval.id } : {}),
  };
}

function subagentStep(key: string, agent: Extract<ConvItem, { kind: "subagent" }>, root?: string): ThreadTrailStep {
  const state: ThreadStepState = agent.status === "running" ? "running"
    : agent.status === "failed" ? "failed" : "done";
  const description = agent.description.trim();
  return {
    kind: "step", key, keys: [key], item: agent, state, prefix: "",
    words: threadStepWords({ tool: "Task", summary: description, input: { description } }, state, root),
    title: description !== "" ? `${agent.subagentType} — ${description}` : agent.subagentType,
    meta: agent.status === "ended" ? `${agent.subagentType} · ended` : agent.subagentType,
    stat: null, peek: NO_PEEK,
  };
}

/** A file change whose call is not in this turn: a finished step of its own. */
function diffStep(key: string, diff: DiffItem, root?: string): ThreadTrailStep {
  return {
    kind: "step", key, keys: [key], item: diff, state: "done", prefix: "",
    words: threadStepWords({ tool: "Edit", summary: diff.path, input: { file_path: diff.path } }, "done", root),
    title: diff.path, meta: "", stat: editedFilesStat([diff]), peek: NO_PEEK,
  };
}

/** The steer's first line, bounded; the whole message stays in the transcript. */
function steerText(text: string): string {
  const line = text.split(/\r?\n/u).find(part => part.trim() !== "")?.trim() ?? "";
  return cut(line, THREAD_STEER_MAX);
}

/**
 * PURE. A running turn's header and trail, from the rows after its prompt.
 * O(rows) per call, plus one backward scan per active approval (there is
 * almost always none, and never more than a handful).
 */
export function threadLiveTurn(rows: readonly TranscriptRow[], options: ThreadLiveOptions = {}):
    ThreadLiveTurnModel {
  const waiting = options.waiting ?? NONE;
  const steers = options.steers ?? NONE;
  const only = options.only;
  const root = options.root;
  const calls = new Set<string>();
  const diffsByCall = new Map<string, { key: string; item: DiffItem }[]>();
  for (const row of rows) {
    if (row.kind !== "item") continue;
    if (row.item.kind === "tool") calls.add(row.item.id);
    else if (row.item.kind === "diff") {
      const list = diffsByCall.get(row.item.callId);
      if (list === undefined) diffsByCall.set(row.item.callId, [{ key: row.key, item: row.item }]);
      else list.push({ key: row.key, item: row.item });
    }
  }
  const { onTool, hidden } = matchApprovals(rows, waiting);
  const segments: ThreadLiveSegment[] = [];
  let trail: ThreadTrailRow[] | null = null;
  let started = false;
  let running: ThreadTrailStep | null = null;
  let waitingStep: ThreadTrailStep | null = null;
  const push = (entry: ThreadTrailRow): void => {
    started = true;
    if (entry.kind === "step") {
      if (entry.state === "running" && entry.item.kind !== "approval") running = entry;
      if (entry.state === "waiting" && waitingStep === null) waitingStep = entry;
    }
    if (trail === null) {
      trail = [];
      segments.push({ kind: "trail", key: `trail:${entry.key}`, rows: trail });
    }
    trail.push(entry);
  };
  const drawn = (row: TranscriptRow): void => {
    trail = null;
    segments.push({ kind: "row", key: row.key, row });
  };
  for (const row of rows) {
    if (only !== undefined && !only(row.key)) continue;
    if (row.kind === "mark") { drawn(row); continue; }
    const item = row.item;
    switch (item.kind) {
      case "text":
        // A block the stream opened and has not filled yet is not output.
        if (item.text.trim() === "") continue;
        started = true;
        drawn(row);
        break;
      case "user":
        if (steers.has(row.key) && !isCliWrittenPrompt(item.text)) {
          push({ kind: "steer", key: row.key, text: steerText(item.text) });
        } else drawn(row);
        break;
      case "tool":
        push(toolStep(row, item, diffsByCall.get(item.id) ?? [], onTool.get(row.key), root));
        break;
      case "diff":
        // Drawn with its call, whose row says it once the call has finished.
        if (calls.has(item.callId)) continue;
        push(diffStep(row.key, item, root));
        break;
      case "approval":
        if (hidden.has(row.key)) continue;
        push(approvalStep(row.key, item, options));
        break;
      case "subagent":
        push(subagentStep(row.key, item, root));
        break;
      default:
        started = true;
        drawn(row);
    }
  }
  const waitingFound = waitingStep as ThreadTrailStep | null;
  const runningFound = running as ThreadTrailStep | null;
  const header: ThreadLiveHeader = waitingFound !== null
    ? { state: "waiting", words: null, line: THREAD_WAITING_LINE, short: THREAD_WAITING_LINE }
    : runningFound !== null
      ? { state: "working", words: runningFound.words, line: threadStepLine(runningFound.words),
        short: threadStepLine(runningFound.words, true) }
      : { state: "thinking", words: null, line: THREAD_THINKING, short: THREAD_THINKING };
  return {
    segments, header, started,
    runningKey: runningFound?.key ?? null,
    waitingId: waitingFound?.approvalId ?? null,
  };
}

/** What a screen reader hears when the live turn changes state, and only then. */
export function threadLiveAnnouncement(state: ThreadLiveState | "done"): string {
  if (state === "waiting") return "Waiting for your approval";
  if (state === "done") return "Finished";
  return "Working";
}

/** A trail step's accessible name: its words, its note, its lines and its state. */
export function threadStepLabel(step: ThreadTrailStep): string {
  const words = threadStepLine(step.words);
  const parts = [step.prefix === "" ? words : `${step.prefix} ${words}`];
  if (step.meta !== "") parts.push(step.meta);
  if (step.stat !== null && (step.stat.added > 0 || step.stat.removed > 0)) {
    parts.push(`${step.stat.added} ${step.stat.added === 1 ? "line" : "lines"} added, ${step.stat.removed} removed`);
  }
  parts.push(step.state === "done" ? "done" : step.state);
  return parts.join(", ");
}
