import type { EngineEvent } from "../shared/engine";

/**
 * HOW MANY LINES AN AGENT HAS CHANGED, from what the renderer already holds.
 *
 * There is no per-session line count anywhere on the wire: `usage.updated`
 * carries tokens and nothing else, and the review pane's additions/deletions
 * come from `git --numstat` over an open folder, which is a fact about the
 * WORKING TREE and not about which agent wrote it. The one per-session source is
 * `diff.produced`, which every engine emits with the two sides of each change:
 * Claude sends the Edit's old_string/new_string (or "" and the file for a
 * Write), Codex rebuilds them from the hunks of its unified diff, Grok passes
 * the ACP diff block's oldText/newText.
 *
 * So the counting happens here. It is an honest measure of the edits this window
 * SAW — a session resumed from disk has no diff events until it edits something
 * again, and `null` is what says so. Never 0/0: a card that claims "+0 −0" over
 * an agent that rewrote a file is worse than a card that says nothing.
 */
export interface DiffStat {
  added: number;
  removed: number;
}

/**
 * Cost ceiling for the line-level LCS below. Beyond it the changed region is
 * reported as wholly replaced (every line of one side removed, every line of the
 * other added), which is the same thing a diff tool falls back to and is never
 * an undercount. 250k cells is a ~500x500-line changed region, well past
 * anything a single tool call produces.
 */
const MAX_LCS_CELLS = 250_000;

/**
 * Lines added and removed between two texts.
 *
 * Common head and tail lines are stripped first — which is the whole reason
 * this is cheap: a tool's before/after usually share almost everything, so the
 * quadratic step runs over the handful of lines that actually differ. What
 * remains goes through a longest-common-subsequence count, so an edit that
 * moves three lines inside a fifty-line context reports 3/3 and not 50/50.
 */
export function lineDelta(before: string, after: string): DiffStat {
  if (before === after) return { added: 0, removed: 0 };
  const a = splitLines(before);
  const b = splitLines(after);
  let head = 0;
  while (head < a.length && head < b.length && a[head] === b[head]) head += 1;
  let tail = 0;
  while (tail < a.length - head && tail < b.length - head
    && a[a.length - 1 - tail] === b[b.length - 1 - tail]) tail += 1;
  const left = a.slice(head, a.length - tail);
  const right = b.slice(head, b.length - tail);
  if (left.length === 0 || right.length === 0
    || left.length * right.length > MAX_LCS_CELLS) {
    return { added: right.length, removed: left.length };
  }
  const common = commonSubsequence(left, right);
  return { added: right.length - common, removed: left.length - common };
}

export type DiffProducedEvent = Extract<EngineEvent, { kind: "diff.produced" }>;

const EVENT_LINE_DELTA = new WeakMap<object, DiffStat>();

/** Line delta for one `diff.produced` event, cached on the event object. */
export function eventLineDelta(event: DiffProducedEvent): DiffStat {
  const hit = EVENT_LINE_DELTA.get(event);
  if (hit !== undefined) return hit;
  const delta = lineDelta(event.before, event.after);
  EVENT_LINE_DELTA.set(event, delta);
  return delta;
}

/**
 * The event without its file bodies, its line delta already cached on it.
 * The live log keeps this: nothing reads a diff's bodies but its line count,
 * and the bodies (often whole files, and for a Write the tool call's own input
 * a second time) filled the session's display budget and forced compaction.
 */
export function slimDiffEvent(event: DiffProducedEvent): DiffProducedEvent {
  if (event.before === "" && event.after === "") return event;
  const slim = { ...event, before: "", after: "" };
  EVENT_LINE_DELTA.set(slim, eventLineDelta(event));
  return slim;
}

/**
 * The log's diffs that changed something, in log order, from one pass over it.
 *
 * A diff is announced with its call, before the call runs or is approved, so a
 * diff whose call has a `tool.result` with `ok: false` in this log (declined or
 * failed) changed nothing and is left out. A call with no result yet keeps its
 * diffs. The same rule over transcript items is tool-activity.ts's
 * `appliedDiffs`.
 */
export function appliedDiffEvents(events: readonly EngineEvent[]): DiffProducedEvent[] {
  const diffs: DiffProducedEvent[] = [];
  let failed: Set<string> | undefined;
  for (const event of events) {
    if (event.kind === "diff.produced") diffs.push(event);
    else if (event.kind === "tool.result" && !event.ok) (failed ??= new Set()).add(event.callId);
  }
  return failed === undefined ? diffs : diffs.filter(diff => !failed.has(diff.callId));
}

/**
 * Every applied diff this session produced, summed — or null when it produced
 * none, which is "unknown" and must render as nothing at all.
 */
export function sessionDiffStat(events: readonly EngineEvent[]): DiffStat | null {
  let added = 0;
  let removed = 0;
  let saw = false;
  for (const event of appliedDiffEvents(events)) {
    saw = true;
    const delta = eventLineDelta(event);
    added += delta.added;
    removed += delta.removed;
  }
  return saw ? { added, removed } : null;
}

/**
 * "+12 −4", or "" when there is nothing truthful to say. U+2212 MINUS, matching
 * the review pane's per-file counts rather than a hyphen.
 *
 * A net-zero stat prints nothing on purpose: the only way to reach it is a diff
 * whose two sides are identical, and "+0 −0" reads as a measurement of the
 * session rather than of that one no-op.
 */
export function formatDiffStat(stat: DiffStat | null | undefined): string {
  if (stat === null || stat === undefined) return "";
  if (stat.added === 0 && stat.removed === 0) return "";
  return `+${stat.added} −${stat.removed}`;
}

/** The same fact spelled for a screen reader, or "" when there is none. */
export function diffStatLabel(stat: DiffStat | null | undefined): string {
  if (formatDiffStat(stat) === "" || stat === null || stat === undefined) return "";
  return `${stat.added} ${stat.added === 1 ? "line" : "lines"} added, `
    + `${stat.removed} removed`;
}

/** A session's summed stat and its distinct paths, in one pass over the log. */
export interface SessionDiffFacts {
  readonly stat: DiffStat | null;
  readonly paths: readonly string[];
}

/** `sessionDiffStat` and `sessionDiffPaths` together, for a caller that wants both per frame. */
export function sessionDiffFacts(events: readonly EngineEvent[]): SessionDiffFacts {
  let added = 0;
  let removed = 0;
  let saw = false;
  const seen = new Set<string>();
  const paths: string[] = [];
  for (const event of appliedDiffEvents(events)) {
    saw = true;
    const delta = eventLineDelta(event);
    added += delta.added;
    removed += delta.removed;
    if (seen.has(event.path)) continue;
    seen.add(event.path);
    paths.push(event.path);
  }
  return { stat: saw ? { added, removed } : null, paths };
}

/** Distinct paths of the applied `diff.produced` events this window saw. */
export function sessionDiffPaths(events: readonly EngineEvent[]): string[] {
  const seen = new Set<string>();
  const paths: string[] = [];
  for (const event of appliedDiffEvents(events)) {
    if (seen.has(event.path)) continue;
    seen.add(event.path);
    paths.push(event.path);
  }
  return paths;
}

/**
 * Composer dirty pill: `N files changed +X −Y`. Empty when the stat is
 * unknown or a net-zero — the same silence `formatDiffStat` already keeps.
 */
export function dirtyFilesLabel(
  fileCount: number, stat?: DiffStat | null,
): string {
  const delta = formatDiffStat(stat);
  if (delta === "") return "";
  const noun = fileCount === 1 ? "file" : "files";
  return `${fileCount} ${noun} changed ${delta}`;
}

// A trailing newline terminates the last line; it does not begin an empty one.
// "" is zero lines, not one — a Write's empty `before` must not count as a
// removed line.
function splitLines(text: string): string[] {
  if (text === "") return [];
  const split = text.split("\n");
  if (split.length > 1 && split[split.length - 1] === "") split.pop();
  return split;
}

// Two rolling rows rather than the full table: only the length is wanted, and
// the changed region can still be a few hundred lines wide.
function commonSubsequence(a: readonly string[], b: readonly string[]): number {
  let previous = new Array<number>(b.length + 1).fill(0);
  let row = new Array<number>(b.length + 1).fill(0);
  for (let i = 1; i <= a.length; i += 1) {
    row[0] = 0;
    for (let j = 1; j <= b.length; j += 1) {
      row[j] = a[i - 1] === b[j - 1]
        ? previous[j - 1] + 1
        : Math.max(previous[j], row[j - 1]);
    }
    const swap = previous;
    previous = row;
    row = swap;
  }
  return previous[b.length];
}
