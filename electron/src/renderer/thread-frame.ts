import { approvalVerdictKey, type ApprovalVerdict, type ApprovalVerdicts } from "./approval-keys";
import type { ConvItem } from "./conversation-reducer";
import { lineDelta, type DiffStat } from "./diff-stats";
import { displayPath } from "./display-paths";
import { threadStepWords } from "./thread-live";
import { isCommandActivity, pathFromToolInput, toolActivityKind, toolActivityName }
  from "./tool-activity";

/**
 * THE FRAME AROUND A THREADS PANE'S LIVE TURN (owner, 2026-10-07): the
 * approval docked above the composer, the composer's running and waiting
 * states, and the head's Changes cue. Pure and total, so the words, the order
 * and the bounds are tested without a DOM; the components only draw them.
 */

export type ThreadApproval = Extract<ConvItem, { kind: "approval" }>;

/** What a docked request asks for, which decides its title and its body. */
export type ThreadDockKind = "command" | "edit" | "write" | "other";

const toolKey = (tool: string): string => tool.trim().toLowerCase().replace(/[\s_-]+/gu, "");

function record(input: unknown): Record<string, unknown> {
  return typeof input === "object" && input !== null && !Array.isArray(input)
    ? input as Record<string, unknown> : {};
}

const text = (value: unknown): string => (typeof value === "string" ? value : "");

/**
 * Codex names its shell approvals `command` and its patch approvals
 * `fileChange`; Claude's are Bash, Edit, MultiEdit and Write. Write is told
 * apart from the other edits because it replaces a whole file.
 */
export function threadDockKind(tool: string): ThreadDockKind {
  const name = toolKey(tool);
  if (name === "write" || name === "writefile") return "write";
  if (name === "command" || isCommandActivity(tool)) return "command";
  if (toolActivityKind(tool) === "edit") return "edit";
  return "other";
}

/** A shell word as a shell would read it back: bare when safe, else single-quoted. */
function shellWord(word: string): string {
  return /^[\w@%+=:,./-]+$/u.test(word) ? word : `'${word.replace(/'/gu, "'\\''")}'`;
}

/**
 * The command a shell approval would run, whole: a string as sent, an argv
 * array as shell words. Null when the request carries none (Codex's stdin
 * writes), and the body then shows the request's own input.
 */
export function threadDockCommand(input: unknown): string | null {
  const rec = record(input);
  const command = rec.command ?? rec.cmd;
  if (typeof command === "string" && command.trim() !== "") return command.trim();
  if (Array.isArray(command) && command.length > 0
      && command.every(word => typeof word === "string")) {
    return (command as string[]).map(shellWord).join(" ");
  }
  return null;
}

/** The paths a request names: Claude's one file, or Codex's stashed list. */
function requestPaths(input: unknown): string[] {
  const single = pathFromToolInput(input);
  if (single !== undefined) return [single];
  const listed = record(input).interfacePaths;
  return Array.isArray(listed)
    ? [...new Set(listed.filter((path): path is string => typeof path === "string" && path !== ""))]
    : [];
}

/** Claude's old/new pairs: Edit's one, MultiEdit's list. */
function editPairs(input: unknown): { before: string; after: string }[] {
  const rec = record(input);
  if (Array.isArray(rec.edits)) {
    return rec.edits.map(edit => ({ before: text(record(edit).old_string),
      after: text(record(edit).new_string) }));
  }
  return "old_string" in rec || "new_string" in rec
    ? [{ before: text(rec.old_string), after: text(rec.new_string) }] : [];
}

/**
 * What the request would change, counted the way the pane counts a diff
 * (lineDelta). An edit's pairs are exact; a Write's removed lines are not
 * known (it may replace a file), so only its added lines are said. Unknown
 * is absent, never a zero.
 */
function requestStat(input: unknown, kind: ThreadDockKind): Partial<DiffStat> | undefined {
  if (kind === "write") {
    const content = text(record(input).content);
    return content === "" ? undefined : { added: lineDelta("", content).added };
  }
  const pairs = editPairs(input);
  if (pairs.length === 0) return undefined;
  return pairs.reduce((sum, pair) => {
    const delta = lineDelta(pair.before, pair.after);
    return { added: (sum.added ?? 0) + delta.added, removed: (sum.removed ?? 0) + delta.removed };
  }, { added: 0, removed: 0 } as Partial<DiffStat>);
}

export interface ThreadDockFile {
  /** The path as the engine sent it, for the row's title. */
  path: string;
  /** Project-relative, in the live turn's own words (threadStepWords). */
  shown: string;
  /** +N −M for an edit, +N for a Write; absent where it is not known. */
  stat?: Partial<DiffStat>;
}

/** The file rows of an edit or a Write: what it touches and by how much. */
export function threadDockFiles(item: Pick<ThreadApproval, "tool" | "summary" | "input">,
    root?: string): ThreadDockFile[] {
  const kind = threadDockKind(item.tool);
  const paths = requestPaths(item.input);
  if (paths.length === 1) {
    const words = threadStepWords(item, "waiting", root);
    return [{ path: paths[0], shown: words.target !== "" ? words.target : displayPath(paths[0], root),
      stat: requestStat(item.input, kind) }];
  }
  return paths.map(path => ({ path, shown: displayPath(path, root) }));
}

export interface ThreadDockLine { sign: "+" | "−"; text: string }
export interface ThreadDockDiff { lines: ThreadDockLine[]; more: number }

/** The most lines a docked change previews; "+N more lines" says the rest. */
export const THREAD_DOCK_DIFF_LINES = 12;

function lines(value: string): string[] {
  if (value === "") return [];
  const split = value.split(/\r?\n/u);
  if (split.length > 1 && split[split.length - 1] === "") split.pop();
  return split;
}

/**
 * A small diff of what an edit asks to change: the lines it removes, then the
 * lines it adds, with the context the two share at either end left out. A
 * Write previews the start of the file it writes. Null when there is nothing
 * to preview (Codex's patch approvals carry paths only).
 */
export function threadDockDiff(input: unknown, kind: ThreadDockKind,
    limit = THREAD_DOCK_DIFF_LINES): ThreadDockDiff | null {
  const all: ThreadDockLine[] = [];
  if (kind === "write") {
    for (const line of lines(text(record(input).content))) all.push({ sign: "+", text: line });
  } else if (kind === "edit") {
    for (const pair of editPairs(input)) {
      const before = lines(pair.before);
      const after = lines(pair.after);
      let head = 0;
      while (head < before.length && head < after.length && before[head] === after[head]) head += 1;
      let tail = 0;
      while (tail < before.length - head && tail < after.length - head
        && before[before.length - 1 - tail] === after[after.length - 1 - tail]) tail += 1;
      for (const line of before.slice(head, before.length - tail)) all.push({ sign: "−", text: line });
      for (const line of after.slice(head, after.length - tail)) all.push({ sign: "+", text: line });
    }
  }
  if (all.length === 0) return null;
  return { lines: all.slice(0, limit), more: Math.max(0, all.length - limit) };
}

/** "Web fetch" → "web fetch"; an acronym ("MCP …") keeps its capitals. */
function sentenceCase(name: string): string {
  return /^[A-Z][a-z]/u.test(name) ? name.charAt(0).toLowerCase() + name.slice(1) : name;
}

/** The docked card's question: what answering Allow would let the agent do. */
export function threadDockTitle(item: Pick<ThreadApproval, "tool" | "input">): string {
  switch (threadDockKind(item.tool)) {
    case "command": return "Run a command?";
    case "write": return "Write a file?";
    case "edit": {
      const count = requestPaths(item.input).length;
      return count > 1 ? `Edit ${count} files?` : "Edit a file?";
    }
    case "other": return `Allow ${sentenceCase(toolActivityName(item.tool))}?`;
  }
}

/** A docked question card's title, by how many questions it holds. */
export function threadDockQuestionTitle(count: number): string {
  return count > 1 ? `Answer ${count} questions?` : "Answer a question?";
}

/** How long a request has waited: "0:42", "12:05", "1:02:03". */
export function threadWaitingClock(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = String(total % 60).padStart(2, "0");
  return hours > 0 ? `${hours}:${String(minutes).padStart(2, "0")}:${seconds}`
    : `${minutes}:${seconds}`;
}

/** The docked card's footnote: "in website · waiting 0:42", or the clock alone. */
export function threadDockMeta(project: string | undefined, ms: number): string {
  const clock = `waiting ${threadWaitingClock(ms)}`;
  const name = project?.trim() ?? "";
  return name === "" ? clock : `in ${name} · ${clock}`;
}

/**
 * Every request this pane holds open, in transcript order, subagents' own
 * included (they draw inside their parent, so a nested request comes before
 * any top-level row after it — the order keyedApprovalId walks). The dock
 * draws exactly these, and the transcript draws none of them.
 */
export function threadDockApprovals(items: Iterable<ConvItem>,
    active: ReadonlySet<string>): ThreadApproval[] {
  if (active.size === 0) return [];
  const found: ThreadApproval[] = [];
  const seen = new Set<string>();
  const walk = (list: Iterable<ConvItem>): void => {
    for (const item of list) {
      if (item.kind === "subagent") walk(item.children);
      else if (item.kind === "approval" && active.has(item.id) && !seen.has(item.id)) {
        seen.add(item.id);
        found.push(item);
      }
    }
  };
  walk(items);
  return found;
}

/** The verdict this window sent and that landed; a failed send is none. */
export function threadApprovalSent(item: Pick<ThreadApproval, "resolved">,
    answer: ApprovalVerdict | undefined): "allow" | "deny" | undefined {
  return answer?.error === undefined ? answer?.verdict ?? item.resolved : undefined;
}

/**
 * The pane is blocked on its owner: a docked request the owner may answer
 * (not one the approval policy is still checking) has no verdict yet. The
 * composer turns amber on exactly this, and nothing merely pending.
 */
export function threadApprovalPending(dock: readonly ThreadApproval[],
    owner: ReadonlySet<string>, approvals: ApprovalVerdicts, sessionKey: string): boolean {
  return dock.some(item => owner.has(item.id)
    && threadApprovalSent(item, approvals[approvalVerdictKey(sessionKey, item.id)]) === undefined);
}

/** How the head's Changes chip moves: pop when its totals arrive, flash when they change. */
export type ThreadChangesCue = "" | "pop" | "flash-a" | "flash-b";

/**
 * The next cue from the totals the chip printed before and prints now
 * (`undefined` before = the pane's first look, which moves nothing; `null` =
 * not known). A change alternates two names so the flash replays each time.
 */
export function threadChangesCue(before: string | null | undefined, now: string | null,
    cue: ThreadChangesCue): ThreadChangesCue {
  if (before === undefined || now === null || now === before) return cue;
  if (before === null) return "pop";
  return cue === "flash-a" ? "flash-b" : "flash-a";
}
