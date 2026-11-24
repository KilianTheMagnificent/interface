import type {
  DiffFile, DiffFileStatus, DiffOmitted, DiffScope, ThreadCommand, ThreadDiff, ThreadView,
} from "../shared/threads";

/**
 * The thread diff panel's pure rules: which scopes a thread offers, the
 * command each one sends, how a file is described, how its patch reads as
 * numbered hunks (stacked, or paired side by side), and what a commented line
 * puts into the prompt.
 * Rust computes every diff (`turnDiff` / `threadDiff`); nothing here reads git.
 */

/** What the panel is showing: one turn, or one of Rust's thread-wide scopes. */
export type ThreadDiffChoice =
  | { scope: "turn"; ordinal: number }
  | { scope: Exclude<DiffScope, "turn"> };

/** A turn Rust can diff: it captured the start checkpoint. */
export function diffableTurns(thread: ThreadView): number[] {
  return thread.turns.filter(turn => turn.hasStart).map(turn => turn.ordinal)
    .sort((a, b) => a - b);
}

/** Turn ordinals are 0-based; the owner counts prompts from 1. */
export function turnLabel(ordinal: number): string {
  return `Turn ${ordinal + 1}`;
}

/** When a turn was sent, as the clock read: "2:16 PM". */
export function turnTime(sentAt: number): string {
  return new Date(sentAt).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
}

const NO_CHECKPOINT = "No checkpoint was captured for this thread yet.";
const NO_TURN_CHECKPOINT = "No checkpoint was captured for this turn.";

/** One row of the scope menu. */
export interface ThreadDiffScopeEntry {
  /** `threadDiffScopeValue` of `choice`: the menu's value for this row. */
  value: string;
  choice: ThreadDiffChoice;
  /** "Turn 3", "Whole thread", "Uncommitted", "Branch". */
  label: string;
  /** The turn's send time, or null for a thread-wide scope. */
  time: string | null;
  /** Why the row cannot be chosen, or null. */
  disabled: string | null;
}

/** The menu value for a choice: `turn:2`, `thread`, `uncommitted`, `branch`. */
export function threadDiffScopeValue(choice: ThreadDiffChoice): string {
  return choice.scope === "turn" ? `turn:${choice.ordinal}` : choice.scope;
}

/** The choice a menu value names, or null for a value it never offered. */
export function threadDiffChoiceFromValue(value: string): ThreadDiffChoice | null {
  const turn = /^turn:(\d+)$/u.exec(value);
  if (turn !== null) return { scope: "turn", ordinal: Number(turn[1]) };
  return value === "thread" || value === "uncommitted" || value === "branch" ? { scope: value } : null;
}

/** Branch exists only for a worktree thread with a base to measure against. */
function offersBranch(thread: ThreadView): boolean {
  return thread.workspace?.mode === "worktree" && thread.workspace.baseRef !== undefined;
}

/**
 * The scope menu: every recorded turn, newest first ("Turn 3" at its send
 * time), then Whole thread / Uncommitted / Branch. A turn without a start
 * checkpoint is listed but cannot be chosen, and Whole thread needs one
 * checkpoint at least. The current choice is always a row, so the menu can
 * name what the panel shows even when the thread no longer lists it.
 */
export function threadDiffScopes(thread: ThreadView, choice: ThreadDiffChoice,
    formatTime: (sentAt: number) => string = turnTime): ThreadDiffScopeEntry[] {
  const turns = [...thread.turns].sort((a, b) => b.ordinal - a.ordinal);
  const entries: ThreadDiffScopeEntry[] = turns.map(turn => ({
    value: threadDiffScopeValue({ scope: "turn", ordinal: turn.ordinal }),
    choice: { scope: "turn", ordinal: turn.ordinal },
    label: turnLabel(turn.ordinal),
    time: formatTime(turn.sentAt),
    disabled: turn.hasStart ? null : NO_TURN_CHECKPOINT,
  }));
  if (choice.scope === "turn" && !turns.some(turn => turn.ordinal === choice.ordinal)) {
    entries.unshift({ value: threadDiffScopeValue(choice), choice, label: turnLabel(choice.ordinal),
      time: null, disabled: null });
  }
  entries.push(
    { value: "thread", choice: { scope: "thread" }, label: "Whole thread", time: null,
      disabled: diffableTurns(thread).length === 0 ? NO_CHECKPOINT : null },
    { value: "uncommitted", choice: { scope: "uncommitted" }, label: "Uncommitted", time: null,
      disabled: null },
  );
  if (offersBranch(thread) || choice.scope === "branch") {
    entries.push({ value: "branch", choice: { scope: "branch" }, label: "Branch", time: null,
      disabled: null });
  }
  return entries;
}

/** What the scope button says: "Turn 3 · 2:16 PM", "Whole thread", … */
export function threadDiffScopeLabel(entry: Pick<ThreadDiffScopeEntry, "label" | "time">): string {
  return entry.time === null ? entry.label : `${entry.label} · ${entry.time}`;
}

/**
 * Whether a turn's Undo may be offered beside its diff. A local thread undoes
 * through its per-turn checkpoint (ConversationTranscript's `restoreReady`);
 * a worktree thread through Rust's git restore, which only a worktree the
 * thread created may use — a shared one is another thread's files to rewind.
 * The caller still decides whether that turn has a restore point at all, and
 * Rust refuses with a sentence when its own rules say no.
 */
export function turnUndoAllowed(thread: ThreadView): boolean {
  const workspace = thread.workspace;
  if (workspace === undefined || workspace.mode === "none") return false;
  return workspace.mode !== "worktree" || workspace.owned;
}

/** Where the panel opens when nothing asked for a particular turn. */
export function defaultThreadDiffChoice(thread: ThreadView): ThreadDiffChoice {
  return diffableTurns(thread).length > 0 ? { scope: "thread" } : { scope: "uncommitted" };
}

export function threadDiffCommand(key: string, choice: ThreadDiffChoice): ThreadCommand {
  return choice.scope === "turn"
    ? { kind: "turnDiff", key, ordinal: choice.ordinal }
    : { kind: "threadDiff", key, scope: choice.scope };
}

export function sameThreadDiffChoice(a: ThreadDiffChoice, b: ThreadDiffChoice): boolean {
  return a.scope === b.scope
    && (a.scope !== "turn" || b.scope !== "turn" || a.ordinal === b.ordinal);
}

/** Does an answer belong to this request? A late answer for another scope never paints. */
export function diffAnswers(diff: ThreadDiff, choice: ThreadDiffChoice): boolean {
  return diff.scope === choice.scope;
}

const STATUS_WORD: Record<DiffFileStatus, string> = {
  added: "Added", modified: "Modified", deleted: "Deleted", renamed: "Renamed",
};
const STATUS_MARK: Record<DiffFileStatus, string> = {
  added: "A", modified: "M", deleted: "D", renamed: "R",
};

export function diffStatusWord(status: DiffFileStatus): string { return STATUS_WORD[status]; }
export function diffStatusMark(status: DiffFileStatus): string { return STATUS_MARK[status]; }

/** Why Rust left a file's patch out, in words. */
export function omittedSentence(reason: DiffOmitted): string {
  switch (reason) {
    case "sensitive": return "Not shown: this file may hold credentials.";
    case "large": return "Not shown: this file's change is larger than 256 KiB.";
    case "binary": return "Binary file: there is no text to compare.";
  }
}

export const TRUNCATED_SENTENCE =
  "Some files are listed without their changes: this diff is larger than 2 MiB.";

/** The patch body Rust sent, or why there is none. */
export function filePatchState(file: DiffFile): { kind: "patch"; text: string }
    | { kind: "omitted"; sentence: string } | { kind: "none"; sentence: string } {
  if (file.omitted !== undefined) return { kind: "omitted", sentence: omittedSentence(file.omitted) };
  if (file.binary) return { kind: "omitted", sentence: omittedSentence("binary") };
  if (file.patch === undefined || file.patch === "") {
    return { kind: "none", sentence: file.status === "renamed"
      ? "Renamed without changes." : "No line changes." };
  }
  return { kind: "patch", text: file.patch };
}

/** A patch line as the panel draws it: its kind, its text without the marker, its numbers. */
export interface DiffRow {
  /** `note` is git's "\ No newline at end of file" and anything else that is not a line. */
  kind: "add" | "del" | "context" | "note";
  text: string;
  /** The line's number before the change; null for an added line or a note. */
  oldLine: number | null;
  /** The line's number after the change; null for a removed line or a note. */
  newLine: number | null;
}

export interface DiffHunk {
  oldStart: number;
  oldCount: number;
  newStart: number;
  newCount: number;
  /** What git names after the second `@@` (the enclosing function), or "". */
  section: string;
  /** Unchanged lines between the previous hunk (or the top of the file) and this one. */
  skippedBefore: number;
  rows: DiffRow[];
}

export interface ParsedPatch {
  hunks: DiffHunk[];
  /** A patch with no hunk (a mode change): git's header is all there is. */
  header: string[];
}

const HUNK_HEADER = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@ ?(.*)$/u;

/**
 * A unified patch, in hunks, each line numbered on the side it belongs to.
 *
 * Git's header (`diff --git`, `index`, `new file mode`, `---`/`+++`) repeats
 * what the file row already says, so it is dropped when there is a hunk; a
 * patch without one keeps it as `header`. Inside a hunk the first character
 * alone decides, so a removed line that began with `--` is a deletion, not a
 * header, and once the hunk's counts are spent nothing more is numbered.
 * `skippedBefore` is read off the hunk headers alone: the lines before the
 * first hunk, and between each hunk and the next, that the patch leaves out.
 * What follows the last hunk is not in the patch, so it is not counted.
 */
export function parsePatch(patch: string): ParsedPatch {
  const lines = patch.endsWith("\n") ? patch.slice(0, -1).split("\n") : patch.split("\n");
  const hunks: DiffHunk[] = [];
  const header: string[] = [];
  let hunk: DiffHunk | null = null;
  let oldLine = 0;
  let newLine = 0;
  let oldLeft = 0;
  let newLeft = 0;
  /** The last old-side line the previous hunk covers (0: the top of the file). */
  let lastOld = 0;
  for (const text of lines) {
    const match = HUNK_HEADER.exec(text);
    if (match !== null) {
      const oldStart = Number(match[1]);
      const oldCount = match[2] === undefined ? 1 : Number(match[2]);
      const newStart = Number(match[3]);
      const newCount = match[4] === undefined ? 1 : Number(match[4]);
      // A count of 0 names the line AFTER which the hunk sits.
      const firstOld = oldCount === 0 ? oldStart + 1 : oldStart;
      hunk = { oldStart, oldCount, newStart, newCount, section: match[5].trim(),
        skippedBefore: Math.max(0, firstOld - lastOld - 1), rows: [] };
      hunks.push(hunk);
      lastOld = oldCount === 0 ? oldStart : oldStart + oldCount - 1;
      oldLine = oldStart;
      newLine = newStart;
      oldLeft = oldCount;
      newLeft = newCount;
      continue;
    }
    if (hunk === null) { header.push(text); continue; }
    const marker = text.charAt(0);
    if (marker === "+" && newLeft > 0) {
      hunk.rows.push({ kind: "add", text: text.slice(1), oldLine: null, newLine: newLine++ });
      newLeft -= 1;
    } else if (marker === "-" && oldLeft > 0) {
      hunk.rows.push({ kind: "del", text: text.slice(1), oldLine: oldLine++, newLine: null });
      oldLeft -= 1;
    } else if ((marker === " " || text === "") && oldLeft > 0 && newLeft > 0) {
      // An empty line is a context line whose leading space was trimmed.
      hunk.rows.push({ kind: "context", text: text.slice(1), oldLine: oldLine++, newLine: newLine++ });
      oldLeft -= 1;
      newLeft -= 1;
    } else if (text !== "" || oldLeft > 0 || newLeft > 0) {
      hunk.rows.push({ kind: "note", text, oldLine: null, newLine: null });
    }
  }
  return { hunks, header: hunks.length === 0 ? header : [] };
}

/** The number a row shows: the new side for context and added lines, the old side for removed ones. */
export function diffRowLine(row: DiffRow): number | null {
  return row.kind === "del" ? row.oldLine : row.newLine;
}

/** One side of a split row: the parsed row it draws, and that row's index in its hunk. */
export interface SplitCell {
  row: DiffRow;
  /**
   * The row's index in `DiffHunk.rows`. Selection, quoting, marks and syntax
   * colours all key on it, so a split cell is the same line as its stacked row.
   */
  index: number;
}

/** One row of the split layout: the old file on the left, the new on the right; null is an empty filler. */
export interface SplitRow {
  left: SplitCell | null;
  right: SplitCell | null;
}

/**
 * A hunk's rows side by side. A context line sits on both sides (the same
 * cell twice). Inside a change, the run of removed lines is paired with the
 * run of added lines that follows it, row by row: removal i beside addition i,
 * and whichever run is longer leaves the other side empty. An addition with
 * no removal before it has an empty left; a removal after an addition starts
 * a new pairing, so nothing is ever drawn out of order on its own side. A
 * note (git's "\ No newline at end of file") stays on the side of the line it
 * follows, or on both after a context line.
 */
export function splitHunkRows(rows: readonly DiffRow[]): SplitRow[] {
  const split: SplitRow[] = [];
  let left: SplitCell[] = [];
  let right: SplitCell[] = [];
  /** Which side the previous row was drawn on, for a note that follows it. */
  let side: "left" | "right" | "both" = "both";
  const flush = (): void => {
    for (let at = 0; at < Math.max(left.length, right.length); at += 1) {
      split.push({ left: left[at] ?? null, right: right[at] ?? null });
    }
    left = [];
    right = [];
  };
  rows.forEach((row, index) => {
    const cell: SplitCell = { row, index };
    if (row.kind === "del") {
      if (right.length > 0) flush();
      left.push(cell);
      side = "left";
    } else if (row.kind === "add") {
      right.push(cell);
      side = "right";
    } else if (row.kind === "note" && side === "left") {
      left.push(cell);
    } else if (row.kind === "note" && side === "right") {
      right.push(cell);
    } else {
      flush();
      split.push({ left: cell, right: cell });
      side = "both";
    }
  });
  flush();
  return split;
}

/** How the Changes panel draws a patch: one column, or the old and new files side by side. */
export type ThreadDiffLayout = "stacked" | "split";

/** The viewer's layout choice, kept in this browser's storage beside the Threads scope. */
export const THREAD_DIFF_LAYOUT_KEY = "interface.threads.diff-layout";

/** A stored layout, or Stacked for anything else (nothing stored, a foreign value). */
export function parseThreadDiffLayout(raw: string | null | undefined): ThreadDiffLayout {
  return raw === "split" ? "split" : "stacked";
}

/**
 * The narrowest panel that draws two columns, in CSS pixels: each side then
 * keeps about 25 characters of code beside its numbers. The Changes column is
 * 400–560px wide (theme.css `.has-thread-diff`), so Split shows from a window
 * of about 1,390px.
 */
export const SPLIT_DIFF_MIN_WIDTH = 500;

/** Whether a panel this many CSS pixels wide has room for Split. */
export function splitDiffFits(width: number): boolean {
  return width >= SPLIT_DIFF_MIN_WIDTH;
}

/**
 * The layout drawn: Split only when it is chosen and fits. `fits` is null
 * before the panel is measured (as on the server), which draws the choice.
 */
export function shownThreadDiffLayout(chosen: ThreadDiffLayout, fits: boolean | null): ThreadDiffLayout {
  return chosen === "split" && fits !== false ? "split" : "stacked";
}

/** "12 unchanged lines" for the band a hunk header leaves out. */
export function unchangedLinesText(count: number): string {
  return `${count} unchanged ${count === 1 ? "line" : "lines"}`;
}

/** Lines of a diff quoted into the prompt draft, with what the owner said about them. */
export interface ThreadDiffQuote {
  /** The file's path as Rust sent it (relative to the repository). */
  path: string;
  startLine: number;
  endLine: number;
  /** `old`: every quoted line was removed, so the numbers are the old file's. */
  side: "new" | "old";
  /** The quoted lines as the diff shows them, each with its `+`, `-` or space. */
  code: string;
  comment: string;
}

const ROW_MARKER: Record<Exclude<DiffRow["kind"], "note">, string> = { add: "+", del: "-", context: " " };

/**
 * The quote a run of rows makes, or null when it holds no line. A run of only
 * removed lines is numbered on the old side; any other run on the new side,
 * by the lines it keeps or adds.
 */
export function diffQuoteFor(path: string, rows: readonly DiffRow[], comment: string): ThreadDiffQuote | null {
  const lines = rows.filter((row): row is DiffRow & { kind: "add" | "del" | "context" } =>
    row.kind !== "note");
  if (lines.length === 0) return null;
  const side = lines.every(row => row.kind === "del") ? "old" : "new";
  const numbers = lines.map(row => side === "old" ? row.oldLine : row.newLine)
    .filter((line): line is number => line !== null);
  return {
    path, side,
    startLine: Math.min(...numbers),
    endLine: Math.max(...numbers),
    code: lines.map(row => `${ROW_MARKER[row.kind]}${row.text}`).join("\n"),
    comment: comment.trim(),
  };
}

/** "line 21", or "lines 14–17" for a range. */
export function diffQuoteLines(quote: Pick<ThreadDiffQuote, "startLine" | "endLine">): string {
  return quote.startLine === quote.endLine
    ? `line ${quote.startLine}` : `lines ${quote.startLine}–${quote.endLine}`;
}

/**
 * What a quote puts into the prompt draft: `path:21` (or `path:14-17`), the
 * lines in a fenced diff block, the comment, and a trailing blank line so the
 * next quote or the owner's own words start on a fresh paragraph. Removed
 * lines say so in the header, since their numbers are the old file's. The
 * fence is longer than any run of backticks in the code, so code that holds
 * a fence of its own cannot close this one.
 */
export function formatDiffQuote(quote: ThreadDiffQuote): string {
  const lines = quote.startLine === quote.endLine
    ? `${quote.startLine}` : `${quote.startLine}-${quote.endLine}`;
  const header = `${quote.path}:${lines}${quote.side === "old" ? " (removed)" : ""}`;
  const longest = Math.max(0, ...[...quote.code.matchAll(/`+/gu)].map(run => run[0].length));
  const fence = "`".repeat(Math.max(3, longest + 1));
  const comment = quote.comment.trim();
  return [header, `${fence}diff`, quote.code, fence, ...(comment === "" ? [] : [comment]), "", ""]
    .join("\n");
}

/**
 * A thread's draft with a quote added: after whatever is already typed, one
 * blank line apart, so a review comment never overwrites the owner's words.
 */
export function appendDiffQuote(draft: string, quote: ThreadDiffQuote): string {
  const current = draft.replace(/\s+$/u, "");
  const block = formatDiffQuote(quote);
  return current === "" ? block : `${current}\n\n${block}`;
}

/** "src/a.ts → src/b.ts" for a rename, the path otherwise. */
export function diffFileName(file: DiffFile): string {
  return file.oldPath !== undefined && file.oldPath !== file.path
    ? `${file.oldPath} → ${file.path}` : file.path;
}

/**
 * A file header's label in two parts, so the file's name reads first: what
 * comes before its last separator (the folders, and a rename's old path),
 * drawn dimmer, and the name itself. The two join back to the label.
 */
export function diffPathParts(label: string): { dir: string; name: string } {
  const cut = Math.max(label.lastIndexOf("/"), label.lastIndexOf("\\")) + 1;
  return { dir: label.slice(0, cut), name: label.slice(cut) };
}

/** The lines a whole answer added and removed. */
export function diffTotals(diff: ThreadDiff): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const file of diff.files) { added += file.added; removed += file.removed; }
  return { added, removed };
}

/** "3 files · +41 −12", or "No changes" for an empty answer. */
export function diffSummaryText(diff: ThreadDiff): string {
  if (diff.files.length === 0) return "No changes";
  const { added, removed } = diffTotals(diff);
  const files = `${diff.files.length} ${diff.files.length === 1 ? "file" : "files"}`;
  return added === 0 && removed === 0 ? files : `${files} · +${added} −${removed}`;
}
