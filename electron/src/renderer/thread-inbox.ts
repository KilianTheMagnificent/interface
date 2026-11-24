import { sessionKey } from "../shared/engine";
import type { EngineId, SessionKey, SessionStatus, SessionSummary } from "../shared/engine";
import type {
  PullRequestState, ThreadCommand, ThreadSection, ThreadSettings, ThreadTurn, ThreadView, ThreadsView,
  WorkspaceKept,
} from "../shared/threads";
import { searchesMessages, type JournalHit } from "../shared/thread-journal";
import { DONE_WORD, statusWord } from "./components/status-words";
import { diffStatLabel } from "./diff-stats";
import type { ThreadLiveFacts } from "./session-activity";
import {
  managedWorktreeName, projectNameOf, queryTerms, sessionAgeLabel, sessionSurfaceOf, type SidebarRow,
} from "./sidebar-view";
import { THREAD_THINKING, threadStepLine, threadStepWords } from "./thread-live";
import { threadDisplayTitle } from "./thread-title";

/**
 * The Threads inbox's view model: which threads the Threads sidebar lists, in
 * which section and order, with which one status mark, under which project
 * scope; plus the commands its menu, drags and Undo send.
 *
 * Rust owns every decision about a thread (section, unread, woke, order keys,
 * auto-settle); this module only reads the `ThreadsView` it pushes and joins it
 * to the session listing. A session Rust has no record for yet is drawn Active,
 * created at its `firstSeen`, never unread — the honest reading of "nothing is
 * known about it" — but only until Rust has answered, or for a few minutes
 * after it first appeared: Rust records every thread it means to list and
 * deliberately leaves out hidden worker sessions.
 */

/**
 * The sections, in the order the inbox draws them. From terminals (review
 * C1) is last: sessions a Code pane or the CLI started, folded by default.
 */
export const THREAD_SECTIONS: readonly ThreadSection[] =
  ["pinned", "active", "snoozed", "settled", "fromTerminals"];

export const THREAD_SECTION_LABEL: Readonly<Record<ThreadSection, string>> = {
  pinned: "Pinned",
  active: "Active",
  snoozed: "Snoozed",
  settled: "Settled",
  fromTerminals: "From terminals",
};

/** The section a row is drawn under: Rust's, From terminals included. */
export function threadDrawnSection(section: ThreadSection): ThreadSection {
  return section;
}

/** Settled lists its newest threads a page at a time. */
export const SETTLED_PAGE_SIZE = 50;

/* ---- project scope -------------------------------------------------------
   The inbox is flat across projects; the scope picker narrows it to one
   project or to plain chats. A renderer preference, stored as one string. */

/** "all", "none" (plain chats) or `project:<root>`. */
export type ThreadScope = string;
export const ALL_THREAD_PROJECTS: ThreadScope = "all";
export const NO_THREAD_PROJECT: ThreadScope = "none";
/** What the scope picker and a plain chat's row call "no project". */
export const NO_PROJECT_LABEL = "No project";
const PROJECT_SCOPE = "project:";

export function threadScopeFor(root: string | null): ThreadScope {
  return root === null ? NO_THREAD_PROJECT : `${PROJECT_SCOPE}${root}`;
}

/** Anything unreadable is "All projects": a stale value never empties the inbox silently. */
export function parseThreadScope(raw: string | null | undefined): ThreadScope {
  if (raw === NO_THREAD_PROJECT) return NO_THREAD_PROJECT;
  if (typeof raw === "string" && raw.startsWith(PROJECT_SCOPE) && raw.length > PROJECT_SCOPE.length) {
    return raw;
  }
  return ALL_THREAD_PROJECTS;
}

/** undefined for "All projects", null for "No project", else the project root. */
export function threadScopeRoot(scope: ThreadScope): string | null | undefined {
  if (scope === NO_THREAD_PROJECT) return null;
  return scope.startsWith(PROJECT_SCOPE) ? scope.slice(PROJECT_SCOPE.length) : undefined;
}

// The one rewrite a project root takes, the same one the session sidebar makes:
// `/a/web/` and `/a/web` are one project.
function normalizeRoot(root: string): string {
  let end = root.length;
  while (end > 1 && root[end - 1] === "/") end--;
  return root.slice(0, end);
}

/**
 * The project a thread belongs to, or null for "No project". The thread's own
 * workspace wins: a worktree thread's session runs inside the worktree, whose
 * git root is not the project the owner picked. Without a record, the
 * session's repository root is the best fact there is.
 */
export function threadProjectRoot(session: Pick<SessionSummary, "projectRoot">,
    thread?: Pick<ThreadView, "workspace">): string | null {
  const workspace = thread?.workspace;
  if (workspace !== undefined) {
    return workspace.mode === "none" || workspace.projectRoot.trim() === ""
      ? null : normalizeRoot(workspace.projectRoot);
  }
  const root = session.projectRoot;
  return root === undefined || root.trim() === "" ? null : normalizeRoot(root);
}

/* ---- the status mark -----------------------------------------------------
   One mark per row, highest wins. Needs-you and working are live facts from
   the session listing; failed is live too, or a failed run nobody has opened;
   unread and woke are Rust's. */

export type ThreadMark = "needs-you" | "working" | "failed" | "unread" | "woke" | "none";

/** Highest first. */
export const THREAD_MARK_LADDER: readonly ThreadMark[] =
  ["needs-you", "working", "failed", "unread", "woke", "none"];

export function threadMark(status: SessionStatus, needsYou: boolean,
    thread?: Pick<ThreadView, "unread" | "woke" | "lastRun">): ThreadMark {
  if (needsYou || status === "waiting") return "needs-you";
  if (status === "working") return "working";
  if (status === "failed"
    || (thread?.unread === true && thread.lastRun?.outcome === "failed")) return "failed";
  if (thread?.unread === true) return "unread";
  if (thread?.woke === true) return "woke";
  return "none";
}

/** The mark's words, for its accessible name and tooltip. "" for none. */
export function threadMarkWord(mark: ThreadMark): string {
  switch (mark) {
    case "needs-you": return statusWord("waiting");
    case "working": return statusWord("working");
    case "failed": return statusWord("failed");
    case "unread": return DONE_WORD;
    case "woke": return "Back from snooze";
    case "none": return "";
  }
}

/** The highest mark among rows: what a collapsed section's header shows. */
export function highestThreadMark(marks: Iterable<ThreadMark>): ThreadMark {
  let best = THREAD_MARK_LADDER.length - 1;
  for (const mark of marks) best = Math.min(best, THREAD_MARK_LADDER.indexOf(mark));
  return THREAD_MARK_LADDER[best];
}

/* ---- the model ----------------------------------------------------------- */

/** Lines a thread's turns added and removed, over the turns that reported both. */
export interface ThreadDiffTotals {
  readonly added: number;
  readonly removed: number;
}

/** What the row says about the thread's pull request. */
export interface ThreadRowPullRequest {
  readonly number: number;
  readonly state: PullRequestState;
}

export interface ThreadInboxRow {
  readonly key: SessionKey;
  /** The thread's title as stored: what Rename starts from and the tooltip shows. */
  readonly title: string;
  /** What the row prints (`threadDisplayTitle`): a prompt title shortened to one line. */
  readonly displayTitle: string;
  readonly engine: EngineId;
  readonly status: SessionStatus;
  /** null is "No project". */
  readonly projectRoot: string | null;
  /** The project's name; "" for No project. */
  readonly projectName: string;
  /** The second line's project words: the name, or "No project". */
  readonly projectLabel: string;
  /** The thread's branch when Rust knows it (`workspace.branch`); never for No project. */
  readonly branch?: string;
  /** Summed over the turns that reported counts; absent when none did (unknown is never zero). */
  readonly diff?: ThreadDiffTotals;
  readonly pullRequest?: ThreadRowPullRequest;
  /** The row's age source, epoch ms (`threadActivityAt`); 0 when nothing is known. */
  readonly activityAt: number;
  /** The open run's start, for the working mark's elapsed time. */
  readonly runStartedAt?: number;
  /**
   * Where the session runs (its repository root, else its cwd): what a live
   * step's paths are shown relative to. A worktree thread's is the worktree.
   */
  readonly sessionRoot: string;
  readonly section: ThreadSection;
  readonly mark: ThreadMark;
  readonly selected: boolean;
  /** Rust has a record for this thread. */
  readonly known: boolean;
  /** Pinned, even while snoozed (snoozing keeps the pin). */
  readonly pinned: boolean;
  readonly unread: boolean;
  readonly autoSettle: boolean;
  readonly createdAt: number;
  readonly snoozedUntil?: number;
  readonly settledAt?: number;
  /** For the hover card, the menu and the lifecycle actions. */
  readonly lastActivity: string;
  readonly external: boolean;
  readonly cloud: boolean;
  readonly worktree?: string;
  /** An Away loop runs (or ran) as this thread (protocol 44): the row says so. */
  readonly away?: true;
}

export interface ThreadInboxSection {
  readonly section: ThreadSection;
  readonly label: string;
  /** Rust's order: what Move up / Move down, a drop and Undo place a thread against. */
  readonly rows: readonly ThreadInboxRow[];
  /**
   * The order the rows are drawn in: Rust's, except that a row that needs you
   * floats to the top of Pinned and Active (owner, 2026-10-07). Display only;
   * the same rows as `rows`.
   */
  readonly drawn: readonly ThreadInboxRow[];
  /** What the header shows while the section is folded. */
  readonly mark: ThreadMark;
}

export interface ThreadInboxProject {
  readonly root: string;
  readonly name: string;
}

export interface ThreadInboxModel {
  /** Only sections with rows, in THREAD_SECTIONS order. */
  readonly sections: readonly ThreadInboxSection[];
  /** Rows across every section. */
  readonly total: number;
  /** Rows the search kept before the scope narrowed them, for the empty-state copy. */
  readonly unscoped: number;
  readonly scope: ThreadScope;
  /** Every project with a thread (the scope picker's list), the scoped one included. */
  readonly projects: readonly ThreadInboxProject[];
  /** Some listed thread is a plain chat. */
  readonly hasNoProject: boolean;
  /** A search is narrowing the inbox: folded sections open and Settled is not paged. */
  readonly searching: boolean;
  /**
   * Threads whose messages match the search and whose row did not (protocol
   * 56), in the journal's order: ordinary rows with the match's excerpt in
   * the project's place.
   */
  readonly inMessages: readonly ThreadInboxRow[];
  /** Rust's records by key, for menus and Undo. */
  readonly records: ReadonlyMap<SessionKey, ThreadView>;
  readonly settings: ThreadSettings | null;
}

export interface ThreadInboxInput {
  /** The Threads surface's sessions after the existing filters, before search. */
  readonly sessions: readonly SessionSummary[];
  readonly view: ThreadsView | null;
  readonly scope: ThreadScope;
  readonly query: string;
  readonly selectedKey: SessionKey | null;
  /** Sessions blocked on the owner: an approval or a question. */
  readonly needsYou?: ReadonlySet<SessionKey>;
  /** The clock for the unrecorded-session grace; tests pin it. */
  readonly now?: number;
  /** The thread journal's matches for `query` (newest activity first). */
  readonly messageHits?: readonly JournalHit[];
}

/** The caption over threads found by their messages. */
export const IN_MESSAGES_LABEL = "In messages";

/**
 * The first excerpt per thread from the journal's hits, when `query` is long
 * enough to search messages; Rust's order (newest activity first) is kept.
 */
export function threadMessageExcerpts(query: string, hits: readonly JournalHit[] | undefined):
    ReadonlyMap<SessionKey, string> {
  const excerpts = new Map<SessionKey, string>();
  if (!searchesMessages(query)) return excerpts;
  for (const hit of hits ?? []) {
    if (!excerpts.has(hit.key)) excerpts.set(hit.key, hit.excerpt);
  }
  return excerpts;
}

/**
 * How long a session Rust has no record for still shows once Rust has
 * answered: the moments between a new session reaching the registry and the
 * push that records it. Older ones are sessions Rust chose not to list.
 */
export const UNRECORDED_THREAD_GRACE_MS = 5 * 60_000;

function firstSeenMs(session: SessionSummary): number {
  const at = Date.parse(session.firstSeen);
  return Number.isFinite(at) ? at : 0;
}

/** Fractional-index keys compare by code unit (base-62 is ASCII-ordered); keyed rows first. */
function compareOrderKeys(a: string | undefined, b: string | undefined): number {
  if (a !== undefined && b !== undefined) return a < b ? -1 : a > b ? 1 : 0;
  if (a !== undefined) return -1;
  if (b !== undefined) return 1;
  return 0;
}

function newestFirst(a: number | undefined, b: number | undefined): number {
  return (b ?? 0) - (a ?? 0);
}

function byKey(a: ThreadInboxRow, b: ThreadInboxRow): number {
  return a.key < b.key ? -1 : a.key > b.key ? 1 : 0;
}

/**
 * Each section's order. Pinned and Active follow Rust's order keys, rows
 * without one after the keyed rows, newest first (activity never reorders).
 * Snoozed wakes soonest first; Settled is newest settled first.
 */
function sectionComparator(section: ThreadSection,
    records: ReadonlyMap<SessionKey, ThreadView>): (a: ThreadInboxRow, b: ThreadInboxRow) => number {
  const record = (row: ThreadInboxRow): ThreadView | undefined => records.get(row.key);
  switch (section) {
    case "pinned":
      return (a, b) => compareOrderKeys(record(a)?.pinOrder, record(b)?.pinOrder)
        || newestFirst(record(a)?.pinnedAt, record(b)?.pinnedAt)
        || newestFirst(a.createdAt, b.createdAt) || byKey(a, b);
    case "active":
      return (a, b) => compareOrderKeys(record(a)?.activeOrder, record(b)?.activeOrder)
        || newestFirst(a.createdAt, b.createdAt) || byKey(a, b);
    case "snoozed":
      return (a, b) => (a.snoozedUntil ?? 0) - (b.snoozedUntil ?? 0)
        || newestFirst(a.createdAt, b.createdAt) || byKey(a, b);
    case "settled":
    case "fromTerminals":
      return (a, b) => newestFirst(a.settledAt ?? a.createdAt, b.settledAt ?? b.createdAt)
        || newestFirst(a.createdAt, b.createdAt) || byKey(a, b);
  }
}

/**
 * Whether a session satisfies every search term. The session sidebar's own
 * haystack (title, engine, cwd, repository root) plus the thread's project: a
 * worktree thread runs in a directory that does not carry the project's name.
 */
function threadMatches(session: SessionSummary, projectRoot: string | null,
    terms: readonly string[]): boolean {
  if (terms.length === 0) return true;
  const hay = `${session.title}\n${session.engine}\n${session.cwd}\n${session.projectRoot ?? ""}\n${projectRoot ?? ""}`
    .toLowerCase();
  return terms.every(term => hay.includes(term));
}

/**
 * A thread's lines added and removed, summed over the turns that reported
 * both counts. undefined when no turn did: a thread whose turns are all
 * unmeasured has no diff to show, which is not the same as "+0 −0".
 */
export function threadDiffTotals(turns: readonly Pick<ThreadTurn, "added" | "removed">[]):
    ThreadDiffTotals | undefined {
  let added = 0;
  let removed = 0;
  let known = false;
  for (const turn of turns) {
    if (typeof turn.added !== "number" || typeof turn.removed !== "number") continue;
    known = true;
    added += turn.added;
    removed += turn.removed;
  }
  return known ? { added, removed } : undefined;
}

/**
 * The whole thread's totals, for its inbox row and its pane head's Changes
 * button. A switched thread (protocol 59) adds every segment's own totals,
 * which Rust sums by the same rule (protocol 69; the head's segment is its
 * own turns), so the count matches the Changes panel's Whole thread; any
 * other thread totals its turns.
 */
export function threadLineTotals(thread: Pick<ThreadView, "turns" | "chain">): ThreadDiffTotals | undefined {
  return threadDiffTotals(thread.chain?.segments ?? thread.turns);
}

/**
 * When a thread last moved, for its row's age: the session's `lastActivity`
 * or the end of the thread's last run, whichever is newer. `lastActivity` is
 * the registry's own clock for the transcript, so it covers turns Rust never
 * saw as a run (a CLI resume, an import); a run's end covers a transcript the
 * registry has not rescanned since the run finished. A run still going has no
 * end and leaves `lastActivity` to speak. Neither known: when the thread was
 * created, and 0 (drawn as nothing) when even that is not.
 */
export function threadActivityAt(session: Pick<SessionSummary, "lastActivity">,
    thread: Pick<ThreadView, "lastRun"> | undefined, createdAt: number): number {
  const activity = Date.parse(session.lastActivity);
  const ended = thread?.lastRun?.endedAt;
  const candidates = [Number.isFinite(activity) ? activity : undefined, ended]
    .filter((at): at is number => at !== undefined && at > 0);
  return candidates.length > 0 ? Math.max(...candidates) : Math.max(0, createdAt);
}

/**
 * A section's drawn order: a row that needs you floats to the top, every other
 * row keeps Rust's order (owner, 2026-10-07, "Inbox under load"). A working
 * row never moves: a row the owner watched finish must not jump away. Stable,
 * and the same array when nothing floats.
 */
export function threadNeedsYouFirst(rows: readonly ThreadInboxRow[]): readonly ThreadInboxRow[] {
  const first = rows.filter(row => row.mark === "needs-you");
  if (first.length === 0 || first.every((row, at) => rows[at] === row)) return rows;
  return [...first, ...rows.filter(row => row.mark !== "needs-you")];
}

/**
 * A thread found by its messages, drawn as an ordinary (Active) row: the
 * excerpt takes the project's place on line two, and the branch, pull
 * request and diff make room for it.
 */
function messageRow(row: ThreadInboxRow, excerpt: string): ThreadInboxRow {
  const { branch: _branch, diff: _diff, pullRequest: _pullRequest, ...rest } = row;
  return { ...rest, section: "active", projectLabel: excerpt };
}

/**
 * Characters an In messages excerpt keeps before its match: line two shows
 * only about fourteen beside the project dot and the provider mark.
 */
const EXCERPT_LEAD = 6;

/**
 * `excerpt` starting at most {@link EXCERPT_LEAD} characters (whole words)
 * before `query`'s first case-insensitive match, so the matching text is on
 * the line. Rust's excerpt keeps about 40 characters before the match, which
 * a sidebar row cuts off. Unchanged when the match is near the start or
 * cannot be found.
 */
export function excerptNearMatch(excerpt: string, query: string): string {
  const needle = query.trim().split(/\s+/u).join(" ").toLowerCase();
  const lower = excerpt.toLowerCase();
  // Lower-casing that changes length would move the match: keep it whole.
  if (needle === "" || lower.length !== excerpt.length) return excerpt;
  const at = lower.indexOf(needle);
  if (at <= EXCERPT_LEAD) return excerpt;
  const words = excerpt.slice(0, at).split(/(?<=\s)/u);
  let lead = "";
  while (words.length > 0 && lead.length + words[words.length - 1].length <= EXCERPT_LEAD) {
    lead = words.pop() + lead;
  }
  return `…${lead}${excerpt.slice(at)}`;
}

export function buildThreadInbox(input: ThreadInboxInput): ThreadInboxModel {
  const records = new Map<SessionKey, ThreadView>(
    (input.view?.threads ?? []).map(thread => [thread.key, thread]));
  const terms = queryTerms(input.query);
  const scopeRoot = threadScopeRoot(input.scope);
  const projects = new Map<string, string>();
  let hasNoProject = false;
  let unscoped = 0;
  const bySection = new Map<ThreadSection, ThreadInboxRow[]>(THREAD_SECTIONS.map(section => [section, []]));
  const now = input.now ?? Date.now();
  const excerpts = terms.length === 0 ? new Map<SessionKey, string>()
    : threadMessageExcerpts(input.query, input.messageHits);
  const found = new Map<SessionKey, ThreadInboxRow>();
  // A switched thread's earlier sessions (protocol 59) are part of its row.
  const segments = new Set<SessionKey>((input.view?.threads ?? []).flatMap(thread =>
    (thread.chain?.segments ?? []).map(segment => segment.key).filter(key => key !== thread.key)));

  for (const session of input.sessions) {
    // Agent tasks belong to a teammate, never to the owner's inbox.
    if (sessionSurfaceOf(session) !== "chat") continue;
    const key = sessionKey(session.engine, session.sessionId);
    if (segments.has(key)) continue;
    const thread = records.get(key);
    // Once Rust has answered, a session it has no record for is one it left
    // out (a hidden worker), unless it is new enough to be still on its way.
    if (input.view !== null && thread === undefined
      && now - firstSeenMs(session) > UNRECORDED_THREAD_GRACE_MS) continue;
    const projectRoot = threadProjectRoot(session, thread);
    if (projectRoot === null) hasNoProject = true;
    else if (!projects.has(projectRoot)) projects.set(projectRoot, projectNameOf(projectRoot));
    const matched = threadMatches(session, projectRoot, terms);
    const excerpt = matched ? undefined : excerpts.get(key);
    if (!matched && excerpt === undefined) continue;
    if (matched) unscoped++;
    if (scopeRoot !== undefined && scopeRoot !== projectRoot) continue;
    const worktree = managedWorktreeName(session);
    const section = threadDrawnSection(thread?.section ?? "active");
    const projectName = projectRoot === null ? "" : projectNameOf(projectRoot);
    const createdAt = thread?.createdAt ?? firstSeenMs(session);
    const branch = projectRoot === null ? "" : thread?.workspace?.branch?.trim() ?? "";
    const diff = thread === undefined ? undefined : threadLineTotals(thread);
    const pullRequest = thread?.pullRequest;
    const run = thread?.lastRun;
    const row: ThreadInboxRow = {
      key, title: session.title, engine: session.engine, status: session.status,
      displayTitle: threadDisplayTitle(session.title, thread?.titleSource, session.titleFromSummary),
      projectRoot, projectName, projectLabel: projectRoot === null ? NO_PROJECT_LABEL : projectName,
      ...(branch === "" ? {} : { branch }),
      ...(diff === undefined ? {} : { diff }),
      ...(pullRequest === undefined ? {} : { pullRequest: { number: pullRequest.number, state: pullRequest.state } }),
      activityAt: threadActivityAt(session, thread, createdAt),
      ...(run === undefined || run.endedAt !== undefined ? {} : { runStartedAt: run.startedAt }),
      sessionRoot: session.projectRoot === undefined || session.projectRoot.trim() === ""
        ? session.cwd : session.projectRoot,
      section,
      mark: threadMark(session.status, input.needsYou?.has(key) === true, thread),
      selected: key === input.selectedKey,
      known: thread !== undefined,
      pinned: thread?.pinnedAt !== undefined,
      unread: thread?.unread === true,
      autoSettle: thread?.autoSettle ?? true,
      createdAt,
      ...(thread?.snoozedUntil === undefined ? {} : { snoozedUntil: thread.snoozedUntil }),
      ...(thread?.settledAt === undefined ? {} : { settledAt: thread.settledAt }),
      lastActivity: session.lastActivity,
      external: session.origin !== "app",
      cloud: session.cloud,
      ...(worktree === undefined ? {} : { worktree }),
      ...(thread?.away === undefined ? {} : { away: true as const }),
    };
    if (excerpt !== undefined) {
      found.set(key, messageRow(row, excerptNearMatch(excerpt, input.query)));
      continue;
    }
    bySection.get(section)!.push(row);
  }

  if (scopeRoot !== undefined && scopeRoot !== null && !projects.has(scopeRoot)) {
    projects.set(scopeRoot, projectNameOf(scopeRoot));
  }
  const sections: ThreadInboxSection[] = [];
  let total = 0;
  for (const section of THREAD_SECTIONS) {
    const rows = bySection.get(section)!;
    if (rows.length === 0) continue;
    rows.sort(sectionComparator(section, records));
    total += rows.length;
    sections.push({
      section, label: THREAD_SECTION_LABEL[section], rows,
      drawn: section === "pinned" || section === "active" ? threadNeedsYouFirst(rows) : rows,
      mark: highestThreadMark(rows.map(row => row.mark)),
    });
  }
  return {
    sections, total, unscoped, scope: input.scope,
    projects: [...projects].map(([root, name]) => ({ root, name }))
      .sort((a, b) => a.name.localeCompare(b.name) || (a.root < b.root ? -1 : a.root > b.root ? 1 : 0)),
    hasNoProject: hasNoProject || scopeRoot === null,
    searching: terms.length > 0,
    inMessages: [...excerpts.keys()].flatMap(key => found.get(key) ?? []),
    records,
    settings: input.view?.settings ?? null,
  };
}

/**
 * The sessions the Threads listing treats as already revealed: the session
 * sidebar's own set, plus every thread started here that Rust keeps a record
 * of. The sidebar's declutter rule hides small idle transcripts, but a thread
 * the owner pinned, snoozed or left unread is not clutter however short it
 * is, and Settled is where an idle one goes. Only app-started sessions are
 * added, so the "not started here" rule still keeps provider history out.
 */
export function threadListingRevealed(revealed: ReadonlySet<SessionKey>,
    sessions: readonly SessionSummary[], view: ThreadsView | null): ReadonlySet<SessionKey> {
  if (view === null || view.threads.length === 0) return revealed;
  const recorded = new Set(view.threads.map(thread => thread.key));
  let next: Set<SessionKey> | null = null;
  for (const session of sessions) {
    if (session.origin !== "app") continue;
    const key = sessionKey(session.engine, session.sessionId);
    if (!recorded.has(key) || revealed.has(key)) continue;
    next ??= new Set(revealed);
    next.add(key);
  }
  return next ?? revealed;
}

/** The session-row shape the hover card and the lifecycle confirmations read. */
export function threadSidebarRow(row: ThreadInboxRow): SidebarRow {
  return {
    key: row.key, title: row.title, engine: row.engine, status: row.status, badge: row.status,
    cloud: row.cloud, selected: row.selected, lastActivity: row.lastActivity, external: row.external,
    ...(row.worktree === undefined ? {} : { worktree: row.worktree }),
  };
}

/** The scope picker's choices: every project, then plain chats when there are any. */
export function threadScopeOptions(model: Pick<ThreadInboxModel, "projects" | "hasNoProject">):
    { value: ThreadScope; label: string; description?: string }[] {
  const names = new Map<string, number>();
  for (const project of model.projects) names.set(project.name, (names.get(project.name) ?? 0) + 1);
  return [
    { value: ALL_THREAD_PROJECTS, label: "All projects" },
    // Two projects called "web" are told apart by where they live.
    ...model.projects.map(project => (names.get(project.name) ?? 0) > 1
      ? { value: threadScopeFor(project.root), label: project.name, description: project.root }
      : { value: threadScopeFor(project.root), label: project.name }),
    ...(model.hasNoProject ? [{ value: NO_THREAD_PROJECT, label: NO_PROJECT_LABEL }] : []),
  ];
}

/** A section header's accessible name; folded, it also says what the fold hides. */
export function threadSectionAccessibleName(section: ThreadInboxSection, folded: boolean): string {
  const count = section.rows.length;
  const name = `${section.label}, ${count} thread${count === 1 ? "" : "s"}`;
  const word = threadMarkWord(section.mark);
  return folded && word !== "" ? `${name}, ${word.toLowerCase()}` : name;
}

/** When a snoozed thread comes back, for its row's tooltip. Local time. */
export function snoozeWakeLabel(until: number, now: number): string {
  const wake = new Date(until);
  const today = new Date(now);
  const time = wake.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  if (wake.toDateString() === today.toDateString()) return `Wakes today at ${time}`;
  const tomorrow = new Date(today);
  tomorrow.setDate(today.getDate() + 1);
  if (wake.toDateString() === tomorrow.toDateString()) return `Wakes tomorrow at ${time}`;
  return `Wakes ${wake.toLocaleDateString([], { weekday: "short", month: "short", day: "numeric" })} at ${time}`;
}

/* ---- what a row draws ------------------------------------------------------
   Pinned and Active rows are two lines (owner, 2026-10-06 redesign): the
   title with ONE right-hand slot, then the provider pip, project and branch
   with the pull request or the diff at the right. Snoozed and Settled rows
   are one slim line. Every decision about what those slots say is here, so
   the component only paints it. */

const WEEKDAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"] as const;
const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"] as const;

/** "6 pm", "9:30 am": the slim row's wall-clock time. Local time. */
function clockLabel(at: Date): string {
  const hours = at.getHours();
  const hour = hours % 12 === 0 ? 12 : hours % 12;
  const half = hours < 12 ? "am" : "pm";
  const minutes = at.getMinutes();
  return minutes === 0 ? `${hour} ${half}` : `${hour}:${String(minutes).padStart(2, "0")} ${half}`;
}

function startOfDay(at: number): number {
  const day = new Date(at);
  day.setHours(0, 0, 0, 0);
  return day.getTime();
}

/**
 * When a snoozed thread comes back, short enough for a slim row: the time
 * today ("6 pm"), the weekday within the coming week ("Mon"), else the date
 * ("Oct 19"). The row's tooltip carries the long form (`snoozeWakeLabel`).
 */
export function threadWakeTime(until: number, now: number): string {
  const wake = new Date(until);
  // Calendar days, rounded so a daylight-saving day still counts as one.
  const days = Math.round((startOfDay(until) - startOfDay(now)) / 86_400_000);
  if (days === 0) return clockLabel(wake);
  if (days > 0 && days < 7) return WEEKDAYS[wake.getDay()];
  return `${MONTHS[wake.getMonth()]} ${wake.getDate()}`;
}

/** A row's age, "now" / "5m" / "3h" / "2d" / "3w"; "" when nothing is known. */
export function threadAgeLabel(at: number, now: number): string {
  return at > 0 ? sessionAgeLabel(new Date(at).toISOString(), now) : "";
}

/** A run's elapsed time, "2:41", or "1:02:09" past the hour. */
export function threadElapsedLabel(startedAt: number, now: number): string {
  const total = Math.max(0, Math.floor((now - startedAt) / 1000));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = String(total % 60).padStart(2, "0");
  return hours > 0 ? `${hours}:${String(minutes).padStart(2, "0")}:${seconds}` : `${minutes}:${seconds}`;
}

/** The marks that put a word in a row's slot; the rest show the age. */
export type ThreadSlotMark = "needs-you" | "working" | "failed" | "woke";

/**
 * The slot's words. Compact on purpose: the slot shares a 240px row with the
 * title. "Needs you" is the canvas the owner approved (2026-10-06); the
 * tooltip and the folded section's name keep `threadMarkWord`'s long forms.
 */
export const THREAD_SLOT_WORD: Readonly<Record<ThreadSlotMark, string>> = {
  "needs-you": "Needs you",
  working: "Working",
  failed: "Failed",
  woke: "Back",
};

/** Line one's right-hand slot: a status word, or the age (after the blue dot when unread). */
export type ThreadRowSlot =
  | { readonly kind: "status"; readonly mark: ThreadSlotMark; readonly word: string; readonly elapsed?: string }
  | { readonly kind: "age"; readonly age: string; readonly unread: boolean };

/** What line one's slot shows, from the one-mark ladder: one thing, never two. */
export function threadRowSlot(row: Pick<ThreadInboxRow, "mark" | "activityAt" | "runStartedAt">,
    now: number): ThreadRowSlot {
  switch (row.mark) {
    case "needs-you":
    case "failed":
    case "woke":
      return { kind: "status", mark: row.mark, word: THREAD_SLOT_WORD[row.mark] };
    case "working":
      return row.runStartedAt === undefined
        ? { kind: "status", mark: "working", word: THREAD_SLOT_WORD.working }
        : { kind: "status", mark: "working", word: THREAD_SLOT_WORD.working,
          elapsed: threadElapsedLabel(row.runStartedAt, now) };
    case "unread":
    case "none":
      return { kind: "age", age: threadAgeLabel(row.activityAt, now), unread: row.mark === "unread" };
  }
}

/**
 * A slim row's right edge: a live status as a glyph (a shelved thread that
 * needs you must still say so), else when a snoozed thread wakes, else the age.
 */
export type ThreadShelfSlot =
  | { readonly kind: "status"; readonly mark: Exclude<ThreadSlotMark, "woke">; readonly word: string }
  | { readonly kind: "wake"; readonly label: string; readonly title: string }
  | { readonly kind: "age"; readonly age: string; readonly unread: boolean };

export function threadShelfSlot(row: Pick<ThreadInboxRow,
    "mark" | "section" | "snoozedUntil" | "activityAt">, now: number): ThreadShelfSlot {
  if (row.mark === "needs-you" || row.mark === "working" || row.mark === "failed") {
    return { kind: "status", mark: row.mark, word: threadMarkWord(row.mark) };
  }
  if (row.section === "snoozed" && row.snoozedUntil !== undefined) {
    return { kind: "wake", label: threadWakeTime(row.snoozedUntil, now),
      title: snoozeWakeLabel(row.snoozedUntil, now) };
  }
  return { kind: "age", age: threadAgeLabel(row.activityAt, now), unread: row.mark === "unread" };
}

/** Line two's right edge: the pull request, else a diff that changed something, else nothing. */
export type ThreadRowTrail =
  | { readonly kind: "pr"; readonly number: number; readonly state: PullRequestState }
  | { readonly kind: "diff"; readonly added: number; readonly removed: number };

export function threadRowTrail(row: Pick<ThreadInboxRow, "pullRequest" | "diff">): ThreadRowTrail | null {
  if (row.pullRequest !== undefined) {
    return { kind: "pr", number: row.pullRequest.number, state: row.pullRequest.state };
  }
  // A measured zero is silent, as `formatDiffStat` keeps it everywhere else.
  if (row.diff !== undefined && (row.diff.added > 0 || row.diff.removed > 0)) {
    return { kind: "diff", added: row.diff.added, removed: row.diff.removed };
  }
  return null;
}

/**
 * Line two's words for a live thread (owner, 2026-10-07, "Inbox under load"):
 * a working row says the step it is on ("Editing login.ts", "Thinking"), a
 * row that needs you says what it asks ("Run npm test?"), a failed row the
 * first line of its newest error. Anything the live log cannot say keeps the
 * project line (`meta`): a session with no log this run, a run the log does
 * not see, a wait with no request in it.
 */
export type ThreadRowLine =
  | { readonly kind: "meta" }
  | { readonly kind: "step" | "ask" | "error"; readonly text: string };

/** The project line: tile, project, branch, then the pull request or the diff. */
export const THREAD_META_LINE: ThreadRowLine = { kind: "meta" };

export function threadRowLine(row: Pick<ThreadInboxRow, "mark" | "sessionRoot">,
    live: ThreadLiveFacts | undefined): ThreadRowLine {
  if (live === undefined) return THREAD_META_LINE;
  if (row.mark === "working") {
    if (!live.running) return THREAD_META_LINE;
    return { kind: "step", text: live.step === undefined ? THREAD_THINKING
      : threadStepLine(threadStepWords(live.step, "running", row.sessionRoot), true) };
  }
  if (row.mark === "needs-you") {
    const ask = live.ask;
    if (ask === undefined) return THREAD_META_LINE;
    return { kind: "ask", text: ask.question
      ?? `${threadStepLine(threadStepWords(ask, "waiting", row.sessionRoot), true)}?` };
  }
  if (row.mark === "failed" && live.error !== undefined) return { kind: "error", text: live.error };
  return THREAD_META_LINE;
}

/**
 * Where a drop on a drawn row lands, in Rust's order: before that row on its
 * upper half; on its lower half before the row Rust keeps after it (the end
 * when it is Rust's last). A row that needs you floats only on screen, so the
 * row drawn under the target is not what "after it" means.
 */
export function threadDropBeforeKey(rustKeys: readonly SessionKey[], over: SessionKey,
    upper: boolean): SessionKey | undefined {
  if (upper) return over;
  const at = rustKeys.indexOf(over);
  return at < 0 ? undefined : rustKeys[at + 1];
}


/**
 * The two letters a thread's tile shows (the TN Code look, owner 2026-11-16):
 * the first letters of the title's first two words, or the first two letters
 * of a one-word title, in capitals. Punctuation leads no word; a title with no
 * letter or digit gets none.
 */
export function threadInitials(title: string): string {
  const words = title.split(/[^\p{L}\p{N}]+/u).filter(word => word !== "");
  if (words.length === 0) return "";
  const letters = words.length === 1
    ? [...words[0]].slice(0, 2)
    : [[...words[0]][0], [...words[1]][0]];
  return letters.join("").toLocaleUpperCase();
}
/**
 * The title's weight: unread is the strongest line in the list; a read
 * thread with nothing to report recedes; a status or the selection keeps the
 * ordinary tier.
 */
export type ThreadTitleTone = "unread" | "plain" | "read";

export function threadTitleTone(row: Pick<ThreadInboxRow, "unread" | "mark" | "selected">): ThreadTitleTone {
  if (row.unread) return "unread";
  return row.mark === "none" && !row.selected ? "read" : "plain";
}

/**
 * The row button's accessible name: everything both lines say, in reading
 * order. The working clock and the live step are left out: a name that
 * changed every second, or at every tool call, would be re-read on a focused
 * row; the hover card has the run's facts. What a thread asks and why it
 * failed follow the status word; the project line stays in the name even
 * while the row draws one of those in its place.
 */
export function threadRowAccessibleName(row: Pick<ThreadInboxRow, "title" | "projectLabel" | "branch" | "away">,
    slot: ThreadRowSlot | ThreadShelfSlot, trail: ThreadRowTrail | null = null,
    line: ThreadRowLine = THREAD_META_LINE): string {
  const parts = [row.title];
  if (row.away === true) parts.push(AWAY_ROW_WORDS);
  if (slot.kind === "status") parts.push(slot.word);
  else if (slot.kind === "wake") parts.push(slot.title);
  else {
    if (slot.unread) parts.push(DONE_WORD);
    if (slot.age !== "") parts.push(slot.age);
  }
  if (line.kind === "ask" || line.kind === "error") parts.push(line.text);
  parts.push(row.projectLabel);
  if (row.branch !== undefined) parts.push(`branch ${row.branch}`);
  if (trail?.kind === "pr") parts.push(`pull request #${trail.number} ${trail.state}`);
  else if (trail?.kind === "diff") parts.push(diffStatLabel(trail));
  return parts.join(", ");
}

/** What an Away loop's row says, after its title (review B3). */
export const AWAY_ROW_WORDS = "Away loop";

/**
 * The chords behind menu entries, for their right-aligned hints: only chords
 * `threadChord` actually reads. ⌘⇧S settles an open row and un-settles a
 * settled one; ⌘⌥P pins and unpins.
 */
export const THREAD_MENU_CHORDS: Readonly<Record<string, { readonly chord: ThreadChord;
    readonly hint: string; readonly keys: string }>> = {
  pin: { chord: "pin", hint: "⌘⌥P", keys: "Meta+Alt+P" },
  unpin: { chord: "pin", hint: "⌘⌥P", keys: "Meta+Alt+P" },
  settle: { chord: "settle", hint: "⌘⇧S", keys: "Meta+Shift+S" },
  unsettle: { chord: "settle", hint: "⌘⇧S", keys: "Meta+Shift+S" },
};

/* ---- folding and keyboard order ------------------------------------------ */

export type ThreadSectionFolds = Readonly<Record<ThreadSection, boolean>>;

/** Snoozed and Settled start folded; Pinned and Active open. */
export const DEFAULT_THREAD_FOLDS: ThreadSectionFolds =
  { pinned: false, active: false, snoozed: true, settled: true, fromTerminals: true };

/** A corrupt or partial stored value falls back section by section to the defaults. */
export function parseThreadFolds(raw: string | null): ThreadSectionFolds {
  if (raw === null) return DEFAULT_THREAD_FOLDS;
  try {
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return DEFAULT_THREAD_FOLDS;
    const record = parsed as Record<string, unknown>;
    const fold = (section: ThreadSection): boolean => typeof record[section] === "boolean"
      ? record[section] as boolean : DEFAULT_THREAD_FOLDS[section];
    return { pinned: fold("pinned"), active: fold("active"), snoozed: fold("snoozed"), settled: fold("settled"),
      fromTerminals: fold("fromTerminals") };
  } catch {
    return DEFAULT_THREAD_FOLDS;
  }
}

/** A search opens every section, so a match is never folded away. */
export function threadSectionFolded(model: Pick<ThreadInboxModel, "searching">,
    folds: ThreadSectionFolds, section: ThreadSection): boolean {
  return !model.searching && folds[section];
}

/**
 * The rows a section draws, in drawn order (a row that needs you first in
 * Pinned and Active): Settled and From terminals a page at a time unless a
 * search is narrowing it.
 */
export function threadSectionRows(model: Pick<ThreadInboxModel, "searching">,
    section: ThreadInboxSection, settledShown: number,
    fromTerminalsShown: number = settledShown): readonly ThreadInboxRow[] {
  if (model.searching) return section.drawn;
  if (section.section === "settled") return section.drawn.slice(0, Math.max(0, settledShown));
  if (section.section === "fromTerminals") return section.drawn.slice(0, Math.max(0, fromTerminalsShown));
  return section.drawn;
}

/** Every drawn row, top to bottom: what ⌘⇧[ and ⌘⇧] walk. */
export function threadInboxOrder(model: ThreadInboxModel, folds: ThreadSectionFolds,
    settledShown: number, fromTerminalsShown: number = settledShown): SessionKey[] {
  return model.sections.flatMap(section => threadSectionFolded(model, folds, section.section)
    ? [] : threadSectionRows(model, section, settledShown, fromTerminalsShown).map(row => row.key));
}

/** The Threads chords: ⌘⇧[ / ⌘⇧] walk the inbox, ⌘⇧S settles, ⌘⌥P pins (⌘⇧P is the
 * command palette), ⌘Z undoes the last settle, snooze or archive. Ctrl stands in for ⌘. */
export type ThreadChord = "previous" | "next" | "settle" | "pin" | "undo";

export function threadChord(event: Pick<KeyboardEvent,
    "key" | "code" | "metaKey" | "ctrlKey" | "shiftKey" | "altKey">): ThreadChord | null {
  if (!event.metaKey && !event.ctrlKey) return null;
  if (event.shiftKey && !event.altKey) {
    if (event.code === "BracketLeft") return "previous";
    if (event.code === "BracketRight") return "next";
    return event.key.toLowerCase() === "s" ? "settle" : null;
  }
  if (event.altKey && !event.shiftKey) return event.code === "KeyP" ? "pin" : null;
  if (!event.altKey && !event.shiftKey && event.key.toLowerCase() === "z") return "undo";
  return null;
}

/**
 * The row focus moves to when `key` leaves the drawn list (Archive): the one
 * after it, else the one before it, else none.
 */
export function threadFocusAfterRemoval(order: readonly SessionKey[], key: SessionKey): SessionKey | null {
  const at = order.indexOf(key);
  if (at < 0) return null;
  return order[at + 1] ?? order[at - 1] ?? null;
}

/**
 * Whether an action with this Undo takes a row in `section` out of it:
 * archive always, settle and snooze unless the row is already there. Focus
 * then goes to the row taking its place, so ⌘Z still finds the sidebar
 * focused rather than a page whose focused row is gone.
 */
export function threadActionLeavesSection(undo: ThreadUndoAction | undefined,
    section: ThreadSection): boolean {
  if (undo === "archive") return true;
  if (undo === "settle") return section !== "settled";
  if (undo === "snooze") return section !== "snoozed";
  return false;
}

/** The thread before or after `current` in `order`, wrapping; the first or last from nowhere. */
export function adjacentThreadKey(order: readonly SessionKey[], current: SessionKey | null,
    direction: "previous" | "next"): SessionKey | null {
  if (order.length === 0) return null;
  const at = current === null ? -1 : order.indexOf(current);
  if (at < 0) return direction === "next" ? order[0] : order[order.length - 1];
  const step = direction === "next" ? 1 : -1;
  return order[(at + step + order.length) % order.length];
}

/* ---- snooze --------------------------------------------------------------- */

export interface SnoozePreset {
  readonly id: "hour" | "tonight" | "tomorrow" | "monday";
  readonly label: string;
  /** Epoch ms, local wall-clock times. */
  readonly until: number;
}

/**
 * The Snooze submenu: an hour from now, tonight at 6 pm (only while that is
 * still ahead), tomorrow at 9 am and next Monday at 9 am (a week out on a
 * Monday). Local time, so a daylight-saving change lands on the wall clock.
 */
export function snoozePresets(now: number): SnoozePreset[] {
  const today = new Date(now);
  const at = (days: number, hour: number): number => {
    const day = new Date(today);
    day.setDate(day.getDate() + days);
    day.setHours(hour, 0, 0, 0);
    return day.getTime();
  };
  const presets: SnoozePreset[] = [{ id: "hour", label: "1 hour", until: now + 3_600_000 }];
  const tonight = at(0, 18);
  if (tonight > now) presets.push({ id: "tonight", label: "Tonight 6 pm", until: tonight });
  presets.push({ id: "tomorrow", label: "Tomorrow 9 am", until: at(1, 9) });
  const toMonday = (8 - today.getDay()) % 7 || 7;
  presets.push({ id: "monday", label: "Next Monday 9 am", until: at(toMonday, 9) });
  return presets;
}

/* ---- the row menu's thread commands --------------------------------------- */

export type ThreadUndoAction = "settle" | "snooze" | "archive";

export interface ThreadMenuCommand {
  readonly id: string;
  readonly label: string;
  readonly commands: readonly ThreadCommand[];
  /** This action offers Undo. */
  readonly undo?: ThreadUndoAction;
  /** Submenu items only: the current choice. */
  readonly checked?: boolean;
}

export interface ThreadMenuSubmenu {
  readonly id: "snooze" | "autoSettle";
  readonly label: string;
  readonly items: readonly ThreadMenuCommand[];
}

export type ThreadMenuEntry = ThreadMenuCommand | ThreadMenuSubmenu;

export function isThreadSubmenu(entry: ThreadMenuEntry): entry is ThreadMenuSubmenu {
  return "items" in entry;
}

/**
 * Whether Rust looks for this thread's pull request: tracking is on and the
 * thread is an unsettled worktree thread with a branch. Only then is
 * "Refresh pull request" worth offering.
 */
export function threadTracksPullRequest(thread: Pick<ThreadView, "section" | "workspace"> | undefined,
    settings: Pick<ThreadSettings, "trackPullRequests"> | null): boolean {
  return thread !== undefined && settings?.trackPullRequests === true && thread.section !== "settled"
    && thread.workspace?.mode === "worktree" && (thread.workspace.branch ?? "") !== "";
}

/** The hover card's PR line, "#42 · open", or undefined without a pull request. */
export function threadPullRequestFact(thread: Pick<ThreadView, "pullRequest"> | undefined): string | undefined {
  const pullRequest = thread?.pullRequest;
  return pullRequest === undefined ? undefined : `#${pullRequest.number} · ${pullRequest.state}`;
}

/**
 * The keyboard's way to reorder (drag's twin): Move up places the thread
 * before the row above it, Move down before the row two below it (at the
 * end when the row below is the last). Pinned and Active only, and only the
 * moves there is room for. `sectionKeys` are the section's rows in Rust's
 * order, not as drawn: a row that needs you floats only on screen, and a move
 * means a place in the order Rust keeps.
 */
export function threadMoveEntries(row: Pick<ThreadInboxRow, "key" | "section">,
    sectionKeys: readonly SessionKey[]): ThreadMenuCommand[] {
  const section = row.section;
  if (section !== "pinned" && section !== "active") return [];
  const at = sectionKeys.indexOf(row.key);
  if (at < 0) return [];
  const moves: ThreadMenuCommand[] = [];
  if (at > 0) {
    moves.push({ id: "moveUp", label: "Move up",
      commands: [reorder(row.key, section, sectionKeys[at - 1])] });
  }
  if (at < sectionKeys.length - 1) {
    moves.push({ id: "moveDown", label: "Move down",
      commands: [reorder(row.key, section, sectionKeys[at + 2])] });
  }
  return moves;
}

/**
 * The thread's own menu items, around the existing session items: `lead`
 * comes first (Pin, Settle, Snooze ▸, Mark unread, Move up / Move down),
 * Rename sits between, and `tail` follows it (Regenerate title, Refresh pull
 * request when Rust tracks one, Auto-settle ▸).
 */
export function threadMenuEntries(row: Pick<ThreadInboxRow,
    "key" | "section" | "pinned" | "unread" | "autoSettle">, now: number,
    options: {
      readonly refreshPullRequest?: boolean;
      /** The row's section in Rust's order (`ThreadInboxSection.rows`), for Move up / Move down. */
      readonly sectionKeys?: readonly SessionKey[];
    } = {}): {
  lead: ThreadMenuEntry[]; tail: ThreadMenuEntry[];
} {
  const key = row.key;
  const fromTerminal = row.section === "fromTerminals";
  const settled = row.section === "settled" || fromTerminal;
  const lead: ThreadMenuEntry[] = [
    row.pinned && !settled
      ? { id: "unpin", label: "Unpin", commands: [{ kind: "unpin", key }] }
      : { id: "pin", label: "Pin", commands: [{ kind: "pin", key }] },
    // A terminal session joins Threads once no Code pane drives it (Rust waits).
    fromTerminal
      ? { id: "continueFromTerminal", label: "Continue as thread",
        commands: [{ kind: "continueFromTerminal", key }] }
      : settled
        ? { id: "unsettle", label: "Un-settle", commands: [{ kind: "unsettle", key }] }
        : { id: "settle", label: "Settle", commands: [{ kind: "settle", key }], undo: "settle" },
  ];
  if (!settled) {
    lead.push({
      id: "snooze", label: "Snooze",
      items: [
        ...snoozePresets(now).map((preset): ThreadMenuCommand => ({
          id: `snooze-${preset.id}`, label: preset.label,
          commands: [{ kind: "snooze", key, until: preset.until }], undo: "snooze",
        })),
        ...(row.section === "snoozed"
          ? [{ id: "unsnooze", label: "Wake now", commands: [{ kind: "unsnooze", key }] } satisfies ThreadMenuCommand]
          : []),
      ],
    });
  }
  if (!row.unread) lead.push({ id: "markUnread", label: "Mark unread", commands: [{ kind: "markUnread", key }] });
  lead.push(...threadMoveEntries(row, options.sectionKeys ?? []));
  const tail: ThreadMenuEntry[] = [
    { id: "regenerateTitle", label: "Regenerate title", commands: [{ kind: "regenerateTitle", key }] },
    ...(options.refreshPullRequest === true
      ? [{ id: "refreshPullRequest", label: "Refresh pull request",
        commands: [{ kind: "refreshPullRequest", key }] } satisfies ThreadMenuCommand]
      : []),
    {
      id: "autoSettle", label: "Auto-settle",
      items: [
        { id: "autoSettle-on", label: "Enabled", checked: row.autoSettle,
          commands: [{ kind: "setAutoSettle", key, enabled: true }] },
        { id: "autoSettle-off", label: "Disabled", checked: !row.autoSettle,
          commands: [{ kind: "setAutoSettle", key, enabled: false }] },
      ],
    },
  ];
  return { lead, tail };
}

/* ---- drag and drop -------------------------------------------------------- */

/** Where a dragged thread lands: before `beforeKey` in `section`, or at its end. */
export interface ThreadDropTarget {
  readonly section: ThreadSection;
  readonly beforeKey?: SessionKey;
}

function reorder(key: SessionKey, section: "pinned" | "active", beforeKey?: SessionKey): ThreadCommand {
  return beforeKey === undefined
    ? { kind: "reorder", key, section }
    : { kind: "reorder", key, section, beforeKey };
}

/**
 * The commands a drop sends. Within Pinned or Active it is a reorder. Across
 * sections it pins (Pinned), settles (Settled) or wakes, unpins and
 * un-settles (Active), then places the thread where it was dropped. Snoozed
 * takes no drops: a snooze needs a time, which the menu asks for.
 */
export function threadDropCommands(row: Pick<ThreadInboxRow, "key" | "section" | "pinned">,
    target: ThreadDropTarget): ThreadCommand[] {
  const { key } = row;
  if (target.beforeKey === key) return [];
  switch (target.section) {
    case "snoozed":
    case "fromTerminals":
      return [];
    case "settled":
      return row.section === "settled" ? [] : [{ kind: "settle", key }];
    case "pinned": {
      const commands: ThreadCommand[] = [];
      if (row.section === "snoozed") commands.push({ kind: "unsnooze", key });
      if (!row.pinned || row.section === "settled") commands.push({ kind: "pin", key });
      commands.push(reorder(key, "pinned", target.beforeKey));
      return commands;
    }
    case "active": {
      const commands: ThreadCommand[] = [];
      if (row.section === "settled") commands.push({ kind: "unsettle", key });
      if (row.section === "snoozed") commands.push({ kind: "unsnooze", key });
      if (row.pinned && row.section !== "settled") commands.push({ kind: "unpin", key });
      commands.push(reorder(key, "active", target.beforeKey));
      return commands;
    }
  }
}

/** Which Undo a drop's commands earn: a drop onto Settled settles. */
export function threadDropUndo(commands: readonly ThreadCommand[]): ThreadUndoAction | undefined {
  return commands.some(command => command.kind === "settle") ? "settle" : undefined;
}

/* ---- undo ----------------------------------------------------------------- */

/** What was true of a thread just before a settle, snooze or archive. */
export interface ThreadUndo {
  readonly action: ThreadUndoAction;
  readonly key: SessionKey;
  readonly title: string;
  readonly section: ThreadSection;
  readonly pinned: boolean;
  readonly snoozedUntil?: number;
  /** The row that followed it in its section, so Undo can put it back in place. */
  readonly next?: SessionKey;
}

export function threadUndo(action: ThreadUndoAction, row: ThreadInboxRow,
    model: Pick<ThreadInboxModel, "sections">): ThreadUndo {
  const rows = model.sections.find(section => section.section === row.section)?.rows ?? [];
  const next = rows[rows.findIndex(item => item.key === row.key) + 1]?.key;
  return {
    action, key: row.key, title: row.title, section: row.section, pinned: row.pinned,
    ...(row.snoozedUntil === undefined ? {} : { snoozedUntil: row.snoozedUntil }),
    ...(next === undefined || next === row.key ? {} : { next }),
  };
}

/**
 * The inverse of a settle or snooze, as thread commands. Archive is undone by
 * the session's own unarchive, so it has none. A snooze that would already
 * have ended is not restored.
 */
export function threadUndoCommands(undo: ThreadUndo, now: number): ThreadCommand[] {
  const { key } = undo;
  const resnooze: ThreadCommand[] = undo.section === "snoozed"
    && undo.snoozedUntil !== undefined && undo.snoozedUntil > now
    ? [{ kind: "snooze", key, until: undo.snoozedUntil }] : [];
  if (undo.action === "snooze") return resnooze.length > 0 ? resnooze : [{ kind: "unsnooze", key }];
  if (undo.action !== "settle" || undo.section === "settled") return [];
  if (undo.pinned) {
    return [{ kind: "pin", key },
      ...(undo.section === "pinned" ? [reorder(key, "pinned", undo.next)] : []), ...resnooze];
  }
  return [{ kind: "unsettle", key },
    ...(undo.section === "active" ? [reorder(key, "active", undo.next)] : []), ...resnooze];
}

export function threadUndoMessage(undo: Pick<ThreadUndo, "action" | "title">): string {
  const verb = undo.action === "settle" ? "Settled" : undo.action === "snooze" ? "Snoozed" : "Archived";
  return `${verb} “${undo.title}”.`;
}

/* ---- run ends --------------------------------------------------------------- */

/**
 * Which threads' runs ended since the previous view: a run end Rust recorded
 * that this window had not seen. The first view only establishes what was
 * already known, so opening the app never counts an old run as just ended.
 */
export function threadRunEnds(previous: ReadonlyMap<SessionKey, number | null> | null,
    view: ThreadsView): { seen: Map<SessionKey, number | null>; ended: SessionKey[] } {
  const seen = new Map<SessionKey, number | null>();
  const ended: SessionKey[] = [];
  for (const thread of view.threads) {
    const end = thread.lastRun?.endedAt ?? null;
    seen.set(thread.key, end);
    if (previous !== null && end !== null && previous.get(thread.key) !== end) ended.push(thread.key);
  }
  return { seen, ended };
}

/**
 * The thread to visit because it just became the focused pane of the Threads
 * surface, whichever way (a pane clicked, ⌃Tab, maximize, a neighbour
 * closing): `previous` is the focused pane last seen. Nothing while the
 * first registry listing has not come — the layout is only being restored,
 * which nobody chose — or when no thread pane is focused.
 */
export function threadFocusVisit(previous: SessionKey | null, focused: SessionKey | null,
    registryReady: boolean): SessionKey | null {
  return registryReady && focused !== null && focused !== previous ? focused : null;
}

/**
 * The thread to visit because its run just ended while it was the focused
 * pane of a focused window: the owner watched it finish, so it is not unread.
 */
export function threadRunEndVisit(previous: ReadonlyMap<SessionKey, number | null> | null,
    view: ThreadsView, watchingKey: SessionKey | null, windowFocused: boolean):
    { seen: Map<SessionKey, number | null>; visit: SessionKey | null } {
  const { seen, ended } = threadRunEnds(previous, view);
  return { seen, visit: watchingKey !== null && windowFocused && ended.includes(watchingKey) ? watchingKey : null };
}

/* ---- kept worktrees ---------------------------------------------------------- */

/* ---- the bridges to Code and the Agent tab (review C1) --------------------- */

/** A teammate a thread can be handed to. */
export interface ThreadTeammate {
  readonly id: string;
  readonly name: string;
}

/**
 * Whether Open in terminal applies, and if not, why (its tooltip). Only
 * Claude and Codex resume in a Code pane, and a session moves only between
 * turns: Rust refuses otherwise, this only says so before the click.
 */
export function threadTerminalRefusal(row: Pick<ThreadInboxRow, "engine" | "status">): string | null {
  if (row.engine !== "claude" && row.engine !== "codex") {
    return "Only Claude and Codex threads open in a Code pane.";
  }
  if (row.status === "working" || row.status === "waiting") {
    return "Stop the turn or let it finish first.";
  }
  return null;
}

/**
 * The task a teammate gets for a thread: the thread's title, where it runs and
 * how to find it, then a short summary. The teammate starts its own session;
 * the thread is left as it is.
 */
export function threadHandoffTask(thread: {
  readonly title: string;
  readonly key: SessionKey;
  readonly projectRoot: string | null;
  readonly sessionRoot: string;
  readonly branch?: string;
  readonly summary?: string;
}): string {
  const where = thread.projectRoot === null ? "with no project folder"
    : `in ${thread.sessionRoot}${thread.branch === undefined ? "" : ` (branch ${thread.branch})`}`;
  const lines = [
    `Pick up the work from the thread "${thread.title}", which ran ${where}.`,
    `It is the Interface thread ${thread.key}; do not resume or edit that session.`,
  ];
  const summary = thread.summary?.trim() ?? "";
  if (summary !== "") lines.push("", "Where it stands:", summary);
  return lines.join("\n");
}

/** A worktree Rust kept when its thread was archived or deleted, and why. */
export interface ThreadKeptWorktree {
  readonly key: SessionKey;
  readonly path: string;
  readonly kept: WorkspaceKept;
}

/**
 * Worktrees Rust kept since the previous view: a thread's `workspace.kept`
 * this window had not seen. Rust records it once the release that follows an
 * archive, delete or forget has run, on the thread's own record, which stays
 * in the view after the session leaves the list. The first view only
 * establishes what was already kept, so opening the app never repeats an old
 * notice.
 */
export function threadKeptWorktrees(previous: ReadonlyMap<SessionKey, WorkspaceKept> | null,
    view: ThreadsView): { seen: Map<SessionKey, WorkspaceKept>; kept: ThreadKeptWorktree[] } {
  const seen = new Map<SessionKey, WorkspaceKept>();
  const kept: ThreadKeptWorktree[] = [];
  for (const thread of view.threads) {
    const workspace = thread.workspace;
    if (workspace?.kept === undefined) continue;
    seen.set(thread.key, workspace.kept);
    const path = workspace.worktreePath ?? "";
    if (previous !== null && previous.get(thread.key) !== workspace.kept && path !== "") {
      kept.push({ key: thread.key, path, kept: workspace.kept });
    }
  }
  return { seen, kept };
}

const THREAD_KEPT_REASON: Readonly<Record<WorkspaceKept, string>> = {
  dirty: "it has uncommitted changes",
  ignored: "it has ignored files such as .env",
  inUse: "another session uses it",
};

/** Why a kept worktree stayed, as the notice's clause after the path. */
export function threadKeptReason(kept: WorkspaceKept): string {
  return THREAD_KEPT_REASON[kept];
}

/** The whole notice for a kept worktree, as it reads. */
export function threadKeptText(notice: Pick<ThreadKeptWorktree, "path" | "kept">): string {
  return `Kept the worktree ${notice.path}: ${threadKeptReason(notice.kept)}. Remove it when you’re done.`;
}
