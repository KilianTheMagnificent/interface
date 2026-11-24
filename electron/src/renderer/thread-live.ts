import { displayPath, lastSegment, shortenSummaryPaths } from "./display-paths";
import { pathFromToolInput, toolActivityKind, toolActivityName, toolSummaryRepeatsName }
  from "./tool-activity";

/**
 * The words a running Threads turn says about its work (owner, 2026-10-07,
 * "a live turn"): one verb in the state's tense and the thing it works on, so
 * the live header says "Editing src/routes/login.ts", the trail's finished row
 * says "Edited src/routes/login.ts", an approval says "Run npm test", and the
 * inbox row's second line says "Editing login.ts".
 *
 * One vocabulary for the pane and the inbox. The engine's own summary is the
 * last resort, never the first: Claude's is the file's ABSOLUTE path for Read,
 * Edit and Write and the bare tool name for Grep, which is what the old working
 * strip printed. Paths are shown project-relative (displayPath, Claude Code's
 * own rule); the absolute string stays on the row's title where a caller keeps
 * it. Pure and total: unknown input gives the tool's sentence-ready name.
 */

export type ThreadStepState = "running" | "done" | "failed" | "waiting";

export interface ThreadStepCall {
  tool: string;
  summary: string;
  input: unknown;
}

export interface ThreadStepWords {
  /** The verb in the state's tense: "Editing" running, "Edited" done, "Edit" waiting. */
  verb: string;
  /** What the step works on, project-relative, or "" when nothing names it. */
  target: string;
  /** For narrow places: a path's last segment; a command or pattern stays whole. */
  short: string;
  /** The target is code-like (a path, a command, a pattern) and is set in mono. */
  mono: boolean;
}

/** Running, done and waiting tenses for each kind of work. */
interface Tenses { running: string; done: string; waiting: string }
const READ: Tenses = { running: "Reading", done: "Read", waiting: "Read" };
const EDIT: Tenses = { running: "Editing", done: "Edited", waiting: "Edit" };
const WRITE: Tenses = { running: "Writing", done: "Wrote", waiting: "Write" };
const SEARCH: Tenses = { running: "Searching", done: "Searched", waiting: "Search" };
const LIST: Tenses = { running: "Listing", done: "Listed", waiting: "List" };
const RUN: Tenses = { running: "Running", done: "Ran", waiting: "Run" };
const FETCH: Tenses = { running: "Fetching", done: "Fetched", waiting: "Fetch" };
const WEB: Tenses = { running: "Searching the web", done: "Searched the web", waiting: "Search the web" };
const DELEGATE: Tenses = { running: "Delegating", done: "Delegated", waiting: "Delegate" };
const PLAN: Tenses = { running: "Updating the plan", done: "Updated the plan", waiting: "Update the plan" };

/** The longest target a live line prints; the row's title keeps the rest. */
export const THREAD_STEP_TARGET_MAX = 120;

const key = (tool: string): string => tool.trim().toLowerCase().replace(/[\s_-]+/gu, "");

function record(input: unknown): Record<string, unknown> {
  return typeof input === "object" && input !== null ? input as Record<string, unknown> : {};
}

function text(value: unknown): string {
  return typeof value === "string" ? value : "";
}

/** One line, at most THREAD_STEP_TARGET_MAX characters, ellipsis when cut. */
function oneLine(value: string): string {
  const line = value.split(/\r?\n/u).find(part => part.trim() !== "")?.trim() ?? "";
  return line.length > THREAD_STEP_TARGET_MAX
    ? `${line.slice(0, THREAD_STEP_TARGET_MAX - 1).trimEnd()}…` : line;
}

function tensesFor(tool: string): Tenses | null {
  const name = key(tool);
  if (name === "write" || name === "writefile") return WRITE;
  if (name === "webfetch" || name === "fetch") return FETCH;
  if (name === "websearch") return WEB;
  if (name === "task" || name === "agent") return DELEGATE;
  if (name === "todowrite" || name === "updateplan") return PLAN;
  switch (toolActivityKind(tool)) {
    case "read": return READ;
    case "edit": return EDIT;
    case "search": return SEARCH;
    case "list": return LIST;
    case "command": return RUN;
    default: return null;
  }
}

function tense(tenses: Tenses, state: ThreadStepState): string {
  if (state === "running") return tenses.running;
  if (state === "waiting") return tenses.waiting;
  return tenses.done;
}

/** A path target: shown relative to the root, short as its last segment. */
function pathWords(path: string, root?: string): Pick<ThreadStepWords, "target" | "short" | "mono"> {
  const shown = oneLine(displayPath(path, root));
  return { target: shown, short: lastSegment(shown) || shown, mono: true };
}

/** What the call works on, by the input keys each kind of tool uses. */
function targetFor(call: ThreadStepCall, tenses: Tenses | null, root?: string):
    Pick<ThreadStepWords, "target" | "short" | "mono"> {
  const input = record(call.input);
  const path = pathFromToolInput(call.input);
  if (tenses === RUN) {
    const command = oneLine(text(input.command) || text(input.cmd) || call.summary);
    const shown = shortenSummaryPaths(command, root);
    return { target: shown, short: shown, mono: shown !== "" };
  }
  if (tenses === SEARCH || tenses === WEB) {
    const pattern = oneLine(text(input.pattern) || text(input.query) || text(input.glob));
    if (pattern !== "") {
      const quoted = `"${pattern}"`;
      return { target: quoted, short: quoted, mono: true };
    }
  }
  if (tenses === FETCH) {
    const url = oneLine(text(input.url));
    if (url !== "") return { target: url, short: url, mono: true };
  }
  if (tenses === DELEGATE) {
    const task = oneLine(text(input.description) || call.summary);
    return { target: task, short: task, mono: false };
  }
  if (tenses === PLAN) return { target: "", short: "", mono: false };
  if (path !== undefined) return pathWords(path, root);
  const summary = call.summary.trim();
  if (summary === "" || toolSummaryRepeatsName(call.tool, summary)
      || key(summary) === key(toolActivityName(call.tool))) {
    return { target: "", short: "", mono: false };
  }
  const shown = oneLine(shortenSummaryPaths(summary, root));
  return { target: shown, short: shown, mono: /[/\\.]/u.test(shown) && !/\s/u.test(shown) };
}

/** The verb and target one tool call reads as, in the given state. */
export function threadStepWords(call: ThreadStepCall, state: ThreadStepState,
    root?: string): ThreadStepWords {
  const tenses = tensesFor(call.tool);
  const verb = tenses === null ? toolActivityName(call.tool) : tense(tenses, state);
  return { verb, ...targetFor(call, tenses, root) };
}

/** "Editing src/routes/login.ts", or "Editing login.ts" where `short` is asked for. */
export function threadStepLine(words: ThreadStepWords, short = false): string {
  const target = short ? words.short : words.target;
  return target === "" ? words.verb : `${words.verb} ${target}`;
}

/** The live line before any work is visible. */
export const THREAD_THINKING = "Thinking";
