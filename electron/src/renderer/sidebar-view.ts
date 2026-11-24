import { sessionKey } from "../shared/engine";
import type {
  EngineId, SessionKey, SessionKind, SessionStatus, SessionSummary,
} from "../shared/engine";
import type { TerminalLaunchProfileId, TerminalSummary } from "../shared/ipc";
import type { TerminalLiveActivity } from "./terminal-activity";
import { formatDiffStat } from "./diff-stats";
import type { DiffStat } from "./diff-stats";
import { statusWord } from "./components/status-words";
import { productLabel } from "./product";
import { needsAttention } from "./session-activity";

/**
 * The sidebar's view model: session rows, project groups and their order,
 * terminal workspace projection, search and filters, what the sidebar says
 * about hidden rows, and the import progress line.
 */

/** Sidebar/navigation placement is distinct from the provider execution kind.
 * A code-capable conversation is still a chat, not a CLI. Code contains the
 * separately projected terminal workspaces.
 * Keep Agent tasks on Agent without changing any session's execution options. */
export function sessionSurfaceOf(session: Pick<SessionSummary, "kind">): ConversationSurface {
  return session.kind === "agent" ? "agent" : "chat";
}

/** The surfaces that hold conversations. Code holds terminal workspaces only. */
export type ConversationSurface = Exclude<SessionKind, "code">;

export interface SidebarRow {
  key: SessionKey; title: string; engine: EngineId;
  status: SessionStatus; badge: "waiting" | "working" | "idle" | "failed";
  cloud: boolean; selected: boolean;
  lastActivity: string;
  /**
   * This session was NOT started in Interface (`origin !== "app"`, absent
   * included — see the field's comment in shared/engine.ts). The row menu uses
   * it to decide between offering "Add to Interface" and "Remove from
   * Interface"; nothing about the row's appearance keys off it, because the
   * origin filter already answers "should this row be here at all".
   */
  external: boolean;
  /**
   * The throwaway git checkout this session is standing in RIGHT NOW, by name —
   * absent for every session that is not in one. See `managedWorktreeName` for
   * why a released worktree stops being reported the moment `cwd` moves home.
   */
  worktree?: string;
}

/**
 * The name of the managed worktree a session is running in, or undefined.
 *
 * The renderer has no branch field: the registry keeps `worktreeHome` (where
 * the session lives once the checkout is given back) and `worktreePath` (the
 * directory inside the checkout it was launched in), and nothing else. So the
 * NAME is what can be told truthfully, and it is the branch's own stem —
 * worktree.ts cuts `<base>/<slug>-<n>` on the branch `iface/<slug>-<n>`.
 *
 * Two rules keep it honest:
 *  - `cwd !== worktreePath` means the worktree has been released (the registry
 *    moves cwd back to the home) or the engine reported a move, so there is
 *    nothing to name;
 *  - the launch directory can be NESTED inside the checkout — `worktreePath` is
 *    `<checkout>/<path relative to the repo root>` — so the segments the two
 *    paths share at the tail are dropped before the name is read. Without that
 *    a session started in `<repo>/src/renderer` would be labelled "renderer".
 */
export function managedWorktreeName(
  s: Pick<SessionSummary, "cwd" | "worktreeHome" | "worktreePath">,
): string | undefined {
  const { worktreeHome, worktreePath } = s;
  if (worktreeHome === undefined || worktreePath === undefined) return undefined;
  if (s.cwd !== worktreePath) return undefined;
  const home = worktreeHome.split("/").filter(segment => segment !== "");
  const inside = worktreePath.split("/").filter(segment => segment !== "");
  let shared = 0;
  while (shared < home.length && shared + 1 < inside.length
    && home[home.length - 1 - shared] === inside[inside.length - 1 - shared]) shared++;
  return inside[inside.length - 1 - shared];
}

/** Everything the sidebar's hover card says about a row. */
export interface SidebarRowMeta {
  /** The state's one word, from the shared table every surface prints. */
  status: string;
  /**
   * Lines changed, or null when this window saw no diff at all. Null draws
   * NOTHING — never "+0 −0", which is a measurement claim over a session whose
   * edits simply predate this window.
   */
  diff: DiffStat | null;
  /** Compact recency, or "" when the timestamp is unreadable. */
  age: string;
  /** The managed worktree's name, or "" when the session is not in one. */
  worktree: string;
}

/**
 * The hover card's facts for one row. Honest omission throughout: every field
 * that is unknown comes back empty and is drawn as nothing, so the card never
 * carries a placeholder dash for data the app does not have.
 */
export function sidebarRowMeta(row: SidebarRow, diff?: DiffStat | null,
  now = Date.now()): SidebarRowMeta {
  // formatDiffStat is the one place that decides a stat is worth printing, so
  // a net-zero diff is silent here for the same reason it is silent on a card.
  const changes = formatDiffStat(diff) === "" ? null : diff ?? null;
  return {
    status: statusWord(row.badge),
    diff: changes,
    age: sessionAgeLabel(row.lastActivity, now),
    worktree: row.worktree ?? "",
  };
}

/** Compact recency for a sidebar row. Under a minute reads as "now". */
export function sessionAgeLabel(iso: string, now = Date.now()): string {
  const then = Date.parse(iso);
  if (!Number.isFinite(then)) return "";
  const seconds = Math.max(0, Math.round((now - then) / 1000));
  if (seconds < 60) return "now";
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h`;
  if (seconds < 86400 * 14) return `${Math.floor(seconds / 86400)}d`;
  return `${Math.floor(seconds / (86400 * 7))}w`;
}

function toRow(s: SessionSummary, selected: SessionKey | null): SidebarRow {
  const key = sessionKey(s.engine, s.sessionId);
  const worktree = managedWorktreeName(s);
  return {
    key, title: s.title, engine: s.engine, status: s.status,
    badge: s.status, cloud: s.cloud, selected: key === selected,
    lastActivity: s.lastActivity,
    external: s.origin !== "app",
    ...(worktree === undefined ? {} : { worktree }),
  };
}

export interface SidebarGroup {
  projectPath: string; projectName: string; rows: SidebarRow[];
  /** Rust-hosted terminals projected beside, but never counted as, chat rows. */
  terminals?: SidebarTerminalRow[];
  counts: { total: number; waiting: number; working: number };
}

export interface SidebarTerminalRow {
  id: string;
  title: string;
  state: TerminalSummary["state"];
  root: string;
  cwd: string;
  lastActivity: string;
  selected: boolean;
  profileId?: TerminalLaunchProfileId;
  activity?: TerminalLiveActivity;
  /** A live agent finished a turn nobody has looked at yet. */
  done?: boolean;
}

/**
 * The identity of the one group that is NOT a project: every session whose
 * `projectRoot` is absent, i.e. that ran somewhere outside any repository.
 *
 * It has to be a `projectPath` like any other group, because that string is the
 * React key, the persisted per-project collapse key and every lookup key the
 * sidebar and the status bar use. So it must be a value no real `projectRoot`
 * can ever equal — and a NUL byte is illegal in a path on every filesystem this
 * app runs on, so not even a directory literally named "Other" can collide with
 * it. ("" was rejected: it was a REAL group under the old cwd-based grouping,
 * so a stale collapse entry for it would have landed on this bucket.)
 */
export const OTHER_PROJECT = "\u0000other";

/** Its display name. Deliberately a word, not a path — it is not a place. */
export const OTHER_PROJECT_NAME = "Other";

/** What to show where a real group shows its path: the sentinel is unprintable. */
export const OTHER_PROJECT_HINT = "Sessions that did not run inside a repository";

// The one rewrite allowed on a project root: `/a/web/` and `/a/web` are the same
// project, so a trailing separator cannot be allowed to split the group. A lone
// "/" keeps its slash — a repository at the filesystem root is still a real
// path, and stripping it would leave the empty string, which means "absent".
function normalizeProjectPath(root: string): string {
  let end = root.length;
  while (end > 1 && root[end - 1] === "/") end--;
  return root.slice(0, end);
}

/**
 * Which group a session belongs to. `projectRoot` is the nearest ancestor of
 * its cwd containing a `.git` (computed in the main process — see the field's
 * comment in shared/engine.ts), so an agent run in `<repo>/src/renderer` groups
 * with one run in `<repo>` instead of opening a second group beside it.
 *
 * There is deliberately NO fallback to `cwd`: a session outside every repository
 * pertains to no project, and the user asked for exactly one bucket for those
 * rather than a group per scratch directory. An empty string reads as absent —
 * it is not a path anyone can be in.
 */
function projectKeyOf(s: SessionSummary): string {
  const root = s.projectRoot;
  return root === undefined || root === "" ? OTHER_PROJECT : normalizeProjectPath(root);
}

/** What the sidebar calls a project: its last "/" segment, or "Other" for the bucket. */
export function projectNameOf(projectPath: string): string {
  if (projectPath === OTHER_PROJECT) return OTHER_PROJECT_NAME;
  const segments = projectPath.split("/");
  for (let i = segments.length - 1; i >= 0; i--) {
    if (segments[i] !== "") return segments[i];
  }
  // Only "/" reaches here — a repository at the filesystem root. Its own path is
  // a truer label than any placeholder, and a placeholder would now read as a
  // second "Other".
  return projectPath;
}

/**
 * Renderer-only projection inputs for placing Rust-owned terminal summaries in
 * the same project tree as chats. `projectSessions` must be the unfiltered
 * registry listing: a terminal bound to a hidden worker still belongs to that
 * worker's canonical project root.
 */
export interface SidebarTerminalProjection {
  /** Terminals drawn on the current surface, with the renderer's done mark. */
  terminals: readonly (TerminalSummary & { readonly done?: boolean })[];
  /**
   * The full terminal registry, even when the current surface does not draw
   * terminals. Project archive safety must not disappear when the user moves
   * from Code to Chat or Agent.
   */
  registryTerminals?: readonly TerminalSummary[];
  projectSessions: readonly SessionSummary[];
  /**
   * Session project roots remembered before a chat left the visible registry.
   * A running terminal keeps its original project when its owning chat is
   * archived; this cache is presentation identity, never filesystem authority.
   */
  rememberedProjectRoots?: ReadonlyMap<SessionKey, string | null>;
  selectedTerminalId: string | null;
  /** Renderer-derived live activity, keyed by terminal id. Absent is idle. */
  activities?: Readonly<Record<string, TerminalLiveActivity>>;
}

function projectOwnsPath(project: string, normalized: string): boolean {
  if (normalized === project) return true;
  // "/" is a prefix of every Unix path. A slash-command session whose cwd is
  // "/" must not swallow every other project.
  if (project === "/") return false;
  return normalized.startsWith(`${project}/`);
}

function terminalWorkspaceRoot(terminal: Pick<TerminalSummary, "root" | "cwd">): string {
  const root = terminal.root.trim();
  const cwd = terminal.cwd.trim();
  // A poisoned reopen proof records the filesystem root while the pane still
  // works in a real project directory. Prefer that directory for grouping.
  return root === "" || root === "/" ? cwd : root;
}

function terminalProjectKeyOf(terminal: TerminalSummary,
    sessionsByKey: ReadonlyMap<SessionKey, SessionSummary>,
    rememberedProjectRoots: ReadonlyMap<SessionKey, string | null> | undefined,
    knownProjectPaths: readonly string[]): string {
  if (terminal.sessionKey !== undefined) {
    const owner = sessionsByKey.get(terminal.sessionKey);
    if (owner !== undefined) return projectKeyOf(owner);
    if (rememberedProjectRoots?.has(terminal.sessionKey) === true) {
      const remembered = rememberedProjectRoots.get(terminal.sessionKey);
      return remembered === null || remembered === undefined || remembered === ""
        ? OTHER_PROJECT
        : normalizeProjectPath(remembered);
    }
  }
  // An unbound shell has no chat lineage to join. Prefer a known canonical chat
  // project that contains its exact Rust-authorized workspace; only then fall
  // back to showing that workspace as a terminal-only project. This is
  // presentation metadata and never grants authority to the matched ancestor.
  const workspaceRoot = terminalWorkspaceRoot(terminal);
  if (workspaceRoot === "") return OTHER_PROJECT;
  const normalized = normalizeProjectPath(workspaceRoot);
  const owner = knownProjectPaths.find(project => projectOwnsPath(project, normalized));
  return owner ?? normalized;
}

function terminalMatches(terminal: TerminalSummary, terms: readonly string[]): boolean {
  if (terms.length === 0) return true;
  const hay = [
    terminal.title,
    terminal.launch?.profileId ?? "shell",
    terminal.state,
    terminal.cwd,
    terminal.root,
  ].join("\n").toLowerCase();
  return terms.every(term => hay.includes(term));
}

function terminalStateRank(state: TerminalSummary["state"]): number {
  if (state === "starting") return 0;
  if (state === "running") return 1;
  if (state === "failed") return 2;
  return 3;
}

function terminalStateIsLive(state: TerminalSummary["state"]): boolean {
  return state === "starting" || state === "running";
}

/** A live PTY is busy only while it is producing work or awaiting input;
 *  a process that has not reported yet is an empty pane, not work. */
export function terminalHasActiveWork(terminal: {
  readonly state: SidebarTerminalRow["state"];
  readonly activity?: TerminalLiveActivity;
}): boolean {
  return terminalStateIsLive(terminal.state)
    && (terminal.activity === "working" || terminal.activity === "waiting");
}

/**
 * The one sidebar badge a saved terminal workspace earns from the terminals
 * it currently owns. Failed outranks a request for input, which outranks
 * live work, which outranks rest — so a workspace row and its collapsed
 * group never disagree, and a quiet running shell does not look busy.
 *
 * The four session badges are the only status language this pane speaks.
 */
export function workspaceSidebarBadge(
  members: readonly {
    readonly state: SidebarTerminalRow["state"];
    readonly activity?: TerminalLiveActivity;
  }[],
): SidebarRow["badge"] {
  if (members.some(member => member.state === "failed")) return "failed";
  if (members.some(member =>
    terminalStateIsLive(member.state)
    && member.activity === "waiting")) {
    return "waiting";
  }
  if (members.some(member => terminalStateIsLive(member.state)
    && member.activity === "working")) {
    return "working";
  }
  return "idle";
}

/**
 * Whether a workspace row shows the blue done dot: it would otherwise rest
 * (every other badge outranks done) and one of its live agents finished a
 * turn nobody has looked at yet.
 */
export function workspaceSidebarDone(
  members: readonly {
    readonly state: SidebarTerminalRow["state"];
    readonly activity?: TerminalLiveActivity;
    readonly done?: boolean;
  }[],
): boolean {
  return workspaceSidebarBadge(members) === "idle"
    && members.some(member => terminalStateIsLive(member.state) && member.done === true);
}

/** One workspace pane's pip light: its status, or idle. */
export type WorkspaceSidebarPipLight = "working" | "waiting" | "done" | "failed" | "idle";

/**
 * One workspace pane's pip: its light and, for an agent pane, which agent it
 * runs, so a Codex pane and a Claude pane read apart (owner, 2026-10-02).
 */
export interface WorkspaceSidebarPip {
  readonly light: WorkspaceSidebarPipLight;
  /** Absent for a plain shell, which has no provider colour. */
  readonly profileId?: Exclude<TerminalLaunchProfileId, "shell">;
}

/** The most pips a workspace row draws before "+N". */
export const WORKSPACE_SIDEBAR_PIPS_MAX = 8;

/**
 * One pip per terminal in the workspace, in its saved order (owner,
 * 2026-10-01), with the same precedence as the pane's own light: failed, then
 * waiting, working and an unseen finished turn; the rest are idle. A running
 * agent whose tab was closed keeps its pip, so a hidden one that needs you
 * still shows; one that has exited is gone. Each pip names its agent so the
 * row can paint it in that provider's colour.
 */
export function workspaceSidebarPips(
  members: readonly {
    readonly state: SidebarTerminalRow["state"];
    readonly activity?: TerminalLiveActivity;
    readonly done?: boolean;
    readonly profileId?: TerminalLaunchProfileId;
  }[],
): { readonly shown: readonly WorkspaceSidebarPip[]; readonly more: number } {
  const lightOf = (member: typeof members[number]): WorkspaceSidebarPipLight => {
    if (member.state === "failed") return "failed";
    if (!terminalStateIsLive(member.state)) return "idle";
    if (member.activity === "waiting") return "waiting";
    if (member.activity === "working") return "working";
    return member.done === true ? "done" : "idle";
  };
  const pips = members.filter(member => member.state !== "exited").map((member): WorkspaceSidebarPip => {
    const light = lightOf(member);
    return member.profileId === undefined || member.profileId === "shell"
      ? { light } : { light, profileId: member.profileId };
  });
  // Past the cap, the lights that say something come first, so a pane that
  // needs you or has finished is never the one folded into "+N".
  const shown = pips.length <= WORKSPACE_SIDEBAR_PIPS_MAX ? pips
    : [...pips.filter(pip => pip.light !== "idle"), ...pips.filter(pip => pip.light === "idle")]
      .slice(0, WORKSPACE_SIDEBAR_PIPS_MAX);
  return { shown, more: Math.max(0, pips.length - WORKSPACE_SIDEBAR_PIPS_MAX) };
}

/**
 * A project's workspaces in their saved order, independent of selection,
 * except that any with live work — a shell starting,
 * producing output, or waiting on the user — rise to the top. Stable, so the
 * rest never shuffle.
 */
export function workspacesActiveFirst<T extends { readonly terminalIds: readonly string[] }>(
  records: readonly T[],
  terminals: readonly SidebarTerminalRow[],
): T[] {
  const byId = new Map(terminals.map(terminal => [terminal.id, terminal]));
  const active = (record: T): boolean => record.terminalIds.some(id => {
    const terminal = byId.get(id);
    return terminal !== undefined && terminalHasActiveWork(terminal);
  });
  return [...records].sort((left, right) => Number(active(right)) - Number(active(left)));
}

/** Saved names win. Unnamed legacy workspaces inherit their first member's title,
 * in saved membership order, never the pane focus or most recent activity. */
export function workspaceSidebarTitle(
  workspaceName: string,
  members: readonly { readonly title: string; readonly selected: boolean; readonly lastActivity: string }[],
  firstTerminalTitle?: string,
): string {
  const named = workspaceName.trim();
  if (named !== "" && !/^Workspace \d+$/i.test(named)) return named;
  return firstTerminalTitle?.trim() || members.find(member => member.title.trim() !== "")?.title || named || "Workspace";
}

/**
 * Where a saved workspace belongs in the project list. Live terminals win,
 * except a filesystem-root assignment (a poisoned reopen proof). After a
 * restart they may not exist yet, so fall back to the first saved working
 * directory so the row does not vanish until PTYs reopen.
 */
export function workspaceSidebarProjectPath(
  terminalIds: readonly string[],
  terminalProjects: Readonly<Record<string, string>>,
  cwd: string | undefined,
  knownProjectPaths: readonly string[] = [],
): string | undefined {
  const live = terminalIds
    .map(id => terminalProjects[id])
    .find((path): path is string => typeof path === "string"
      && path.trim() !== ""
      && path.trim() !== "/");
  if (live !== undefined) return live;
  const root = cwd?.trim() ?? "";
  if (root === "") return undefined;
  const normalized = normalizeProjectPath(root);
  const owner = [...knownProjectPaths]
    .filter(path => path !== OTHER_PROJECT)
    .sort((left, right) => right.length - left.length || left.localeCompare(right))
    .find(project => projectOwnsPath(project, normalized));
  return owner ?? normalized;
}

interface TerminalProjectIndex {
  sessionsByKey: ReadonlyMap<SessionKey, SessionSummary>;
  knownProjectPaths: readonly string[];
}

/**
 * The two lookups `terminalProjectKeyOf` needs: the sessions a terminal may
 * belong to, by key, and every project root it could sit under, longest first
 * so the deepest root wins. Built once per render and shared by the grouping
 * pass and the per-terminal project map, which each used to build their own.
 */
function terminalProjectIndex(projection: SidebarTerminalProjection): TerminalProjectIndex {
  return {
    sessionsByKey: new Map(projection.projectSessions.map(summary =>
      [sessionKey(summary.engine, summary.sessionId), summary])),
    knownProjectPaths: [...new Set(projection.projectSessions
      .map(projectKeyOf)
      .concat([...(projection.rememberedProjectRoots?.values() ?? [])]
        .map(root => root === null || root === "" ? OTHER_PROJECT : normalizeProjectPath(root)))
      .filter(project => project !== OTHER_PROJECT))]
      .sort((left, right) => right.length - left.length || left.localeCompare(right)),
  };
}

function projectGroupsWithTerminals(groups: readonly SidebarGroup[],
    projection: SidebarTerminalProjection, terms: readonly string[],
    index: TerminalProjectIndex): SidebarGroup[] {
  const buckets = new Map<string, SidebarGroup>();
  for (const source of groups) {
    const rows = projection.selectedTerminalId === null
      ? source.rows
      : source.rows.map(row => row.selected ? { ...row, selected: false } : row);
    buckets.set(source.projectPath, { ...source, rows });
  }

  for (const terminal of projection.terminals) {
    if (!terminalMatches(terminal, terms)) continue;
    const projectPath = terminalProjectKeyOf(
      terminal,
      index.sessionsByKey,
      projection.rememberedProjectRoots,
      index.knownProjectPaths,
    );
    let bucket = buckets.get(projectPath);
    if (bucket === undefined) {
      bucket = {
        projectPath,
        projectName: projectNameOf(projectPath),
        rows: [],
        counts: { total: 0, waiting: 0, working: 0 },
      };
      buckets.set(projectPath, bucket);
    }
    const title = terminal.title.trim() === "" ? "Terminal" : terminal.title.trim();
    const activity = terminalStateIsLive(terminal.state)
      ? terminal.activity ?? projection.activities?.[terminal.id]
      : undefined;
    const row: SidebarTerminalRow = {
      id: terminal.id,
      title,
      state: terminal.state,
      root: terminal.root,
      cwd: terminal.cwd,
      lastActivity: terminal.updatedAt,
      selected: terminal.id === projection.selectedTerminalId,
      ...(terminal.launch === undefined ? {} : { profileId: terminal.launch.profileId }),
      ...(activity === undefined ? {} : { activity }),
      ...(terminalStateIsLive(terminal.state) && terminal.done === true ? { done: true } : {}),
    };
    (bucket.terminals ??= []).push(row);
  }

  return [...buckets.values()].sort(compareSidebarGroups).map(group => {
    group.terminals?.sort((left, right) =>
      Number(right.selected) - Number(left.selected)
      || terminalStateRank(left.state) - terminalStateRank(right.state)
      || (left.lastActivity < right.lastActivity ? 1 : left.lastActivity > right.lastActivity ? -1 : 0)
      || left.title.localeCompare(right.title));
    return group;
  });
}

/**
 * A project's slot in the pane never depends on what is happening inside it:
 * urgency reaches the user through the header dot and the lifted attention
 * section, and recency through the rows, so the list itself stays a fixed map
 * the eye can learn — by name, path as the tie-break, "Other" last whatever
 * it holds. "Other" is a leftovers bin rather than a project, and its header
 * still carries the amber dot and count, so nothing is concealed by the floor.
 */
export function compareSidebarGroups(a: SidebarGroup, b: SidebarGroup): number {
  const aOther = a.projectPath === OTHER_PROJECT ? 1 : 0;
  const bOther = b.projectPath === OTHER_PROJECT ? 1 : 0;
  return aOther - bOther
    || a.projectName.localeCompare(b.projectName)
    || a.projectPath.localeCompare(b.projectPath);
}

/**
 * A title that is nothing but a slash command — `/doctor`, `/usage`, `/mcp`.
 * Anchored at both ends, so "/model please" and "/doctor output was odd" do not
 * match; the whole title has to BE the command.
 *
 * This is the filter's PRIMARY signal, and it replaces a byte-threshold-only
 * design that could not work. Measured over this machine's 87 real Claude
 * sessions: 17 of them (19.5%) have a bare-slash-command title — /usage x7,
 * /model x3, /doctor x3, /effort, /config, /mcp, /cd — and size cannot find
 * them, because a slash command's transcript size has nothing to do with how
 * much was SAID. /doctor dumps a diagnostic report (287/326/334 KB, straddling
 * the median), /model is 1.6 MB and /effort 1.9 MB. The one session the byte
 * rule catches that this one misses is "Switch to Opus model" at 2,523 bytes,
 * which is why size survives below as a secondary signal rather than the only
 * one.
 */
const BARE_SLASH_COMMAND = /^\/[a-z][a-z0-9-]*$/i;

/**
 * Transcript bytes under which an idle session is treated as throwaway on SIZE
 * alone. Empirical, not a guess: a size study over the same 87 transcripts
 * found every threshold in [24029 … 28304] hides the exact same 13 sessions —
 * a plateau above the largest observed `/usage` transcript (24028) and below
 * the first session that could be real work ("Set up GitHub authentication",
 * 28304).
 *
 * 26 KiB is the value chosen from inside that plateau, rather than a round
 * 26000, so the number the user is shown is EXACT in the unit they are shown
 * it in. 26000/1000 labelled "KB" was both rounded and disagreed with what
 * `ls -lh` and `du -h` report, which count in 1024s. Any value in the plateau
 * behaves identically on the measured corpus, so this costs nothing.
 */
export const TRIVIAL_SESSION_BYTES = 26_624;

/**
 * The threshold in the unit the UI states it in. Exported so the sentence the
 * user reads is derived from the constant that is actually applied and cannot
 * drift from it, and asserted integral in the tests — change the constant to
 * something that is not a whole number of KiB and the test fails rather than
 * the label silently rounding.
 */
export const TRIVIAL_SESSION_KIB = TRIVIAL_SESSION_BYTES / 1024;

/** Grouped sidebar plus the facts the sidebar cannot derive from `groups`. */
export interface SidebarView {
  groups: SidebarGroup[];
  /**
   * Exactly the rows missing from `groups`, split by the rule that removed
   * them: `external` for "not started in Interface", `trivial` for the
   * declutter rule. A session both rules could hide is attributed to the one
   * that actually removed it, never to both — so `external + trivial` is always
   * the true number of hidden rows, in every combination of the two toggles,
   * and the sidebar can name the cause without ever overstating a count.
   */
  hidden: { external: number; trivial: number };
  /**
   * How many rows BOTH rules together would remove, whatever the toggles are
   * doing right now — the number the filter row offers when nothing is hidden.
   * Counts a session once even when both rules could hide it, so it is never
   * less than `hidden.external + hidden.trivial` and never a sum of overlaps.
   */
  hideable: number;
  /**
   * Distinct projects in the UNFILTERED listing. The sidebar decides whether to
   * draw project headers at all from this, never from `groups.length`: deriving
   * it from the filtered list lets turning the filter on collapse a two-project
   * sidebar into flat mode, which changes every row's indent, adds a title
   * suffix, and voids the user's persisted per-project collapse state — a large
   * visual change caused by a filter that is only supposed to drop rows.
   */
  projectCount: number;
  /**
   * Exactly the sessions behind `groups`, in listing order — the flat set that
   * survived every filter and the search.
   *
   * It exists so every consumer of the visible listing agrees about which
   * agents are on screen: handing one `state.sessions` would bury six running
   * agents under eighty idle ones; handing it this array makes "the pane shows
   * what the sidebar shows" a fact rather than a convention.
   */
  visible: SessionSummary[];
  /**
   * How many sessions in the whole listing are NOT Interface's yet
   * (`origin !== "app"`, absent included), whatever the filters are doing.
   *
   * IT IS NOT `hidden.external`, and the difference is the reason it exists.
   * `hidden.external` is 0 whenever the origin filter is off, so keying the
   * import affordances to that number took the ONLY bulk-import control in the
   * app off the screen at exactly the moment the user clicked "Show all" to
   * look at those sessions before importing them — and the way back was a
   * button reading "Hide 87", which is the opposite of what they asked for.
   * The two also count different things: `hidden.external` excludes sessions
   * that are non-idle or opened this run, because neither filter ever hides
   * one, whereas the import marks EVERY session the engines know about — so a
   * button labelled from it understated its own work by the rows in front of
   * the user.
   */
  importable: number;
  /**
   * Rows a FILTER is holding back that the current search would have matched.
   * 0 whenever there is no search. It is the difference between "nothing here
   * is called that" and "the thing you are looking for is behind the filter" —
   * the second is a dead end unless the pane says so.
   */
  hiddenMatches: number;
  /** Whether a search is narrowing this view at all. */
  searching: boolean;
  /** Full-registry project identities with an agent or terminal doing/awaiting work. */
  protectedProjects: string[];
  /** Stable terminal-to-project identity, independent of search/filter visibility. */
  terminalProjects: Record<string, string>;
}

/* ---- search ---------------------------------------------------------------
   112 sessions across 25 directories are not findable by scrolling, and the
   engines' own titles are the only labels they have. The rule is deliberately
   dumb and predictable: case-insensitive substring, every whitespace-separated
   term must match SOMETHING (title, engine, path), so "iface claude" narrows
   rather than widens. No fuzzy matching — a scoring function nobody can predict
   is worse than a filter that finds nothing and says so. */

/** The query split into the terms every session must satisfy. */
export function queryTerms(query: string): string[] {
  return query.toLowerCase().split(/\s+/).filter(t => t !== "");
}

/**
 * Whether one session satisfies every term. The haystack is the title, the
 * engine id, the cwd and the project root: a user hunting for "the codex run in
 * interface" has exactly those words, and the path is the only place the
 * project name lives on a SessionSummary.
 */
export function sessionMatches(s: SessionSummary, terms: readonly string[]): boolean {
  if (terms.length === 0) return true;
  const hay = `${s.title}\n${s.engine}\n${s.cwd}\n${s.projectRoot ?? ""}`.toLowerCase();
  return terms.every(t => hay.includes(t));
}

/**
 * Whether a session may be hidden. Every clause is a veto, and the order below
 * is the order of how much damage getting it wrong does.
 *
 * 1. NOT idle — never. A "waiting" session is blocked on the user's approval,
 *    and the first version of this filter hid one. "failed" is worse than it
 *    looks: a crashed launch writes a tiny transcript, so a size rule hides
 *    failures SYSTEMATICALLY, exactly when the user needs to see them.
 * 2. NOT revealed. Sticky for the whole app run — see `revealed` below.
 * 3. Then either signal:
 *    - the title IS a bare slash command AND the engine itself supplied that
 *      title (`titleFromSummary`). The second half is load-bearing: a title can
 *      also be a firstPrompt fallback, and a REAL session that merely opened
 *      with "/model" would be hidden on the title alone. When the flag is
 *      absent (unknown) it reads as false, which shows the session.
 *    - or the transcript is KNOWN to be small. `size === undefined` is the
 *      engine declining to answer — every Codex thread today — and must never
 *      count as small. `<` is strict, so a session exactly at the cut is kept.
 */
function isTrivial(s: SessionSummary, revealed: ReadonlySet<SessionKey>): boolean {
  if (s.status !== "idle") return false;
  if (revealed.has(sessionKey(s.engine, s.sessionId))) return false;
  if (s.titleFromSummary === true && BARE_SLASH_COMMAND.test(s.title)) return true;
  return s.size !== undefined && s.size < TRIVIAL_SESSION_BYTES;
}

/**
 * Whether a session may be hidden as "not started here". The two vetoes above
 * this rule's own test are the SAME two the declutter rule opens with, in the
 * same order and for the same reasons, and they are repeated rather than shared
 * so that neither rule can ever be tightened without seeing them:
 *
 * 1. NOT idle — never. A session waiting on the user's approval, still working,
 *    or failed is news, and where it was started from does not make it less so.
 *    Hiding a waiting session was a real defect once already.
 * 2. NOT revealed (which always includes the current selection). A row the user
 *    has opened this run stays put, so nothing vanishes under the pointer.
 * 3. Then: the session did not come from Interface. ABSENT `origin` counts as
 *    "store", never as "app" — see the field's comment in shared/engine.ts. The
 *    marker is a durable engine-side tag, so unknown really does mean "no tag",
 *    and guessing the other way would refill the sidebar with CLI sessions.
 */
function isExternal(s: SessionSummary, revealed: ReadonlySet<SessionKey>): boolean {
  if (s.status !== "idle") return false;
  if (revealed.has(sessionKey(s.engine, s.sessionId))) return false;
  return s.origin !== "app";
}

/**
 * The grouped sidebar with both view filters applied. Non-destructive: this
 * drops rows from ONE render, and nothing about the session changes — archiving
 * is still the per-row button, and it is still the only thing that mutates. In
 * particular `appOnly` never writes, unwrites or moves an engine-side tag.
 *
 * `revealed` is every session key the user has selected at any point during
 * this app run, and the caller keeps adding to it — it is deliberately NOT just
 * the current selection. Exempting only the CURRENT selection made the row
 * vanish the instant the user clicked a different one, and when it was its
 * group's last survivor the project header went with it, mid-click. A session
 * the user has looked at stays visible until the app restarts.
 *
 * Filtering happens BEFORE grouping, which is what keeps the counts honest: a
 * group's `counts` are derived from the sessions that reached it, so a header
 * can never advertise rows the user cannot see. It also means a group whose
 * every session was hidden does not exist at all rather than rendering as an
 * empty project header — a header with nothing under it is pure noise.
 *
 * `query` is the search box, and it is the one filter that DOES drop the
 * selected row. That is the difference between a search and the two toggles
 * above: a search is transient, explicit, and undone by clearing one field the
 * user is already typing in, whereas the toggles are persistent settings that
 * would otherwise make a row vanish under the pointer. The conversation stays
 * open in the right pane either way, so nothing is lost by the row going.
 */
export function buildSidebarView(sessions: SessionSummary[], selected: SessionKey | null,
  revealed: ReadonlySet<SessionKey>, hideTrivial: boolean, appOnly: boolean,
  query = "", terminalProjection?: SidebarTerminalProjection): SidebarView {
  // The current selection is revealed by definition. The caller maintains that
  // invariant too; enforcing it here as well means a selection restored from
  // outside the click path can never hide the row it points at.
  const seen: ReadonlySet<SessionKey> = selected === null || revealed.has(selected)
    ? revealed
    : new Set(revealed).add(selected);

  const terms = queryTerms(query);
  const kept: SessionSummary[] = [];
  const projects = new Set<string>();
  let external = 0;
  let trivial = 0;
  let hideable = 0;
  let hiddenMatches = 0;
  let importable = 0;
  const protectedProjects = new Set<string>();
  for (const session of terminalProjection?.projectSessions ?? sessions) {
    if (session.status === "working" || needsAttention(session)) {
      protectedProjects.add(projectKeyOf(session));
    }
  }
  for (const s of sessions) {
    projects.add(projectKeyOf(s));
    // The import's own number, taken off the durable engine-side tag alone —
    // no status veto, no revealed veto, no toggle. See SidebarView.importable
    // for why it must not be `hidden.external`.
    if (s.origin !== "app") importable++;
    const ext = isExternal(s, seen);
    const triv = isTrivial(s, seen);
    // Once each, for the "Hide N" offer: a session both rules could take is
    // still only one row.
    if (ext || triv) hideable++;
    // Attribution follows the toggles, so each count is what its rule actually
    // removed from THIS render rather than what it might remove in some other
    // configuration. Origin is tested first only to break the tie when both
    // apply; the row that is hidden is the same either way.
    if ((appOnly && ext) || (hideTrivial && triv)) {
      if (appOnly && ext) external++;
      else trivial++;
      // A filtered-out row that MATCHES the search is the one the user is
      // probably hunting for. Counted here, before the search drops it, so the
      // pane can offer the way to it instead of saying "nothing found".
      if (terms.length > 0 && sessionMatches(s, terms)) hiddenMatches++;
      continue;
    }
    if (!sessionMatches(s, terms)) continue;
    kept.push(s);
  }
  const baseGroups = buildSidebarGroups(kept, selected);
  const terminalProjects: Record<string, string> = {};
  let groups = baseGroups;
  if (terminalProjection !== undefined) {
    const index = terminalProjectIndex(terminalProjection);
    for (const terminal of terminalProjection.registryTerminals ?? terminalProjection.terminals) {
      const project = terminalProjectKeyOf(
        terminal,
        index.sessionsByKey,
        terminalProjection.rememberedProjectRoots,
        index.knownProjectPaths,
      );
      terminalProjects[terminal.id] = project;
      const activity = terminal.activity ?? terminalProjection.activities?.[terminal.id];
      if (terminalHasActiveWork({
        state: terminal.state,
        ...(activity === undefined ? {} : { activity }),
      })) {
        protectedProjects.add(project);
      }
    }
    for (const terminal of terminalProjection.terminals) {
      const project = terminalProjects[terminal.id] ?? terminalProjectKeyOf(
        terminal,
        index.sessionsByKey,
        terminalProjection.rememberedProjectRoots,
        index.knownProjectPaths,
      );
      terminalProjects[terminal.id] = project;
      projects.add(project);
    }
    groups = projectGroupsWithTerminals(baseGroups, terminalProjection, terms, index);
  }
  return {
    groups,
    hidden: { external, trivial },
    hideable,
    projectCount: projects.size,
    visible: kept,
    importable,
    hiddenMatches,
    searching: terms.length > 0,
    protectedProjects: [...protectedProjects],
    terminalProjects,
  };
}

/* ---- what the sidebar SAYS about the rows it is not showing ---------------
   Both sentences are derived from `SidebarView.hidden` and live here, beside
   the rule that produced those numbers, for the same reason the size threshold
   is read off its own constant: a filter's sentence that can drift from the
   filter is a lie waiting to happen, and this way it is pinned by tests. The
   component owns the markup; it does not own the arithmetic. */

function sessionWord(n: number): string {
  return n === 1 ? "session" : "sessions";
}

/**
 * The filter row's label. Derived from the hidden counts ALONE: they are by
 * definition what is missing from this render, so the row never has to be told
 * which toggles are on and cannot contradict the list under it. Each cause is
 * named only while it is actually taking rows; when both are, the row states
 * the total rather than an ellipsised list of two in a narrow pane.
 */
export function sidebarFilterLabel(hidden: { external: number; trivial: number }): string {
  if (hidden.external === 0 && hidden.trivial === 0) return "Showing all sessions";
  if (hidden.trivial === 0) return `${hidden.external} from other tools hidden`;
  if (hidden.external === 0) return `${hidden.trivial} throwaway hidden`;
  return `${hidden.external + hidden.trivial} hidden`;
}

/**
 * What the pane should offer as the way OUT of an empty list. The sentence
 * alone is not enough: every empty state here has a different cause and a
 * different escape, and offering the wrong button (a "New session" CTA under
 * "your filter hid all 112 of them") is how a filter starts reading as a bug.
 *
 *  - "show-all"     a FILTER emptied the list; the toggle above puts it back.
 *  - "clear-search" the SEARCH emptied it, and no hidden row matches either.
 *  - "new"          the list is genuinely empty. Start something.
 */
export type EmptyAction = "show-all" | "clear-search" | "new";

export interface EmptyState { text: string; hint: string | null; action: EmptyAction }

/**
 * The zero-state, in two parts: what is true, then what to do about it.
 *
 * The origin filter makes an empty pane the NORMAL first run — no session on
 * the machine carries Interface's marker until one is started here — so "No
 * agent sessions yet." would be flatly wrong at the moment the app most needs
 * to be believed, and an unexplained empty sidebar reads as a broken app. The
 * hint names the count and points at the control one row above it.
 *
 * THE DEFECT THIS ALSO FIXES, for the caller that used to decide from
 * `sessions.length` alone: with two Interface sessions of 100 and 200 bytes on
 * the machine, the declutter rule empties the list and the old right-pane
 * sentence read "No chats started in Interface yet." — false, and it pointed at
 * the wrong control. Both panes now read the same two counts, so they cannot
 * contradict each other or the list.
 *
 * `searching` is checked FIRST because it is the one the user just did: when a
 * search finds nothing, saying "no chats started in Interface yet" is answering
 * a question nobody asked.
 *
 * THE THIRD PATH, and why "No chats started in Interface yet." is not the
 * answer whenever `external` is non-zero. Attribution makes the two counts
 * disjoint: a row the origin rule could take is counted as `external`, so a row
 * counted as `trivial` while that rule is ON is one the origin rule did NOT
 * apply to — which, given both rules share the same idle-and-unrevealed
 * vetoes, means `origin === "app"`. So `external > 0 && trivial > 0` is
 * PROOF that chats were started in Interface, and the sentence that says
 * otherwise is false in exactly that state. It gets its own branch, naming both
 * causes, rather than a subordinate clause under a wrong headline.
 */
export function sidebarEmptyState(hidden: { external: number; trivial: number },
    search: { searching?: boolean; hiddenMatches?: number } = {}): EmptyState {
  if (search.searching === true) {
    const behind = search.hiddenMatches ?? 0;
    if (behind > 0) {
      return {
        text: "No matches in the sessions shown.",
        hint: `${behind} hidden ${sessionWord(behind)} match. Choose “Show all” above.`,
        action: "show-all",
      };
    }
    return { text: "No sessions match your search.", hint: null, action: "clear-search" };
  }
  if (hidden.external > 0 && hidden.trivial > 0) {
    return {
      text: "Both filters are hiding everything here.",
      hint: `Hidden: ${hidden.external} started from your terminal or another tool, ` +
        `and ${hidden.trivial} throwaway ${sessionWord(hidden.trivial)} started here. ` +
        "Choose “Show all” above.",
      action: "show-all",
    };
  }
  if (hidden.external > 0) {
    return {
      text: `No chats started in ${productLabel()} yet.`,
      hint: `Hidden: ${hidden.external} ${sessionWord(hidden.external)} started from ` +
        "your terminal or another tool. Choose “Show all” above.",
      action: "show-all",
    };
  }
  if (hidden.trivial > 0) {
    return {
      text: "Every session here is a slash command or a scratch run.",
      hint: "Choose “Show all” above.",
      action: "show-all",
    };
  }
  return { text: "No agent sessions yet.", hint: null, action: "new" };
}

/* ---- importing every existing session ------------------------------------ */

/** Live progress line for a run over ~170 sessions. */
export function importProgressLabel(p: { adopt: boolean; done: number; total: number }): string {
  const verb = p.adopt ? "Importing" : "Removing";
  return p.total === 0 ? `${verb}…` : `${verb} ${p.done} of ${p.total}…`;
}

/**
 * What an import actually did, in the user's terms — and honest about `local`.
 *
 * Codex cannot stamp a thread as Interface's (its metadata call carries git
 * fields and nothing else), so those sessions rest on this app's own registry
 * flag: they show up exactly like the others until a reinstall loses the cache.
 * Reporting `ok` alone would imply a durability half the rows do not have, and
 * reporting them as failures would be wrong in the other direction.
 */
export function importSummary(report: {
  adopt: boolean; total: number; ok: number; local: number; failed: number;
}): string {
  const verb = report.adopt ? "Imported" : "Removed";
  const parts = [`${verb} ${report.ok + report.local} of ${report.total} ` +
    `${sessionWord(report.total)}.`];
  if (report.local > 0) {
    parts.push(`${report.local} marked in ${productLabel()} only — that engine has no tag ` +
      `of its own, so a reinstall would forget ${report.local === 1 ? "it" : "them"}.`);
  }
  if (report.failed > 0) {
    parts.push(`${report.failed} failed.`);
  }
  return parts.join(" ");
}

// Groups keep a fixed name order (compareSidebarGroups). Rows inside a group
// are newest lastActivity first so a project folder reads as a recents list.
// Equal timestamps keep first-appearance order because sort() is stable.
export function buildSidebarGroups(sessions: SessionSummary[],
  selected: SessionKey | null): SidebarGroup[] {
  const buckets = new Map<string, SidebarGroup>();
  for (const s of sessions) {
    const projectPath = projectKeyOf(s);
    let bucket = buckets.get(projectPath);
    if (bucket === undefined) {
      bucket = {
        projectPath, projectName: projectNameOf(projectPath), rows: [],
        counts: { total: 0, waiting: 0, working: 0 },
      };
      buckets.set(projectPath, bucket);
    }
    bucket.rows.push(toRow(s, selected));
    bucket.counts.total++;
    // The group's own share of the one attention fact — same predicate as the
    // attention tray and the dock badge, so a project header can never call a
    // session blocked that the toolbar does not.
    if (needsAttention(s)) bucket.counts.waiting++;
    else if (s.status === "working") bucket.counts.working++;
  }
  return [...buckets.values()].sort(compareSidebarGroups).map(group => {
    group.rows.sort((a, b) =>
      a.lastActivity < b.lastActivity ? 1 : a.lastActivity > b.lastActivity ? -1 : 0);
    return group;
  });
}
