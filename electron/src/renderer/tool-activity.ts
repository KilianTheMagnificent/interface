import { diffStatLabel, formatDiffStat } from "./diff-stats";
import type { DiffStat } from "./diff-stats";
import {
  absolutePathsIn, displayPath, isSeparator, lastSegment, shortenSummaryPaths,
} from "./display-paths";
import type { ConvItem, SubagentItem, SubagentStatus } from "./conversation-reducer";
import type { TranscriptRow } from "./transcript-merge";

/**
 * What a tool, file-change or work row prints: tool naming and grouping,
 * file-change verbs, per-turn change summaries, activity kinds, work-group
 * labels and a tool call's input blocks.
 */

/**
 * One spelling of an identifier. Case, camelCase, `_`, `-`, `/` and `\` are all
 * ways different engines spell the SAME name — Codex returns "fileChange" as a
 * tool and "file change" as that tool's summary, and an MCP call returns
 * "node_repl/js" as both.
 */
function identifierKey(value: string): string {
  return value
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .toLowerCase()
    .replace(/[\s_\-/\\]+/g, " ")
    .trim();
}

/**
 * The largest number of distinct tool names one of the caches below holds.
 * Every tool row classifies its name several times per render, over a session
 * vocabulary of a dozen or so names, so each pure name function remembers its
 * answers. MCP names are open-ended, so past the cap a cache starts over
 * rather than growing without limit. Exported for its test.
 */
export const TOOL_NAME_CACHE_LIMIT = 512;

function byToolName(cache: Map<string, string>, tool: string,
    compute: (tool: string) => string): string {
  const known = cache.get(tool);
  if (known !== undefined) return known;
  const value = compute(tool);
  if (cache.size >= TOOL_NAME_CACHE_LIMIT) cache.clear();
  cache.set(tool, value);
  return value;
}

const TOOL_KEYS = new Map<string, string>();
/** identifierKey with the spaces removed: the key the activity Sets are spelled in. */
function normalizedToolKey(tool: string): string {
  return byToolName(TOOL_KEYS, tool, name => identifierKey(name).replace(/ /gu, ""));
}

function mcpActivityParts(tool: string): { server: string; method: string } | null {
  const text = tool.trim();
  const slash = text.indexOf("/");
  if (slash > 0 && slash < text.length - 1) {
    return { server: text.slice(0, slash), method: text.slice(slash + 1) };
  }
  if (text.toLowerCase().startsWith("mcp__")) {
    const parts = text.slice(5).split("__").filter(Boolean);
    if (parts.length >= 2) {
      return { server: parts[0], method: parts.slice(1).join("__") };
    }
  }
  return null;
}

/** One stretch of adjacent rows and whether it may be drawn as a single line. */
export interface FinishedToolRun<T> { grouped: boolean; members: T[] }

/**
 * PURE. Work that is over and went well — the one rule a fold is allowed to
 * speak for, in one place so no caller has to restate it.
 *
 * A finished, successful tool call, and a DIFF. A diff is the record of an edit
 * that already happened: it carries no result of its own because the call
 * beside it carries the verdict for both, and excluding it would split every
 * edit away from the change it produced.
 */
export function isFinishedWork(item: ConvItem): boolean {
  return item.kind === "diff"
    || (item.kind === "tool" && item.result?.ok === true);
}

/**
 * PURE. Partition rows into stretches that may collapse into one summary line
 * and stretches that may not.
 *
 * ONE RULE, and it is isFinishedWork above. Running is excluded because
 * in-flight is the fact that outranks the summary — a fold would replace "this
 * is happening now" with a past-tense sentence. Failed is excluded because a
 * fold is a claim that nothing here needs your attention, and a failure is the
 * definition of something that does.
 *
 * A one-row stretch is reported ungrouped, but that is a statement about the
 * STRETCH and not about the row inside it: whether a run of one folds is the
 * density dial's call and always has been, and a caller that needs to tell a
 * lone success from a call that must never fold asks isFinishedWork.
 *
 * A bad call does NOT poison the rows around it — the stretch splits at it, so a
 * turn with one failure in the middle still folds the ten reads either side.
 */
export function groupFinishedTools<T extends { item: ConvItem }>(
  rows: readonly T[],
): FinishedToolRun<T>[] {
  const foldable = (row: T): boolean => isFinishedWork(row.item);
  const out: FinishedToolRun<T>[] = [];
  for (let index = 0; index < rows.length;) {
    if (!foldable(rows[index])) {
      out.push({ grouped: false, members: [rows[index]] });
      index += 1;
      continue;
    }
    const members: T[] = [];
    while (index < rows.length && foldable(rows[index])) {
      members.push(rows[index]);
      index += 1;
    }
    if (members.length === 1) out.push({ grouped: false, members });
    else out.push({ grouped: true, members });
  }
  return out;
}

const TOOL_GROUP_KEYS = new Map<string, string>();
/**
 * The provider-neutral identity used to fold repeated tool activity.
 *
 * Claude groups MCP activity by server rather than exposing every method as a
 * separate top-level object. Codex spells those calls `server/method`; some
 * providers use the older `mcp__server__method` form. Both collapse under the
 * server while ordinary tools retain their exact normalized identity.
 */
export function toolActivityGroupKey(tool: string): string {
  return byToolName(TOOL_GROUP_KEYS, tool, name => {
    const mcp = mcpActivityParts(name);
    if (mcp !== null) return `mcp:${identifierKey(mcp.server)}`;
    const text = name.trim();
    return `tool:${identifierKey(text)}`;
  });
}

const TOOL_WORDS = new Map<string, string>([
  ["api", "API"], ["css", "CSS"], ["html", "HTML"], ["http", "HTTP"],
  ["js", "JavaScript"], ["json", "JSON"], ["mcp", "MCP"],
  ["repl", "REPL"], ["sql", "SQL"], ["ts", "TypeScript"], ["url", "URL"],
]);

function activityDisplayName(stem: string): string {
  const words = stem
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .split(/[\s_-]+/u)
    .filter(Boolean);
  if (words.length === 0) return "Tool";
  return words.map((word, index) => {
    const replacement = TOOL_WORDS.get(word.toLowerCase());
    if (replacement !== undefined) return replacement;
    const lower = word.toLowerCase();
    return index === 0 ? lower.charAt(0).toUpperCase() + lower.slice(1) : lower;
  }).join(" ");
}

const TOOL_NAMES = new Map<string, string>();
/** A sentence-ready method name for one expanded/individual tool call. */
export function toolActivityName(tool: string): string {
  return byToolName(TOOL_NAMES, tool, name => {
    const mcp = mcpActivityParts(name);
    return activityDisplayName(mcp?.method ?? name.trim());
  });
}

const TOOL_GROUP_NAMES = new Map<string, string>();
/** A sentence-ready server/tool name for a collapsed activity group. */
export function toolActivityGroupName(tool: string): string {
  return byToolName(TOOL_GROUP_NAMES, tool, name => {
    const mcp = mcpActivityParts(name);
    return activityDisplayName(mcp?.server ?? name.trim());
  });
}

/**
 * Whether a summary is the tool's own name again.
 *
 * WHOLE-STRING equality after normalising, never a prefix or substring test.
 * That is the deliberate line: "Bash" + "bash -lc …" starts with the tool name
 * and is the most informative field on the row, so a prefix rule would delete
 * real text to fix a cosmetic repeat. Only a summary that says nothing the name
 * did not already say is dropped.
 */
export function toolSummaryRepeatsName(tool: string, summary: string): boolean {
  const text = summary.trim();
  if (text === "") return true;
  return identifierKey(text) === identifierKey(tool);
}

/**
 * Whether a summary is just the list of files its own diff rows name. True only
 * when the summary carries at least two absolute paths AND one of them is a
 * path a following diff row already draws — the engine truncates these lists
 * mid-word at 120 chars (mapper.ts), so the LAST path may be a fragment while
 * the earlier ones are whole, and "one of them matches" is the test that
 * survives that.
 */
export function summaryRelistsDiffPaths(
  summary: string, diffPaths: readonly string[],
): boolean {
  if (diffPaths.length < 2) return false;
  const mentioned = absolutePathsIn(summary);
  if (mentioned.length < 2) return false;
  return mentioned.some(path => diffPaths.includes(path));
}

/**
 * THE TEXT A TOOL ROW'S SUMMARY SLOT PRINTS. "" means print no slot at all.
 *
 * Three rules, in order: a summary that repeats the tool name is dropped; a
 * summary that re-lists the files its own diff rows name below it collapses to
 * a count (the paths are on those rows, and the header is the one place a count
 * cannot be read off the screen); everything else keeps its text with absolute
 * paths shortened. The raw string is untouched in the row's title and in the
 * find corpus either way.
 */
export function toolRowSummaryText(
  item: { tool: string; summary: string },
  options: { root?: string; diffPaths?: readonly string[] } = {},
): string {
  const summary = item.summary.trim();
  if (toolSummaryRepeatsName(item.tool, summary)) return "";
  const diffPaths = options.diffPaths ?? [];
  if (summaryRelistsDiffPaths(summary, diffPaths)) {
    const distinct = new Set(diffPaths).size;
    if (distinct >= 2) return `${distinct} files`;
  }
  return shortenSummaryPaths(summary, options.root);
}

/**
 * WHICH KIND OF CHANGE EACH PATH TOOK — add, delete or update — read from the
 * one place that fact exists.
 *
 * A `diff.produced` event carries no kind; it lives exclusively inside the tool
 * row's summary, which Codex's changeList (crates/engine-codex/src/mapper.rs)
 * writes as "<kind> <path>[, <kind> <path>…]" and then clips at 120 characters
 * through clip_label, which marks the cut with a trailing ellipsis. So the LAST
 * entry may be a fragment, and that fragment may carry the marker — which is why
 * the prefix match below strips it. It used not to: while the marker was there and
 * the strip was not, `startsWith` matched nothing and every clipped patch fell
 * through to "unknown", which is exactly the failure this function exists to
 * prevent.
 *
 * A fragment is resolved ONLY when it can be resolved without guessing: its
 * kind word is intact, and its cut path is a prefix of exactly ONE of the diff
 * paths still unclaimed. One candidate is an identification; two are a guess,
 * and a guess is dropped. Dropping it used to be the whole rule, and it made an
 * all-delete patch of three files read "Changed 3 files" — the truncation
 * manufacturing an unknown, over the exact case where saying "delete" matters
 * most.
 *
 * Never derived from an empty `before`/`after`. Codex rebuilds only the changed
 * hunks of a file and Claude's Write mapper hard-codes before:"" for an
 * overwrite — either would report an ordinary update as an "add", which is the
 * same class of lie as a delete that reads like an edit.
 */
export type FileChangeVerb = "add" | "delete" | "update" | "unknown";

const CHANGE_ENTRY = /^(add|delete|update)\s+(\S.*)$/u;

// The tools whose NAME already says what happened, for the engines that send no
// kind at all (Claude's summary is the bare file_path).
//
// Keyed on the EXACT wire name rather than a normalised one, and that is
// load-bearing rather than sloppy: Claude's "Edit" replaces a string inside a
// file that must already exist, so "update" is a fact — while ACP/Grok sends a
// lowercase "edit" whose kind ALSO covers creating the file (grok/mapper.ts
// derives the name from the ACP kind). Folding the two together printed
// "Updated" over a file the agent had just made. `Write` is absent for the same
// reason: its mapper cannot tell a create from an overwrite, so it falls to
// "unknown" and prints the neutral word instead of guessing "Added".
const VERB_BY_TOOL = new Map<string, FileChangeVerb>([
  ["Edit", "update"], ["MultiEdit", "update"], ["NotebookEdit", "update"],
]);

export function fileChangeVerbs(
  item: { tool: string; summary: string },
  diffPaths: readonly string[],
): Map<string, FileChangeVerb> {
  const verbs = new Map<string, FileChangeVerb>();
  const parts = item.summary.split(/,\s+/u);
  for (const [index, part] of parts.entries()) {
    const entry = CHANGE_ENTRY.exec(part.trim());
    if (entry === null) continue;
    const verb = entry[1] as FileChangeVerb;
    if (diffPaths.includes(entry[2])) {
      verbs.set(entry[2], verb);
      continue;
    }
    if (index !== parts.length - 1) continue;
    // The clip marker is not part of the path. An empty remainder means the cut
    // landed on the kind word itself, and a prefix of nothing matches every
    // candidate — which is a guess, not an identification.
    const cut = entry[2].replace(/…$/u, "");
    if (cut.length === 0) continue;
    const candidates = diffPaths.filter(path =>
      !verbs.has(path) && path.startsWith(cut));
    if (candidates.length === 1) verbs.set(candidates[0], verb);
  }
  const fallback = VERB_BY_TOOL.get(item.tool) ?? "unknown";
  for (const path of diffPaths) {
    if (!verbs.has(path)) verbs.set(path, fallback);
  }
  return verbs;
}

/**
 * WHAT BECAME OF A FILE CHANGE. "refused" is a patch that did not happen —
 * failed or declined alike; the two differ in the word beside the verb, never
 * in the verb's own tense.
 */
export type FileChangeOutcome = "ok" | "running" | "refused";

/**
 * THE VERB A MERGED FILE-CHANGE ROW PRINTS. Always printed, including the
 * commonplace "Updated": this app drops words that NEVER vary, and this one
 * varies across add/update/delete. Marking only the dangerous kinds would leave
 * a delete distinguishable by the ABSENCE of a mark, which is the weakest
 * available signal for the one case that matters most.
 *
 * THE TENSE IS THE OUTCOME'S, not the clock's. A past tense asserts the change
 * landed, so a refused patch takes the bare form: "Delete src/old.ts declined"
 * is true and "Deleted src/old.ts declined" says the file both went and stayed,
 * in four words. The row used to choose past tense for everything that was no
 * longer running, which put that contradiction on every failed and every
 * declined patch.
 *
 * The ok/running pairs match repeatedToolLabel's vocabulary. "Changed" is the
 * honest neutral for a mixed patch or an unreadable kind — such a file was
 * still changed, and that is all we can support.
 */
export function fileChangeVerbWord(
  verbs: readonly FileChangeVerb[], outcome: FileChangeOutcome,
): string {
  const distinct = new Set(verbs);
  const only = distinct.size === 1 ? [...distinct][0] : undefined;
  const forms = only === "add" ? ["Added", "Adding", "Add"]
    : only === "delete" ? ["Deleted", "Deleting", "Delete"]
      : only === "update" ? ["Updated", "Updating", "Update"]
        : ["Changed", "Changing", "Change"];
  return forms[outcome === "ok" ? 0 : outcome === "running" ? 1 : 2];
}

/**
 * THE TARGET SLOT OF A MULTI-FILE PATCH. "" means the caller prints the path
 * itself, which is what a one-file change does.
 *
 * "N files" whenever the verb beside it already says what happened. When the
 * kinds DIFFER the verb has to fall back to the neutral "Changed", and "Changed
 * 3 files" is the one shape in which a deletion is invisible without hovering —
 * so the target slot carries the breakdown instead and the row says it out
 * loud.
 *
 * Ordered destructive-first, because this slot ellipsises at the tail in a
 * narrow pane: measured at the 320px pane it holds nine to sixteen monospace
 * characters depending on the state word beside it, so the half that survives
 * the cut has to be the half that cannot be undone.
 *
 * A patch holding a kind we could not read stays "N files". Listing only the
 * entries we know would print counts that do not add up to the file count
 * printed beside them.
 */
const BREAKDOWN_WORDS: readonly (readonly [FileChangeVerb, string])[] = [
  ["delete", "deleted"], ["add", "added"], ["update", "updated"],
];

export function fileChangeTargetText(verbs: readonly FileChangeVerb[]): string {
  if (verbs.length < 2) return "";
  const distinct = new Set(verbs);
  if (distinct.size < 2 || distinct.has("unknown")) {
    return `${verbs.length} files`;
  }
  return BREAKDOWN_WORDS
    .filter(([verb]) => distinct.has(verb))
    .map(([verb, word]) => `${verbs.filter(v => v === verb).length} ${word}`)
    .join(", ");
}

/**
 * A description that is nothing but a path — "/root/reviewer", "src/agents/x" —
 * as opposed to prose that happens to mention one. Separator present, whitespace
 * absent: "Review /src/a.ts and stop" is prose and stays whole.
 */
function isPathShaped(value: string): boolean {
  return !/\s/.test(value) && [...value].some(isSeparator);
}

/**
 * THE TEXT A SUBAGENT ROW'S DESCRIPTION SLOT PRINTS. "" means print no slot,
 * exactly as it does for a tool row.
 *
 * The same whole-string rule toolSummaryRepeatsName makes, plus one clause for
 * the shape of a subagent description specifically. Codex's MultiAgentV2 mapper
 * derives BOTH fields from one string — the type is the last segment of the
 * agent path and the description is the whole path — so every such cell printed
 * its own name twice, the second copy wearing a namespace prefix. Whole-string
 * equality cannot see that: "root reviewer" is not "reviewer".
 *
 * So a description that is a bare PATH is also compared by its last segment.
 * That stays inside the established line — still whole-string equality, never a
 * prefix or substring test on prose — because a path's final segment is a whole
 * name, not the head of a sentence. Claude's descriptions are prose ("Review
 * Task 1 (spec + quality)") and never take this clause.
 *
 * The raw string is untouched in the row's title and in the find corpus.
 */
export function subagentRowDescription(
  subagentType: string, description: string, root?: string,
): string {
  const shown = displayPath(description.trim(), root);
  if (toolSummaryRepeatsName(subagentType, shown)) return "";
  if (isPathShaped(shown)
      && toolSummaryRepeatsName(subagentType, lastSegment(shown))) return "";
  return shortenSummaryPaths(shown, root);
}

/**
 * The subagent with this agentKey, at any depth. Depth-first, so a nested agent
 * is reachable by key alone — a detail surface holds the key it was handed, not
 * the path it was found at, and the tree re-reduces on every event.
 */
export function findSubagentItem(
  items: readonly ConvItem[], agentKey: string,
): Extract<ConvItem, { kind: "subagent" }> | undefined {
  for (const item of items) {
    if (item.kind !== "subagent") continue;
    if (item.id === agentKey) return item;
    const nested = findSubagentItem(item.children, agentKey);
    if (nested !== undefined) return nested;
  }
  return undefined;
}

/** The app's state vocabulary for a status glyph (components/glyphs.tsx). */
export type RunState = "running" | "ok" | "failed" | "ended";

/** A subagent's status as the glyph draws it, in the transcript and the panel alike. */
export function subagentGlyph(status: SubagentStatus): RunState {
  if (status === "running") return "running";
  if (status === "done") return "ok";
  if (status === "failed") return "failed";
  return "ended";
}

const STATUS_WORD: Record<SubagentStatus, string> = {
  running: "Running", done: "Done", failed: "Failed", ended: "Ended",
};

/* The sentence the cell has no room for. "ended" is the reason this block
   exists: its explanation lives only in a `title` attribute on the row, which
   is unreachable by touch and by keyboard. */
const STATUS_SENTENCE: Record<SubagentStatus, string> = {
  running: "Still working. The list below grows as its events arrive.",
  done: "Finished and reported back.",
  failed: "Reported a failure.",
  ended: "The stream stopped before this subagent reported a result.",
};

/** The one word for this subagent state: the glyph's spoken half. */
export function subagentStatusWord(status: SubagentStatus): string {
  return STATUS_WORD[status];
}

/** The same state, in the sentence the row could not hold. */
export function subagentStatusSentence(status: SubagentStatus): string {
  return STATUS_SENTENCE[status];
}

/**
 * The diff rows drawn DIRECTLY under each tool row, by that row's key. A diff
 * item is a top-level sibling of the call that produced it (see diff.produced
 * above), so "immediately following" is the only link between the two, and any
 * other row kind in between ends the group.
 */
export function diffPathsByToolRow(
  rows: readonly TranscriptRow[],
): Map<string, readonly string[]> {
  const out = new Map<string, readonly string[]>();
  for (let index = 0; index < rows.length; index += 1) {
    const row = rows[index];
    if (row.kind !== "item" || row.item.kind !== "tool") continue;
    const paths: string[] = [];
    for (let next = index + 1; next < rows.length; next += 1) {
      const candidate = rows[next];
      if (candidate.kind !== "item" || candidate.item.kind !== "diff") break;
      paths.push(candidate.item.path);
    }
    if (paths.length > 0) out.set(row.key, paths);
  }
  return out;
}

/**
 * Visible label for a fold of produced diffs. Line counts come from
 * `lineDelta` on events this window reduced; unknown or net-zero stats
 * print the file count only. Never `+0 −0`.
 */
export function editedFilesLabel(
  fileCount: number, stat?: DiffStat | null,
): string {
  const noun = fileCount === 1 ? "file" : "files";
  const files = `Edited ${fileCount} ${noun}`;
  const delta = formatDiffStat(stat);
  return delta === "" ? files : `${files} ${delta}`;
}

/** Sum of per-path deltas this window recorded, or null when none were seen. */
export function editedFilesStat(
  items: readonly { added?: number; removed?: number }[],
): DiffStat | null {
  let added = 0;
  let removed = 0;
  let saw = false;
  for (const item of items) {
    if (item.added === undefined && item.removed === undefined) continue;
    saw = true;
    added += item.added ?? 0;
    removed += item.removed ?? 0;
  }
  return saw ? { added, removed } : null;
}

/**
 * WHAT ONE USER TURN DID TO THE WORKING TREE.
 *
 * The line counts are every edit's, because an edit that re-touched a file
 * still moved lines. `stat` is null when no diff in the window carried counts
 * — the same "unknown, so say nothing" rule diff-stats.ts keeps, never a 0/0.
 */
export interface TurnChangeSummary {
  stat: DiffStat | null;
}

/**
 * Which user turns produced file changes, by transcript row key.
 *
 * THE ATTRIBUTION WINDOW is the whole idea: a diff belongs to the LAST user
 * turn above it, and the window closes at the next one. Three consequences,
 * each of them a thing the chip must not get wrong:
 *
 *   · Diffs above the FIRST prompt belong to nobody. A history read that begins
 *     mid-turn has no prompt to hang them on, and hanging them on the next one
 *     would blame it for changes it has not made yet.
 *   · The live seam is not a boundary. A prompt sent before this window opened
 *     whose edits arrive live is one turn, drawn across the mark.
 *   · A subagent's edits are the turn's own. The human typed one prompt; who
 *     inside the agent tree did the writing is not a fact about the turn.
 *     A user turn NESTED in a subagent is the parent agent's prompt, not the
 *     human's, so it never opens a window of its own — only rows do.
 *
 * A turn with no entry is a turn that changed nothing this window SAW, and
 * the caller draws nothing at all for it. Only `appliedDiffs` count: a
 * declined or failed edit changed nothing.
 */
export function turnChangeSummaries(
  rows: readonly TranscriptRow[],
): Map<string, TurnChangeSummary> {
  const out = new Map<string, TurnChangeSummary>();
  let open: string | null = null;
  let turn: ConvItem[] = [];
  const close = (): void => {
    if (open !== null && turn.length > 0) {
      const diffs = appliedDiffs(turn);
      if (diffs.length > 0) out.set(open, { stat: editedFilesStat(diffs) });
    }
    turn = [];
  };
  for (const row of rows) {
    if (row.kind !== "item") continue;
    if (row.item.kind === "user") {
      close();
      open = row.key;
      continue;
    }
    if (open === null) continue;
    turn.push(row.item);
  }
  close();
  return out;
}

const NO_FAILED_CALLS: ReadonlySet<string> = new Set();
const SUBAGENT_FAILED_CALLS = new WeakMap<SubagentItem, ReadonlySet<string>>();

// The calls this row stands for that came back failed or declined, at any
// depth. Kept per subagent card for the same reason producedDiffs is.
function failedCalls(item: ConvItem): ReadonlySet<string> {
  if (item.kind === "tool") return item.result?.ok === false ? new Set([item.id]) : NO_FAILED_CALLS;
  if (item.kind !== "subagent") return NO_FAILED_CALLS;
  const known = SUBAGENT_FAILED_CALLS.get(item);
  if (known !== undefined) return known;
  const failed = new Set<string>();
  for (const child of item.children) for (const id of failedCalls(child)) failed.add(id);
  const kept = failed.size === 0 ? NO_FAILED_CALLS : failed;
  SUBAGENT_FAILED_CALLS.set(item, kept);
  return kept;
}

/**
 * THE DIFFS AMONG THESE ITEMS THAT CHANGED SOMETHING, subagents' included.
 *
 * A diff arrives with its call, before the call runs or is approved — Claude's
 * from the Edit's input, Codex's from the proposed patch, ACP's announced
 * before the call resolves. So a diff whose call came back `ok: false`
 * (declined or failed) changed nothing, and every count of what changed skips
 * it. A call still running keeps its diffs: it has not been refused. The same
 * rule over raw engine events is diff-stats.ts's `appliedDiffEvents`.
 */
export function appliedDiffs(
  items: readonly ConvItem[],
): Extract<ConvItem, { kind: "diff" }>[] {
  const failed = new Set<string>();
  for (const item of items) for (const id of failedCalls(item)) failed.add(id);
  const out: Extract<ConvItem, { kind: "diff" }>[] = [];
  for (const item of items) {
    for (const diff of producedDiffs(item)) if (!failed.has(diff.callId)) out.push(diff);
  }
  return out;
}

const NO_PRODUCED_DIFFS: readonly Extract<ConvItem, { kind: "diff" }>[] = [];
const SUBAGENT_DIFFS = new WeakMap<SubagentItem, readonly Extract<ConvItem, { kind: "diff" }>[]>();

// Every diff this row stands for, including the ones a subagent it holds
// produced at any depth. A nested user turn is deliberately not a boundary
// here: it is the parent agent's prompt, and the edits under it were still
// caused by the human turn this row belongs to.
//
// Kept per subagent card: the dirty pill and the turn chips ask for every row
// on every streamed frame, and a settled card's tree does not change.
export function producedDiffs(item: ConvItem): readonly Extract<ConvItem, { kind: "diff" }>[] {
  if (item.kind === "diff") return [item];
  if (item.kind !== "subagent") return NO_PRODUCED_DIFFS;
  const known = SUBAGENT_DIFFS.get(item);
  if (known !== undefined) return known;
  const diffs = item.children.flatMap(producedDiffs);
  SUBAGENT_DIFFS.set(item, diffs);
  return diffs;
}

/**
 * The chip's accessible sentence. It names the destination (Review) rather
 * than the numbers alone, because the chip is a control and not a caption, and
 * it borrows diffStatLabel's silence: counts this window never saw are simply
 * absent instead of spoken as zero.
 *
 * IT NO LONGER SPEAKS A FILE COUNT. The row already carries one, on the Undo
 * control beside this chip, and the two were counted from different sources —
 * this one from the diff paths this window saw, that one from the files the
 * checkpoint actually holds — so a row could print "2 files" next to "Restores
 * 3 files" and be right twice. One file count per row, and it belongs to the
 * control that can act on it. The chip keeps the line delta, which nothing else
 * on the row states.
 */
export function turnChangesLabel(summary: TurnChangeSummary): string {
  const delta = diffStatLabel(summary.stat);
  return delta === ""
    ? "Review changes from this message"
    : `Review changes from this message: ${delta}`;
}

const COMMAND_ACTIVITY = new Set(["bash", "shell", "run", "terminal",
  "commandexecution", "execcommand", "execute", "runterminalcommand"]);
const READ_ACTIVITY = new Set(["read", "readfile"]);
const SEARCH_ACTIVITY = new Set(["grep", "glob", "search", "searchfiles", "websearch"]);
const EDIT_ACTIVITY = new Set(["write", "writefile", "edit", "multiedit",
  "notebookedit", "filechange", "applypatch", "searchreplace",
  "strreplace", "replacetext"]);
const LIST_ACTIVITY = new Set(["list", "listdir", "listdirectory"]);

export type ToolActivityKind = "command" | "read" | "search" | "edit" | "list" | "other";

/** What kind of work a provider tool name is, after the usual punctuation noise. */
export function toolActivityKind(tool: string): ToolActivityKind {
  const key = normalizedToolKey(tool);
  if (COMMAND_ACTIVITY.has(key)) return "command";
  if (READ_ACTIVITY.has(key)) return "read";
  if (SEARCH_ACTIVITY.has(key)) return "search";
  if (EDIT_ACTIVITY.has(key)) return "edit";
  if (LIST_ACTIVITY.has(key)) return "list";
  return "other";
}

export function isCommandActivity(tool: string): boolean {
  return toolActivityKind(tool) === "command";
}

function isWriteActivity(tool: string): boolean {
  const key = normalizedToolKey(tool);
  return key === "write" || key === "writefile";
}

function fileLeaf(path: string, root?: string): string {
  const shown = displayPath(path, root);
  const parts = shown.split(/[/\\]/u).filter(part => part.length > 0);
  return parts[parts.length - 1] ?? shown;
}

/** The one file a call's input names, when it names exactly one by a known key. */
export function pathFromToolInput(input: unknown): string | undefined {
  if (typeof input !== "object" || input === null) return undefined;
  const rec = input as Record<string, unknown>;
  for (const key of ["path", "file_path", "target_file", "filePath"]) {
    const value = rec[key];
    if (typeof value === "string" && value.length > 0) return value;
  }
  return undefined;
}

export interface WorkGroupPart {
  text: string;
  added?: number;
  removed?: number;
}

/**
 * Claude Code's collapsed work line: one sentence for a stretch of tools.
 * Commands and file mutations lead; reads and searches only appear when
 * nothing was run or changed.
 */
export function workGroupParts(input: {
  tools: readonly { tool: string; summary: string; input?: unknown;
    result?: { ok: boolean; summary: string } }[];
  diffs?: readonly { path: string; added?: number; removed?: number }[];
  running: boolean;
  root?: string;
}): WorkGroupPart[] {
  const tools = input.tools;
  const diffs = input.diffs ?? [];
  const running = input.running;
  const buckets: Record<ToolActivityKind, typeof tools[number][]> = {
    command: [], read: [], search: [], edit: [], list: [], other: [],
  };
  for (const item of tools) buckets[toolActivityKind(item.tool)].push(item);
  const commands = buckets.command;
  const reads = buckets.read;
  const searches = buckets.search;
  const edits = buckets.edit;
  const lists = buckets.list;
  const others = buckets.other;
  const mutated = commands.length > 0 || edits.length > 0;
  const parts: WorkGroupPart[] = [];

  if (commands.length > 0) {
    if (running) {
      parts.push({ text: commands.length === 1
        ? "Running command" : `Running ${commands.length} commands` });
    } else {
      parts.push({ text: commands.length === 1
        ? "Ran a command" : `Ran ${commands.length} commands` });
    }
  }

  if (edits.length > 0) {
    const paths = [...new Set(diffs.length > 0
      ? diffs.map(diff => diff.path)
      : edits.flatMap(item => {
          const fromInput = pathFromToolInput(item.input);
          return fromInput === undefined ? [] : [fromInput];
        }))];
    const stat = editedFilesStat(diffs);
    const creating = edits.every(item => isWriteActivity(item.tool));
    const verb = running
      ? (creating ? "creating" : "editing")
      : (creating ? "created" : "edited");
    const count = Math.max(paths.length, edits.length);
    const text = paths.length === 1
      ? `${verb} ${fileLeaf(paths[0], input.root)}`
      : `${verb} ${count} ${count === 1 ? "file" : "files"}`;
    parts.push({
      text,
      ...(stat === null ? {} : { added: stat.added, removed: stat.removed }),
    });
  }

  if (!mutated) {
    if (reads.length > 0) {
      parts.push({ text: running
        ? (reads.length === 1 ? "Reading file" : `Reading ${reads.length} files`)
        : (reads.length === 1 ? "Read file" : `Read ${reads.length} files`) });
    }
    if (searches.length > 0) {
      parts.push({ text: running
        ? (searches.length === 1 ? "Searching" : `Searching ${searches.length} times`)
        : (searches.length === 1 ? "Searched" : `Searched ${searches.length} times`) });
    }
    if (lists.length > 0) {
      parts.push({ text: running
        ? (lists.length === 1 ? "Listing directory" : `Listing ${lists.length} directories`)
        : (lists.length === 1 ? "Listed directory" : `Listed ${lists.length} directories`) });
    }
    if (others.length > 0 && parts.length === 0) {
      const name = toolActivityGroupName(others[0].tool);
      parts.push({ text: running
        ? (others.length === 1 ? `Calling ${name}` : `Calling ${name} · ${others.length} calls`)
        : (others.length === 1 ? name : `Called ${name} ${others.length} times`) });
    }
  }

  return parts;
}

/** The plain-text form of already-computed parts, as `WorkGroupText` draws them. */
export function workGroupPartsLabel(parts: readonly WorkGroupPart[]): string {
  return parts.map(part => {
    const delta = formatDiffStat(part.added === undefined
      ? null : { added: part.added, removed: part.removed ?? 0 });
    return delta === "" ? part.text : `${part.text} ${delta}`;
  }).join(", ");
}

/**
 * The failure word on a folded command run. Success stays wordless, exactly as
 * it does on a tool row; a failure that folds away behind "Ran 3 commands" is
 * the one state the collapsed line MUST still carry.
 */
export function commandRunFailureLabel(failed: number, total: number): string {
  if (failed <= 0) return "";
  return total === 1 ? "failed" : `${failed} failed`;
}

// Tool kinds we can name honestly. A tool outside these lists gets no icon
// rather than a decorative default — the engines mint tool names freely (MCP
// calls arrive as "server/tool"), and a glyph that guesses wrong is worse than
// none. Claude's names are the CLI's; the Codex ones are its item types.
// A second vocabulary beside the activity Sets above: these match the
// lower-cased name only, the Sets a normalized one. Edit both or neither
// (electron/AGENTS.md).
const FILE_TOOLS = new Set(["read", "write", "edit", "multiedit", "notebookedit",
  "read_file", "write_file", "list_dir", "grep", "glob"]);
const DIFF_TOOLS = new Set(["filechange", "applypatch", "apply_patch"]);

/** The codicon a tool row wears, or null for a tool it cannot name honestly. */
export function toolGlyph(tool: string): string | null {
  const key = tool.toLowerCase();
  if (isCommandActivity(tool)) return "terminal";
  if (FILE_TOOLS.has(key)) return "file";
  if (DIFF_TOOLS.has(key)) return "git-compare";
  return null;
}

/* ==========================================================================
   What an expanded tool call shows for its arguments.
   ========================================================================== */

/**
 * One block of a tool call's input, as it is meant to be read.
 * `label` is the argument the block came from; a block without one is the whole
 * payload, pretty-printed exactly as this pane has always printed it.
 */
export interface ToolInputBlock {
  label?: string;
  text: string;
}

/**
 * Arguments whose value IS the thing that ran, however short. Everything else
 * qualifies on shape alone (any string holding a line break), so a tool nobody
 * anticipated still reads correctly.
 */
const TOOL_INPUT_BODY_KEYS = new Set([
  "command", "content", "new_string", "old_string", "patch", "prompt", "script",
]);

/** JSON.stringify throws on a cycle or a BigInt; a tool payload is engine data. */
function safeJson(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2) ?? String(value);
  } catch {
    return String(value);
  }
}

/**
 * Split a tool call's input into what a human actually reads.
 *
 * The defect this closes: a Bash call's input is an object holding a multi-line
 * shell command, and JSON.stringify renders each of those newlines as the two
 * characters \ and n — so the one thing worth reading, exactly what the agent
 * ran, arrives as a single escaped line. Reviewing that is a core job of this
 * app, so a body argument gets its own block with its real line breaks.
 *
 * Leftover keys (`timeout`, `description`, a path) stay out of the expanded
 * body once a named body argument was extracted — they remain on the row
 * title and in the find corpus. A payload with no body argument at all is
 * returned untouched, which is what keeps a Read or a Grep row as JSON.
 *
 * `summary` is the sentence the collapsed row already carries (Bash's
 * `description` is that sentence verbatim). An argument that only repeats it is
 * dropped rather than said twice, and only when leftover JSON is still shown.
 */
export function toolInputBlocks(input: unknown, summary = ""): ToolInputBlock[] {
  if (input === undefined) return [];
  if (typeof input === "string") return input === "" ? [] : [{ text: input }];
  if (input === null || typeof input !== "object" || Array.isArray(input)) {
    return [{ text: safeJson(input) }];
  }
  let entries: [string, unknown][];
  try {
    entries = Object.entries(input as Record<string, unknown>);
  } catch {
    return [{ text: safeJson(input) }];
  }

  const blocks: ToolInputBlock[] = [];
  const rest: Record<string, unknown> = {};
  let extractedBodyKey = false;
  for (const [key, value] of entries) {
    const named = TOOL_INPUT_BODY_KEYS.has(key);
    const body = typeof value === "string" && value !== ""
      && (value.includes("\n") || named);
    if (body) {
      if (named) extractedBodyKey = true;
      blocks.push({ label: key, text: value as string });
    }
    else rest[key] = value;
  }
  // Nothing to unescape: this payload reads fine as the object it is, and the
  // pane keeps printing it that way.
  if (blocks.length === 0) return [{ text: safeJson(input) }];
  // A named body is the thing that ran. Timeout / description / path leftovers
  // are not a second pane of JSON underneath it.
  if (extractedBodyKey) return blocks;

  const said = summary.trim();
  if (said !== "") {
    for (const key of Object.keys(rest)) {
      const value = rest[key];
      if (typeof value === "string" && value.trim() === said) delete rest[key];
    }
  }
  if (Object.keys(rest).length > 0) blocks.push({ text: safeJson(rest) });
  return blocks;
}
