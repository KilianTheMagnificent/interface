import {
  isCommandActivity, isFinishedWork, toolActivityGroupKey, toolActivityKind,
} from "./tool-activity";
import type { ConvItem } from "./conversation-reducer";
import type { TranscriptRow } from "./transcript-merge";

export type ToolTranscriptRow = Extract<TranscriptRow, { kind: "item" }> & {
  item: Extract<ConvItem, { kind: "tool" }>;
};
export type DiffItem = Extract<ConvItem, { kind: "diff" }>;
export type ConversationDisplayRow =
  | { kind: "row"; row: TranscriptRow }
  | { kind: "tool-run"; key: string; tool: string; rows: ToolTranscriptRow[];
      diffs?: DiffItem[]; members?: TranscriptRow[] }
  // One file change drawn ONCE: the tool row and the diff rows it produced,
  // which are two renderings of the same fact. `keys` is every transcript key
  // this one display row now stands for, so a find hit on either half still
  // scrolls to something.
  | { kind: "file-change"; key: string; row: ToolTranscriptRow; items: DiffItem[];
      keys: string[] }
  // `keys` for the same reason the merged row carries them: this control stands
  // where several rows used to, and a find hit lands on a key.
  | { kind: "edited-files"; key: string; items: DiffItem[]; paths: string[];
      keys: string[] }
  | { kind: "subagent-grid"; key: string; rows: SubagentTranscriptRow[] };

export type SubagentTranscriptRow = Extract<TranscriptRow, { kind: "item" }> & {
  item: Extract<ConvItem, { kind: "subagent" }>;
};

export const isShellTool = (tool: string): boolean => isCommandActivity(tool);

export function isToolRow(row: TranscriptRow): row is ToolTranscriptRow {
  return row.kind === "item" && row.item.kind === "tool";
}

export function isDiffItemRow(row: TranscriptRow): row is Extract<TranscriptRow, { kind: "item" }> & {
  item: DiffItem;
} {
  return row.kind === "item" && row.item.kind === "diff";
}

export function isWorkItemRow(row: TranscriptRow): row is Extract<TranscriptRow, { kind: "item" }> {
  return isToolRow(row) || isDiffItemRow(row);
}

export function isShellToolRow(row: TranscriptRow): row is ToolTranscriptRow {
  return isToolRow(row) && isShellTool(row.item.tool);
}

export function groupableToolRow(row: TranscriptRow): row is ToolTranscriptRow {
  return isToolRow(row) && !isShellTool(row.item.tool);
}

/**
 * A tool whose row and whose diff rows say the same thing twice. Reuses the
 * reducer's own activity classification rather than minting a second list of
 * the same names — Codex's fileChange / applyPatch, Claude's Edit / Write /
 * MultiEdit / NotebookEdit, and Grok's ACP "edit" all land in it.
 */
export function isFileChangeTool(tool: string): boolean {
  return toolActivityKind(tool) === "edit";
}

export function repeatedToolLabel(tool: string, name: string, count: number, running: boolean): string {
  switch (toolActivityKind(tool)) {
    case "read":
      return `${running ? "Reading" : "Read"} ${count} ${count === 1 ? "file" : "files"}`;
    case "edit":
      return `${running ? "Editing" : "Edited"} ${count} ${count === 1 ? "file" : "files"}`;
    case "search":
      return `${running ? "Searching" : "Searched"} ${count} ${count === 1 ? "time" : "times"}`;
    case "list":
      return `${running ? "Listing" : "Listed"} ${
        count} ${count === 1 ? "directory" : "directories"}`;
    default:
      return running
        ? `Calling ${name} · ${count} ${count === 1 ? "call" : "calls"}`
        : `Called ${name} ${count === 1 ? "once" : `${count} times`}`;
  }
}

/**
 * Fold a consecutive stretch of tools (and the diffs they produced) into one
 * work line, the way Claude Code does. Detailed still keeps each call on its
 * own row. The export name predates mixed work groups.
 */
export function collapseSuccessfulToolRuns(
  rows: readonly TranscriptRow[], minimum = 1,
): ConversationDisplayRow[] {
  if (minimum === Number.POSITIVE_INFINITY) {
    return foldEditedFileRows(mergeFileChangeRows(collapseSameActivityRuns(rows, minimum)));
  }
  // COMPACT (see toolRunMinimum): 0 means no minimum at all, so the two shapes
  // balanced exempts below fold like everything else. Those exemptions are what
  // made compact and balanced the same screen — a lone Read and a stretch of
  // pure edits are most of an ordinary turn, and both were exempt at every step
  // of the dial that folds anything.
  const foldsEveryRun = minimum <= 0;
  const out: ConversationDisplayRow[] = [];
  for (let index = 0; index < rows.length;) {
    const first = rows[index];
    if (!isWorkItemRow(first)) {
      out.push({ kind: "row", row: first });
      index += 1;
      continue;
    }
    // A run must never speak for a call that is still going or that failed —
    // groupFinishedTools' one rule, isFinishedWork. The stretch splits at every
    // such call: only its leading partition of finished work can fold here, and
    // whatever ends that partition goes back through the loop as a row.
    //
    // Only a member that is not finished work ends the fold here. A LONE
    // SUCCESS falls through to everything below, which is where "does a run of
    // one fold?" has always been answered: the density dial and
    // loneInspection, unchanged by this rule.
    if (!isFinishedWork(first.item)) {
      const lead = first;
      out.push({ kind: "row", row: lead });
      index += 1;
      // THE DIFFS THIS CALL PRODUCED LEAVE WITH IT. Every diff is foldable — it
      // is the record of a change that already happened — so a diff left behind
      // here becomes the head of the NEXT partition, and the run built from
      // that partition counts a file no call inside it touched: "edited 2
      // files" over a sentence with no failure in it, while the call that
      // actually made one of those changes stands outside saying "failed".
      // Emitted here, in order, they stay adjacent to their own call and
      // mergeFileChangeRows pairs them exactly as it always did.
      if (isToolRow(lead)) {
        while (index < rows.length) {
          const next = rows[index];
          if (!isDiffItemRow(next) || next.item.callId !== lead.item.id) break;
          out.push({ kind: "row", row: next });
          index += 1;
        }
      }
      continue;
    }
    // The leading partition, scanned once: every consecutive work row that is
    // finished work. It ends at the stretch's end or at the call the next
    // iteration stands up on its own.
    const members: Extract<TranscriptRow, { kind: "item" }>[] = [first];
    let cursor = index + 1;
    while (cursor < rows.length) {
      const next = rows[cursor];
      if (!isWorkItemRow(next) || !isFinishedWork(next.item)) break;
      members.push(next);
      cursor += 1;
    }
    const tools = members.filter(isToolRow);
    const diffs = members.filter(isDiffItemRow).map(row => row.item);
    const kinds = new Set(tools.map(row => {
      const kind = isShellTool(row.item.tool) ? "command"
        : isFileChangeTool(row.item.tool) ? "edit" : "other";
      return kind;
    }));
    const onlyEdits = !foldsEveryRun && tools.length > 0
      && [...kinds].every(kind => kind === "edit");
    const loneInspection = !foldsEveryRun && tools.length === 1
      && !isShellTool(tools[0].item.tool) && diffs.length === 0;
    if (tools.length === 0 || members.length < minimum || onlyEdits || loneInspection) {
      for (const row of members) out.push({ kind: "row", row });
      index = cursor;
      continue;
    }
    const onlyCommands = tools.every(row => isShellTool(row.item.tool));
    out.push({
      kind: "tool-run",
      key: `tool-run:${tools[0].key}`,
      tool: onlyCommands ? "command" : "work",
      rows: tools,
      ...(diffs.length > 0 ? { diffs } : {}),
      members,
    });
    index = cursor;
  }
  return foldEditedFileRows(mergeFileChangeRows(out));
}

/**
 * PURE. Every transcript row key one DRAWN line now stands for.
 *
 * The fold is what makes this necessary: a dozen tool rows can resolve to one
 * line, so anything reasoning about "how much is on screen below here" has to
 * ask the display list, not the transcript. Both merged kinds already carry a
 * `keys` array for the find bar, which needs the same answer for the same
 * reason — a hit on a swallowed row still has to scroll to something.
 *
 * `members` over `rows` on a tool run: `rows` is the tool rows only, `members`
 * is every row the run consumed including the diffs between them, and
 * collapseSameActivityRuns builds runs without one.
 */
export function displayRowKeys(display: ConversationDisplayRow): string[] {
  switch (display.kind) {
    case "row": return [display.row.key];
    case "tool-run": return (display.members ?? display.rows).map(row => row.key);
    case "file-change": return display.keys;
    case "edited-files": return display.keys;
    case "subagent-grid": return display.rows.map(row => row.key);
  }
}

export function isSubagentItemRow(row: TranscriptRow): row is SubagentTranscriptRow {
  return row.kind === "item" && row.item.kind === "subagent";
}

/**
 * Two or more consecutive sibling subagents become one square grid. A lone
 * spawn stays the existing row so a single Task does not turn into a one-cell
 * list, and nested children keep their tree.
 */
export function groupSubagentGrids(
  display: readonly ConversationDisplayRow[],
): ConversationDisplayRow[] {
  const out: ConversationDisplayRow[] = [];
  for (let index = 0; index < display.length;) {
    const first = display[index];
    if (first.kind !== "row" || !isSubagentItemRow(first.row)) {
      out.push(first);
      index += 1;
      continue;
    }
    const rows: SubagentTranscriptRow[] = [first.row];
    let cursor = index + 1;
    while (cursor < display.length) {
      const next = display[cursor];
      if (next.kind !== "row" || !isSubagentItemRow(next.row)) break;
      rows.push(next.row);
      cursor += 1;
    }
    if (rows.length >= 2) {
      out.push({ kind: "subagent-grid", key: `sub-grid:${rows[0].key}`, rows });
    } else {
      out.push(first);
    }
    index = cursor;
  }
  return out;
}

/**
 * PURE. `next`, with every display row that says exactly what its counterpart
 * in `previous` said replaced by that counterpart.
 *
 * The display pass mints a fresh object for every run, file change, edited-files
 * fold and subagent grid on every call, and it is called on every streamed
 * frame. Those objects are the props of memoized rows, so without this every
 * folded run in the pane re-rendered for a token that only touched the tail.
 *
 * "The same" is decided on what the rows are made of, never on a deep compare:
 * transcript rows by key, origin and item IDENTITY (mergeTranscript keeps both
 * stable while an item is unchanged), diff items by identity, and strings by
 * value. Anything else is a new row. The list itself comes back by identity
 * when nothing in it changed.
 */
export function retainDisplayRows(
  previous: readonly ConversationDisplayRow[],
  next: readonly ConversationDisplayRow[],
): readonly ConversationDisplayRow[] {
  if (previous === next || previous.length === 0) return next;
  let byKey: Map<string, ConversationDisplayRow> | undefined;
  let changed = previous.length !== next.length;
  const out = next.map((row, index) => {
    const key = displayRowKey(row);
    // Positions hold while a transcript only grows; the map is for the rest.
    let before: ConversationDisplayRow | undefined = previous[index];
    if (before === undefined || displayRowKey(before) !== key) {
      byKey ??= new Map(previous.map(entry => [displayRowKey(entry), entry]));
      before = byKey.get(key);
    }
    const kept = before !== undefined && sameDisplayRow(before, row) ? before : row;
    if (kept !== previous[index]) changed = true;
    return kept;
  });
  return changed ? out : previous;
}

function displayRowKey(display: ConversationDisplayRow): string {
  return display.kind === "row" ? display.row.key : display.key;
}

function sameTranscriptRow(a: TranscriptRow, b: TranscriptRow): boolean {
  if (a === b) return true;
  if (a.kind === "mark") return b.kind === "mark" && a.key === b.key && a.label === b.label;
  return b.kind === "item" && a.key === b.key && a.origin === b.origin && a.item === b.item;
}

function sameList<T>(a: readonly T[] | undefined, b: readonly T[] | undefined,
    same: (left: T, right: T) => boolean): boolean {
  if (a === b) return true;
  if (a === undefined || b === undefined || a.length !== b.length) return false;
  for (let index = 0; index < a.length; index += 1) {
    if (!same(a[index], b[index])) return false;
  }
  return true;
}

const identical = <T>(a: T, b: T): boolean => a === b;

function sameDisplayRow(a: ConversationDisplayRow, b: ConversationDisplayRow): boolean {
  switch (b.kind) {
    case "row":
      return a.kind === "row" && sameTranscriptRow(a.row, b.row);
    case "tool-run":
      return a.kind === "tool-run" && a.key === b.key && a.tool === b.tool
        && sameList(a.rows, b.rows, sameTranscriptRow)
        && sameList(a.diffs, b.diffs, identical)
        && sameList(a.members, b.members, sameTranscriptRow);
    case "file-change":
      return a.kind === "file-change" && a.key === b.key
        && sameTranscriptRow(a.row, b.row)
        && sameList(a.items, b.items, identical) && sameList(a.keys, b.keys, identical);
    case "edited-files":
      return a.kind === "edited-files" && a.key === b.key
        && sameList(a.items, b.items, identical) && sameList(a.paths, b.paths, identical)
        && sameList(a.keys, b.keys, identical);
    case "subagent-grid":
      return a.kind === "subagent-grid" && a.key === b.key
        && sameList(a.rows, b.rows, sameTranscriptRow);
  }
}

export function collapseSameActivityRuns(
  rows: readonly TranscriptRow[], minimum: number,
): ConversationDisplayRow[] {
  const out: ConversationDisplayRow[] = [];
  for (let index = 0; index < rows.length;) {
    const first = rows[index];
    if (isShellToolRow(first)) {
      const run: ToolTranscriptRow[] = [first];
      let cursor = index + 1;
      while (cursor < rows.length) {
        const next = rows[cursor];
        if (!isShellToolRow(next)) break;
        run.push(next);
        cursor += 1;
      }
      out.push({ kind: "tool-run", key: `tool-run:${first.key}`, tool: "command",
        rows: run });
      index = cursor;
      continue;
    }
    if (!groupableToolRow(first)) {
      out.push({ kind: "row", row: first });
      index += 1;
      continue;
    }
    const groupKey = toolActivityGroupKey(first.item.tool);
    const run: ToolTranscriptRow[] = [first];
    let cursor = index + 1;
    while (cursor < rows.length) {
      const candidate = rows[cursor];
      if (!groupableToolRow(candidate)
        || toolActivityGroupKey(candidate.item.tool) !== groupKey) break;
      run.push(candidate);
      cursor += 1;
    }
    if (run.length >= minimum) {
      out.push({ kind: "tool-run", key: `tool-run:${first.key}`, tool: first.item.tool,
        rows: run });
    } else {
      for (const row of run) out.push({ kind: "row", row });
    }
    index = cursor;
  }
  return out;
}

export function isDiffDisplayRow(
  display: ConversationDisplayRow,
): display is { kind: "row"; row: Extract<TranscriptRow, { kind: "item" }> & { item: DiffItem } } {
  return display.kind === "row" && display.row.kind === "item"
    && display.row.item.kind === "diff";
}

/**
 * Consecutive produced diffs become one control once there are two or more.
 * A lone path pill stays as it is — the fold exists to collapse a run, not
 * to replace a filename with "Edited 1 file".
 */
export function foldEditedFileRows(
  display: readonly ConversationDisplayRow[],
): ConversationDisplayRow[] {
  const out: ConversationDisplayRow[] = [];
  for (let index = 0; index < display.length;) {
    const first = display[index];
    if (!isDiffDisplayRow(first)) {
      out.push(first);
      index += 1;
      continue;
    }
    const items: DiffItem[] = [first.row.item];
    const keys: string[] = [first.row.key];
    let cursor = index + 1;
    while (cursor < display.length) {
      const next = display[cursor];
      if (!isDiffDisplayRow(next)) break;
      items.push(next.row.item);
      keys.push(next.row.key);
      cursor += 1;
    }
    if (items.length >= 2) {
      out.push({
        kind: "edited-files",
        key: `edited:${first.row.key}`,
        items,
        keys,
        // DISTINCT paths, not occurrences. The label counts these, and a patch
        // stream that re-draws one file five times ("Edited 5 files" over one
        // filename) was counting events rather than files. The composer's dirty
        // pill has always de-duped; this is the same rule in the same words.
        paths: [...new Set(items.map(item => item.path))],
      });
    } else {
      out.push(first);
    }
    index = cursor;
  }
  return out;
}

/**
 * ONE FILE CHANGE, ONE ROW.
 *
 * Every engine draws a file edit twice — a tool row, then a diff row per path —
 * and each half carries something the other cannot. The tool row has the VERB
 * (Codex spells the kind only there) and the OUTCOME (a diff row has no state
 * WORD, so a declined or failed patch looks exactly like an applied one). The
 * diff row has the line counts and the way into Monaco. Deleting either half
 * would trade two honest rows for one that lies, so the pair is merged into a
 * single row carrying the union.
 *
 * PAIRED BY callId, NOT BY ADJACENCY. Which call produced a diff is a fact the
 * wire now carries (engine.ts `diff.produced`), and reading it is what stops
 * this pass inventing one. Two ACP edits announced before either resolves
 * arrive as call(a), call(b), diff(a), diff(b); walking forward over "whatever
 * diff rows come next" gave call(b) a row claiming two files, one of which was
 * call(a)'s SUCCESSFUL change, under call(b)'s word "failed". A merge is only
 * worth doing while it cannot say that.
 *
 * The scan still stops at the first row that is not a diff — a call absorbs the
 * diffs it produced from the block right below it, never from further down the
 * transcript. A diff belonging to a different call inside that block is stepped
 * over and left exactly where it was, so it keeps its own row rather than being
 * swallowed or reordered.
 *
 * A tool row with no diffs of its own after it — a MultiEdit, a patch whose diff
 * never arrived, or a call whose diffs are still queued behind another call's —
 * is left exactly as it was: there is no pair to merge.
 */
export function mergeFileChangeRows(
  display: readonly ConversationDisplayRow[],
): ConversationDisplayRow[] {
  const out: ConversationDisplayRow[] = [];
  for (let index = 0; index < display.length;) {
    const first = display[index];
    const row = first.kind === "row" ? first.row : null;
    if (row === null || !isToolRow(row) || !isFileChangeTool(row.item.tool)) {
      out.push(first);
      index += 1;
      continue;
    }
    const items: DiffItem[] = [];
    const keys: string[] = [row.key];
    const others: ConversationDisplayRow[] = [];
    let cursor = index + 1;
    while (cursor < display.length) {
      const next = display[cursor];
      if (!isDiffDisplayRow(next)) break;
      if (next.row.item.callId === row.item.id) {
        items.push(next.row.item);
        keys.push(next.row.key);
      } else {
        others.push(next);
      }
      cursor += 1;
    }
    if (items.length === 0) {
      out.push(first);
      index += 1;
      continue;
    }
    out.push({ kind: "file-change", key: `file-change:${row.key}`, row, items, keys });
    out.push(...others);
    index = cursor;
  }
  return out;
}

/* CONSECUTIVE MERGED FILE CHANGES ARE NO LONGER FOLDED, and the pass that did
   it is gone rather than tightened. It emitted an "Edited N files" control that
   navigates and never reveals its members, so two edits to two files — one
   Claude assistant message with two Edit blocks, the single most common thing
   this pane renders — became one row with no per-file diff to open, no per-file
   counts, no state, and no find anchor for either call. Its densest measured
   case was already the merge's win, not the fold's: the "no repeated path" rule
   is all-or-nothing over the maximal run, so the photographed sequence folded
   nothing. It bought a row and cost two files' worth of reachability.

   foldEditedFileRows below is a DIFFERENT fold — orphan diff rows with no tool
   row of their own — and it stays. It carries its members' keys for exactly the
   reason this one could not. */
