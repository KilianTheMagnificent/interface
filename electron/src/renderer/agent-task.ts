import type { EngineEvent, PlanEntry, SessionStatus } from "../shared/engine";
import { isEngineId } from "./transcript-merge";
import type { ConvItem } from "./conversation-reducer";
import { appliedDiffEvents, eventLineDelta, type DiffStat } from "./diff-stats";
import { appliedDiffs } from "./tool-activity";

export type AgentStepState = "running" | "ok" | "failed" | "ended";

/** One folded line of an agent task's activity. */
export interface AgentStep {
  key: string;
  tool: string;
  /** "Read 3 files" once folded; otherwise the call's own summary. */
  label: string;
  count: number;
  state: AgentStepState;
}

export interface AgentProgress {
  plan: readonly PlanEntry[] | undefined;
  steps: AgentStep[];
  /** Distinct touched paths, newest first. */
  files: string[];
  /**
   * Lines added and removed per path, as the ITEMS themselves reported them —
   * the same arithmetic touchedFileStats does over raw events, carried through
   * the reducer. It is what lets a restarted session print +N −M for a file it
   * touched before this window opened, when the caller's own `diffStats` (built
   * from the live log alone) knows nothing about it. A path nobody measured is
   * absent, never 0/0.
   */
  fileStats: Readonly<Record<string, DiffStat>>;
}

// Verb and plural noun for a folded run of one tool. Unknown tools fold as
// "<Tool> ×N".
const FOLD_WORDS: Record<string, [verb: string, plural: string]> = {
  Read: ["Read", "files"], Edit: ["Edited", "files"], MultiEdit: ["Edited", "files"],
  Write: ["Wrote", "files"], Bash: ["Ran", "commands"], Grep: ["Searched", "patterns"],
  Glob: ["Searched", "patterns"], WebFetch: ["Fetched", "pages"], WebSearch: ["Searched", "queries"],
};

function foldedLabel(tool: string, count: number): string {
  const words = FOLD_WORDS[tool];
  return words === undefined ? `${tool} ×${count}` : `${words[0]} ${count} ${words[1]}`;
}

const SEVERITY: Record<AgentStepState, number> = { ok: 0, ended: 1, running: 2, failed: 3 };
function worse(a: AgentStepState, b: AgentStepState): AgentStepState {
  return SEVERITY[b] > SEVERITY[a] ? b : a;
}

// Distinct diffed paths, most recently touched first, with what each one gained
// and lost. Deliberately not `sessionDiffPaths`: that one dedupes on a path's
// FIRST sighting, so a file edited early and again just now would sort as if it
// were only touched early. What the panel promises is recency, so the LAST
// sighting is the one that orders the row.
//
// A subagent's own diffs count, because they were the raw `diff.produced`
// events this fold used to read and that filter was never by agentKey: the task
// touched those files whoever typed the edit. Its diffs sit inside its card, so
// they are taken depth-first, at the card's own position, which is where the
// spawn stood in the log. A declined or failed edit touched nothing
// (`appliedDiffs`), the same rule touchedFileStats keeps over raw events.
function touchedItemFiles(items: readonly ConvItem[]): {
  files: string[];
  fileStats: Record<string, DiffStat>;
} {
  const latest = new Map<string, number>();
  const fileStats: Record<string, DiffStat> = {};
  let seen = 0;
  for (const item of appliedDiffs(items)) {
    latest.set(item.path, seen);
    seen += 1;
    // Optional on the item, so an item built without them leaves the path
    // unmeasured rather than counting it as a zero-line change.
    if (item.added === undefined && item.removed === undefined) continue;
    const prior = fileStats[item.path] ?? { added: 0, removed: 0 };
    fileStats[item.path] = { added: prior.added + (item.added ?? 0),
      removed: prior.removed + (item.removed ?? 0) };
  }
  return {
    files: [...latest.entries()].sort((a, b) => b[1] - a[1]).map(([path]) => path),
    fileStats,
  };
}

/**
 * Lines added and removed per touched path, summed exactly the way
 * `sessionDiffStat` sums a whole session (applied diffs only) — so a task's
 * per-file figures and the tile's one-line total can never disagree. A path
 * with no diff at all is simply absent; `formatDiffStat` is what keeps a
 * net-zero quiet.
 */
export function touchedFileStats(
  events: readonly EngineEvent[],
): Record<string, DiffStat> {
  const out: Record<string, DiffStat> = {};
  for (const event of appliedDiffEvents(events)) {
    const delta = eventLineDelta(event);
    const prior = out[event.path];
    out[event.path] = prior === undefined ? delta
      : { added: prior.added + delta.added, removed: prior.removed + delta.removed };
  }
  return out;
}

/**
 * The task's transcript, past then live — the same two lists, in the same
 * order, that the conversation under the panel is merged from: the stored
 * history minus every engine-minted row the live log also carries, then the
 * live items.
 */
export function agentTaskItems(
  past: readonly ConvItem[], live: readonly ConvItem[],
): readonly ConvItem[] {
  if (past.length === 0) return live;
  const liveIds = new Set(live.filter(isEngineId).map(item => item.id));
  return [...past.filter(item => !(isEngineId(item) && liveIds.has(item.id))), ...live];
}

/**
 * The Agent surface's Progress panel, derived from the REDUCED transcript. Pure.
 *
 * ConvItems and not events, because a session that is not live has no events:
 * its past arrives as a HistoryView full of items (Conversation.tsx's history
 * prop contract), and a panel that read only the live log said "No activity
 * yet." over a finished task every time the app restarted. Items are what both
 * halves have in common — the live stream is reduced to exactly these — so
 * Progress and the transcript underneath it now describe the same work.
 */
export function agentProgressFromItems(
  items: readonly ConvItem[],
  plan: readonly PlanEntry[] | undefined,
  status: SessionStatus,
): AgentProgress {
  const live = status === "working" || status === "waiting";
  const steps: AgentStep[] = [];
  for (const item of items) {
    if (item.kind === "subagent") {
      // One row for the whole card, named by what it was asked to do — the
      // spawn's own summary is "Task" and says nothing. Its children are its
      // work, not the parent's, so the walk does not descend: that is the same
      // thing the raw fold did by skipping every call carrying an agentKey.
      // A card still "running" on a session that is not gets `ended`, which
      // claims no outcome — the reading settleDetachedSubagents gives the very
      // same card in the transcript below.
      steps.push({ key: item.id, tool: "Agent", label: item.description, count: 1,
        state: item.status === "done" ? "ok"
          : item.status === "failed" ? "failed"
          : item.status === "running" && live ? "running" : "ended" });
      continue;
    }
    if (item.kind !== "tool") continue;
    const state: AgentStepState = item.result === undefined ? (live ? "running" : "ended")
      : item.result.ok ? "ok" : "failed";
    const last = steps[steps.length - 1];
    if (last !== undefined && last.tool === item.tool && last.tool !== "Agent") {
      last.count += 1;
      last.label = foldedLabel(item.tool, last.count);
      last.state = worse(last.state, state);
      continue;
    }
    steps.push({ key: item.id, tool: item.tool, label: item.summary, count: 1, state });
  }
  return { plan, steps, ...touchedItemFiles(items) };
}
