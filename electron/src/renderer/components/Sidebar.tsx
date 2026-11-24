import {
  cloneElement, type HTMLAttributes, type ReactElement, type ReactNode,
  memo, useEffect, useId, useMemo, useRef, useState,
} from "react";
import { createPortal } from "react-dom";
import type { EngineOp } from "../../shared/ipc";
import {
  compareSidebarGroups, importProgressLabel, OTHER_PROJECT, OTHER_PROJECT_HINT,
  projectNameOf, sidebarEmptyState, sidebarFilterLabel, sidebarRowMeta,
  terminalHasActiveWork, TRIVIAL_SESSION_KIB, workspaceSidebarBadge, workspaceSidebarDone,
  workspaceSidebarPips, workspaceSidebarProjectPath, workspaceSidebarTitle, workspacesActiveFirst,
} from "../sidebar-view";
import type { SidebarGroup, SidebarRow, SidebarTerminalRow } from "../sidebar-view";
import type { DiffStat } from "../diff-stats";
import "../theme.css";
import { positionAppSelect, type AppSelectPosition } from "./AppSelect";
import { ConfirmDialog } from "./ConfirmDialog";
import { ProductName } from "./ProductName";
import { productLabel } from "../product";
import { providerLogoKind } from "./ProviderLogo";
import { shellOwnsChord } from "../terminal-keys";
import {
  positionSessionPeek, SESSION_PEEK_CSS, SessionPeek,
  type SessionPeekFacts, type SessionPeekPosition,
} from "./SessionPeek";
import { DONE_WORD, statusWord } from "./status-words";
import { useSidebarHeaderClipping } from "../sidebar-header-clipping";
import { useCommittedActions } from "../use-stable-value";
import { presenceClass, RetainedPresence, usePresence } from "../presence";
import { scrollMovesAnchor } from "./scroll-anchor";
import { ThreadInbox, ThreadScopePicker, type ThreadInboxProps } from "./ThreadInbox";
import type { ThreadInboxController } from "../use-thread-inbox";
import type { ThreadLiveFacts } from "../session-activity";
import {
  FOREGROUND_DIALOG_SELECTOR, menuKeyNav, roveMenuTabIndex, SESSION_PEEK_HOVER_MS,
  SESSION_PEEK_LEAVE_MS, sessionConfirmDialog, useAnchoredOverlay,
} from "./sidebar-controls";

export { FOREGROUND_DIALOG_SELECTOR, SESSION_PEEK_HOVER_MS, SESSION_PEEK_LEAVE_MS };

/**
 * The lifecycle actions a row offers, and the one fact that decides which of
 * them are drawn at all.
 *
 * `can` is feature-detection over the LIVE adapters (window.iface.engineOps()),
 * never a table keyed by engine name: every engine lifecycle operation is optional,
 * the two engines genuinely differ — Codex cannot stamp an adoption, because its
 * metadata call carries git fields and nothing else — and a hardcoded table goes
 * stale the first time an engine gains a method. An action `can` does not
 * confirm is not rendered; it is never rendered-and-rejected.
 */
export interface SidebarLifecycle {
  can: (engine: string, op: EngineOp) => boolean;
  /**
   * Keys with a live engine handle. Destructive lifecycle actions name the
   * extra consequence in their confirmation; the caller then uses closeSession
   * to end liveness before writing the provider store. Stop remains a turn-only
   * action and is never presented as a substitute for close.
   */
  liveKeys: ReadonlySet<string>;
  onRename: (key: string, title: string) => void;
  /** IRREVERSIBLE. This component confirms first; nothing downstream asks. */
  onDelete: (key: string) => void;
  onFork: (key: string) => void;
  /** The ENGINE's archive flag, distinct from `onArchive`, which is local. */
  onEngineArchive: (key: string) => void;
  onAdopt: (key: string, adopt: boolean) => void;
  onStop: (key: string) => void;
  /** The last action's refusal, already a sentence for a person. */
  error: string | null;
  onDismissError: () => void;
}

/** The one-click import of everything the engines already know about. */
export interface SidebarImporter {
  onImportAll: () => void;
  running: boolean;
  done: number;
  total: number;
  adopt: boolean;
  /** What the last run did, in `importSummary`'s words. */
  note: string | null;
  onDismissNote: () => void;
}

export interface SidebarProps {
  /**
   * Project-grouped sessions, straight from `buildSidebarGroups()`. Groups are
   * urgency then recency; rows inside a group are newest lastActivity first.
   * This component deliberately does not re-sort either one — the comparator
   * lives in the view-model and is tested there.
   */
  groups?: SidebarGroup[];
  /**
   * Distinct projects in the caller's UNFILTERED listing — `SidebarView.
   * projectCount`. Whether headers are drawn keys off this rather than off
   * `groups.length`, so a filter that empties a group cannot flip the whole
   * pane from grouped mode to flat mode: that would re-indent every row, graft
   * a project name onto the part header, and strand the user's persisted
   * per-project collapse state on groups that no longer render a chevron.
   * Absent falls back to the group count.
   */
  projectCount?: number;
  /** Project identities with an agent or terminal doing/awaiting work in the full registry. */
  protectedProjects?: readonly string[];
  /** Stable terminal ownership from the unfiltered registry projection. */
  terminalProjects?: Readonly<Record<string, string>>;
  pinnedProjects?: readonly string[];
  onPinProject?: (path: string, pinned: boolean) => void;
  pinnedWorkspaces?: readonly string[];
  onPinWorkspace?: (id: string, pinned: boolean) => void;
  /** Sidebar translucency is on, so sticky project headers need row clipping. */
  glass?: boolean;
  /**
   * The caller has no listing to show YET — the first read of the session store
   * has not come back. Draws session-row-shaped skeletons in place of the list.
   *
   * It is a separate prop and not `groups.length === 0`, because those are two
   * different facts and only one of them may be stated: an empty listing means
   * "no sessions match", which the zero-state says in words, and saying that
   * over a store that simply has not been read yet is a claim that is not true.
   * Absent means "what you were handed is what there is".
   */
  loading?: boolean;
  /**
   * Session key -> the engine's own rate-limit message, for rows whose latest
   * `usage.updated` carried one. Derived by the caller from the event log, not
   * from the registry: rate limiting is display-only and never changes a
   * session's status or its place in this list.
   */
  rateLimits: Record<string, string>;
  /**
   * Session key -> lines that agent added and removed, counted by the caller
   * from the diffs the engines streamed (src/renderer/diff-stats.ts). A key with
   * no entry is UNKNOWN and draws nothing — never "+0 −0" over a resumed session
   * whose edits simply predate this window.
   *
   * A prop rather than a field on SidebarRow for the same reason `rateLimits`
   * is one: the row view-model is a projection of the registry's summary, and
   * this fact comes from the event log the shell owns.
   */
  diffStats?: Record<string, DiffStat>;
  /**
   * Session key -> the extra facts the hover peek shows and the row does not:
   * the last activity line, how many files changed, and the pending approval's
   * own words. Same event log, same derivations the transcript uses — this
   * pane adds no IPC of its own. Absent keys simply narrow the card.
   */
  peeks?: Record<string, SessionPeekFacts>;
  onSelect: (key: string) => void;
  /**
   * Saved terminal workspaces published by the terminal center render as
   * session rows inside their owning project under Workspaces.
   *
   * This pane deliberately does NOT list the terminals themselves any more: a
   * terminal belongs to a workspace, and the workspace is what you go to. The
   * per-terminal rows made every project group grow a second list the user had
   * to hunt through to reach a shell that was already on screen one click away.
   */
  terminalWorkspaces?: {
    readonly records: readonly {
      readonly id: string;
      readonly name: string;
      /** Title of this workspace's first saved terminal, never the pane focus. */
      readonly firstTerminalTitle?: string;
      /** Tabs currently attached; saved membership may be larger. */
      readonly openTerminalCount: number;
      /** Those tabs' terminals in pane order; absent draws pips for every member. */
      readonly openTerminalIds?: readonly string[];
      readonly terminalIds: readonly string[];
      /** First saved working directory; used when live PTYs have not reopened. */
      readonly cwd?: string;
    }[];
    readonly activeId: string | null;
    readonly onSelect: (workspaceId: string) => void;
    readonly onCreate: () => void;
    readonly onRename: (workspaceId: string) => void;
    readonly onDelete: (workspaceId: string) => void;
    readonly busy: boolean;
    readonly canCreate: boolean;
  };
  /**
   * The Agent surface's roster, rendered before the task navigation and above
   * every session listing. A slot rather than props of its own: the agents, the
   * selection and their storage all belong to App.
   */
  agentMenu?: ReactNode;
  /** The terminal surface is the one currently shown in the center. */
  terminalsActive?: boolean;
  /** Code offers workspace and project creation; appearance is selected in Settings. */
  codeWorkspaces?: boolean;
  /** Deliberately adds a session beside the current pane instead of replacing it. */
  onOpenInNewPane?: (key: string) => void;
  onNew: () => void;
  onNewProject?: () => void;
  /** What the create button starts on this surface: "New thread" or "New task". */
  newLabel?: string;
  /** What this pane is listing on the showing surface: "Threads", "Workspaces" or "Tasks". */
  sectionLabel?: string;
  /** The showing surface's copy for the true, unfiltered zero-state. */
  zeroStateText?: string;
  /** Whether New Chat is the primary surface currently shown. */
  newChatActive?: boolean;
  /** Starts a fresh chat with the exact repository root owned by that project group. */
  onNewInProject: (projectPath: string) => void;
  /**
   * Hand the current filter text to the Library's stored-transcript search.
   * Offered only when filtering this pane found nothing.
   */
  onSearchTranscripts?: (query: string) => void;
  /** The settings page is showing, so this pane's own ⌘K must not claim the key. */
  settingsActive?: boolean;
  /** Opens the shared Settings destination from the persistent sidebar footer. */
  onSettings?: () => void;
  settingsDisabled?: boolean;
  /** Providers that need setup: an amber dot and "N providers need setup" on
      the right of the Settings row (owner, 2026-11-15; under Settings as a
      red row since 2026-09-28). Opens Settings when chosen. */
  footerAlert?: {
    label: string;
    /** The same count in fewer words ("2 need setup") for a narrow row. */
    shortLabel?: string;
    description: string;
    accessibleName: string;
  };
  /** Opens Settings on the page that fixes the alert; plain Settings when absent. */
  onFooterAlert?: () => void;
  /** Focus target when the narrow layout presents this pane as a modal drawer. */
  focusOnMobileOpen?: boolean;
  /** A sibling modal owns interaction while this persistent navigation stays mounted. */
  backgroundInert?: boolean;
  /** Closes the narrow modal drawer from its own keyboard/focus boundary. */
  onCloseMobile?: () => void;
  /**
   * Archive (hide) a session row. The registry's onChange broadcast drops the
   * row from this list; Library > Archived provides the restore affordance.
   * It may resolve whether the archive happened: the Threads inbox offers
   * Undo only then.
   */
  onArchive: (key: string) => void | Promise<boolean>;
  /**
   * Exactly the rows missing from `groups`, by cause — `SidebarView.hidden`.
   * The filter command states the number and names the cause from these two; it
   * never adds them to `hideable`, which would double-count.
   */
  hidden: { external: number; trivial: number };
  /**
   * What both filters together would hide — `SidebarView.hideable`. 0 contributes
   * no filter command to the Projects overflow: a control that can only say
   * "nothing to hide" is chrome.
   * Nothing can be hidden without being hideable, so this alone decides whether
   * that command exists; import answers to a different number.
   */
  hideable: number;
  /**
   * How many sessions on this machine are not Interface's yet —
   * `SidebarView.importable`. The import command keys off THIS and never off
   * `hidden.external`, which is zero the moment the origin filter is off: keyed
   * to that, the button vanished exactly when the user clicked "Show all" to
   * look at those sessions before importing them, and the only way to bring it
   * back was to click "Hide 87". Absent contributes no import command at all.
   */
  importable?: number;
  /**
   * Turn every filter off when anything is hidden, and back on when nothing is.
   *
   * ONE control for BOTH rules, deliberately. They are two rules of one
   * statement — "rows are missing, click to see them" — and giving each its own
   * row put two lines of chrome above a list whose whole job is the list, on
   * the very screen where the list can also be empty. The user's mental model
   * here is "show me less" / "show me everything", not a filter matrix; the
   * tooltip states both rules, and one click is the whole way back.
   */
  onToggleFilters: () => void;
  /**
   * The search box's value and its setter. Both absent renders no box at all —
   * the filtering itself is the caller's (`buildSidebarView`), so a box here
   * with nothing behind it would be a control that does nothing.
   */
  query?: string;
  onQuery?: (query: string) => void;
  /**
   * `SidebarView.searching` / `.hiddenMatches`. The zero-state needs both: "no
   * session is called that" and "3 that are called that sit behind a filter"
   * are different answers with different ways out.
   */
  searching?: boolean;
  hiddenMatches?: number;
  /** Optional engine-owned row actions. App-owned actions remain available without it. */
  lifecycle?: SidebarLifecycle;
  /** The bulk import. Absent renders no import affordance anywhere. */
  importer?: SidebarImporter;
  /**
   * The Threads surface's inbox. Present only there: it replaces the project
   * list, the Needs attention lift and the project folds with the flat
   * Pinned / Active / Snoozed / Settled inbox, and adds the project scope
   * picker to the header. Absent, this pane renders exactly as before.
   */
  threadInbox?: ThreadInboxController;
  /**
   * Session key -> what its live log says (`threadLiveFacts`), for a Threads
   * row's second line. Read only with `threadInbox`.
   */
  threadLive?: Readonly<Record<string, ThreadLiveFacts>>;
  /**
   * The bridges from a Threads row to Code and the Agent tab (review C1):
   * Open in terminal and Hand to teammate. Read only with `threadInbox`.
   */
  threadBridges?: Pick<ThreadInboxProps, "onOpenInTerminal" | "teammates" | "onHandToTeammate"
    | "offersFor" | "onSecondOpinion" | "onSwitchTo">;
}

/**
 * The collapse localStorage key this component owns. Value is a JSON array of
 * `projectPath` strings — never `projectName`, which is not unique (`/a/web`
 * and `/b/web` both display as `web`). Paths for projects that are not in the
 * current listing are kept: a project can come back, and forgetting its state
 * would silently expand it.
 *
 * The project archive key below is also renderer-local because it filters
 * groups only inside this component. The sidebar's OTHER persisted view
 * settings — the declutter toggle and the "started here only" toggle — live in
 * `App.tsx` beside the rest of the renderer's storage keys. App owns those
 * settings because it also owns their filtering.
 */
const COLLAPSED_KEY = "interface.sidebar.collapsedProjects";
const ARCHIVED_PROJECTS_KEY = "interface.sidebar.archivedProjects";

export interface ArchivedProject {
  projectPath: string;
  projectName: string;
}

/**
 * Project archive state is intentionally renderer-local: archiving a project
 * hides its group in Interface and never writes to an engine transcript. Keep
 * the small parser exported so corrupt or older localStorage values are a
 * tested empty state instead of a startup failure.
 */
/** A project's rows, folding open and shut rather than blinking. Folding shut,
 * the list stays mounted only while it animates, and inert, so keyboard
 * traversal and screen readers skip it just as they skip the removed list. */
function FoldingRows({ open, children }: {
  open: boolean;
  /** The list while open; `false` while collapsed, so no rows are built. */
  children: ReactElement<HTMLAttributes<HTMLElement>> | false;
}): ReactNode {
  const { phase, onAnimationEnd } = usePresence(open);
  return <RetainedPresence open={open} phase={phase} render={current => children !== false
    && cloneElement(children, {
      className: `${children.props.className ?? ""}${presenceClass(current)}`,
      inert: current === "leaving" || undefined,
      onAnimationEnd,
    })} />;
}

export function parseArchivedProjects(raw: string | null): ArchivedProject[] {
  if (raw === null) return [];
  try {
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    const projects: ArchivedProject[] = [];
    const seen = new Set<string>();
    for (const value of parsed) {
      if (typeof value !== "object" || value === null) continue;
      const record = value as Record<string, unknown>;
      if (typeof record.projectPath !== "string" || record.projectPath === ""
        || typeof record.projectName !== "string" || record.projectName === ""
        || seen.has(record.projectPath)) continue;
      seen.add(record.projectPath);
      projects.push({ projectPath: record.projectPath, projectName: record.projectName });
    }
    return projects;
  } catch {
    return [];
  }
}

export function withArchivedProject(
  projects: readonly ArchivedProject[],
  project: ArchivedProject,
): ArchivedProject[] {
  return [project, ...projects.filter(item => item.projectPath !== project.projectPath)];
}

export function withoutArchivedProject(
  projects: readonly ArchivedProject[],
  projectPath: string,
): ArchivedProject[] {
  return projects.filter(project => project.projectPath !== projectPath);
}

export function withoutProtectedArchivedProjects(
  projects: readonly ArchivedProject[],
  protectedProjectPaths: ReadonlySet<string>,
): ArchivedProject[] {
  return projects.filter(project => !protectedProjectPaths.has(project.projectPath));
}

export function projectArchiveBlocked(group: SidebarGroup,
    protectedProjects: ReadonlySet<string> = new Set()): boolean {
  return protectedProjects.has(group.projectPath)
    || group.counts.waiting > 0 || group.counts.working > 0
    || group.terminals?.some(terminalHasActiveWork) === true;
}

/**
 * A row this pane lifts out of its project group into the pinned "Needs
 * attention" section at the top: an agent blocked on the user, or one that has
 * stopped. Exactly the pair this pane's own Needs attention section holds, so
 * the two surfaces can never disagree about what needs the user.
 *
 * The lift is why a blocked agent cannot hide: with 138 sessions in 16 groups
 * it was previously reachable only by expanding the right project — two
 * collapse toggles deep in "Other 91" — and the only always-on signal for it
 * was the dock badge, outside the window entirely.
 */
export function isAttentionRow(row: SidebarRow): boolean {
  return row.badge === "waiting" || row.badge === "failed";
}

/** Eight 32px rows leave useful room for several project headers at 768px tall. */
export const SIDEBAR_PROJECT_ROW_LIMIT = 8;

export interface SidebarProjectRows {
  rows: SidebarRow[];
  hidden: number;
}

/**
 * Keep each project scannable without concealing state that changes what the
 * user should do. Input order is preserved; the view-model already owns the
 * urgency/newness sort. Search and an explicit reveal show the complete set.
 */
export function visibleSidebarProjectRows(
  rows: readonly SidebarRow[],
  revealAll: boolean,
  searching: boolean,
  limit = SIDEBAR_PROJECT_ROW_LIMIT,
): SidebarProjectRows {
  const boundedLimit = Math.max(1, Math.floor(limit));
  if (revealAll || searching || rows.length <= boundedLimit) {
    return { rows: [...rows], hidden: 0 };
  }

  const visibleKeys = new Set(rows
    .filter(row => row.selected || row.badge !== "idle")
    .map(row => row.key));
  for (const row of rows) {
    if (visibleKeys.size >= boundedLimit) break;
    visibleKeys.add(row.key);
  }
  const visible = rows.filter(row => visibleKeys.has(row.key));
  return { rows: visible, hidden: rows.length - visible.length };
}

/**
 * The group header's ACCESSIBLE NAME, which is the whole point of this
 * function: a collapsed group's rows leave the DOM, so without this a screen
 * reader is told a project name and a number and nothing else — the app's most
 * important signal, hidden behind a twisty, was carried only by the colour of a
 * 6px dot (WCAG 1.4.1, Level A).
 *
 * The counts are the group's own, and `failed` is counted off the rows because
 * SidebarGroup.counts does not carry it. Counts intentionally stay in this
 * accessible name after leaving the visible resting header: collapsed content
 * still needs to say what it conceals.
 */
export function groupAccessibleName(group: SidebarGroup, parent = ""): string {
  const { total, waiting, working } = group.counts;
  const terminals = group.terminals ?? [];
  const failed = group.rows.filter(row => row.badge === "failed").length
    + terminals.filter(terminal => terminal.state === "failed").length;
  // The disambiguating parent segment when the header shows one, so the
  // accessible name still contains the visible label (WCAG 2.5.3) and two
  // projects that both display as "web" are still told apart by ear.
  const shown = parent === "" ? group.projectName : `${group.projectName} · ${parent}`;
  const parts = total > 0
    ? [`${shown}, ${total} session${total === 1 ? "" : "s"}`]
    : [shown];
  if (terminals.length > 0) {
    parts.push(`${terminals.length} terminal${terminals.length === 1 ? "" : "s"}`);
  }
  if (waiting > 0) parts.push(`${waiting} need${waiting === 1 ? "s" : ""} attention`);
  if (failed > 0) parts.push(`${failed} failed`);
  if (working > 0) parts.push(`${working} working`);
  return parts.join(", ");
}

/** Cmd/Ctrl-click splits; a plain click replaces the current pane. */
export function sessionRowActivateAction(
  event: { metaKey?: boolean; ctrlKey?: boolean },
  canSplit: boolean,
): "pane" | "select" {
  return canSplit && (event.metaKey === true || event.ctrlKey === true)
    ? "pane" : "select";
}

function readCollapsed(): string[] {
  try {
    const raw = window.localStorage.getItem(COLLAPSED_KEY);
    if (raw === null) return [];
    const parsed: unknown = JSON.parse(raw);
    // A corrupt or hand-edited key must not white-screen the app, so anything
    // that is not an array of strings degrades to "nothing collapsed".
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((p): p is string => typeof p === "string");
  } catch {
    return [];
  }
}

function writeCollapsed(paths: string[]): void {
  try {
    window.localStorage.setItem(COLLAPSED_KEY, JSON.stringify(paths));
  } catch {
    // Quota or a storage-less context: collapsing still works for this run, it
    // just does not survive a reload. Not worth surfacing to the user.
  }
}

/**
 * The header's fold-everything toggle: every listed project collapsed, or
 * every listed project open. Paths outside the listing (filtered out,
 * archived) keep their state, as they do for a single chevron.
 */
export function withAllProjectsCollapsed(collapsed: readonly string[],
    listed: readonly string[], next: boolean): string[] {
  return next
    ? [...collapsed, ...listed.filter(path => !collapsed.includes(path))]
    : collapsed.filter(path => !listed.includes(path));
}

function readArchivedProjects(): ArchivedProject[] {
  try {
    return parseArchivedProjects(window.localStorage.getItem(ARCHIVED_PROJECTS_KEY));
  } catch {
    return [];
  }
}

function writeArchivedProjects(projects: readonly ArchivedProject[]): void {
  try {
    window.localStorage.setItem(ARCHIVED_PROJECTS_KEY, JSON.stringify(projects));
  } catch {
    // The archive remains reversible for this run if persistence is blocked.
  }
}

// The parent directory of a project, used only to disambiguate two projects
// that share a basename. "" when there is no parent segment to show — which is
// also the answer for the "Other" bucket, whose sentinel path has no separator
// in it. That is the wanted behaviour if a real repository is ever named
// "Other": the repository gets the disambiguating parent, the bucket stays bare.
function parentSegment(projectPath: string): string {
  const segments = projectPath.split("/").filter(s => s !== "");
  return segments.length >= 2 ? segments[segments.length - 2] : "";
}

// Read off the view-model's own constant rather than restated, so the sentence
// the user reads can never drift from the cut that is actually applied — and
// stated EXACTLY, with no rounding step to drift through. KiB, not KB: the
// threshold is a whole number of 1024s (the view-model's tests hold it to that)
// and 1024s are what `ls -lh` and `du -h` count in, so the number here is the
// number the user sees when they go and look at the file.
const TRIVIAL_SIZE = `${TRIVIAL_SESSION_KIB} KiB`;

// How many skeleton rows to draw before the first listing arrives, and how wide
// each title bar is. A fixed, varied set rather than a single repeated width:
// identical bars read as a progress graphic, varied ones read as the list of
// session titles that is coming.
const SKELETON_TITLE_WIDTHS = ["72%", "54%", "83%", "61%", "46%", "76%"];

/**
 * A row with a one-way action armed on it, and which one.
 *
 *  - "delete"  the transcript goes from the engine's own store, for good.
 *  - "engine"  archived in the engine's store AND here. Reversible only from
 *              the vendor CLI; nothing in Interface can undo it.
 *
 * Hiding a session locally is NOT here: it is reversible from Library >
 * Archived, so both of its entry points spend one unconfirmed click.
 */
interface Confirm { key: string; kind: "delete" | "engine" }

type SidebarAuxMenu =
  | { kind: "create" }
  | { kind: "projects" }
  | { kind: "project"; projectPath: string }
  | { kind: "workspace"; workspaceId: string };


// Rules this component owns, kept beside the markup they style rather than in
// theme.css: the hover/focus-reveal affordances need :hover and :focus-within,
// which an inline style object cannot express, and the group chrome is only
// ever rendered here. Colours are tokens — no literal may appear in this file.
const SIDEBAR_CSS = `
/* The launcher's micro-label, so every surface that groups things speaks one
   vocabulary (owner, 2026-09-12). An empty Pinned section says so in the same
   line instead of spending a sentence and a divider on it. */
.sidebar-list-heading { display: flex; align-items: baseline; gap: 6px; margin: 10px 12px 6px; color: var(--fg-dim);
  font-size: var(--text-meta); font-weight: var(--wght-strong); letter-spacing: var(--track-caps); text-transform: uppercase; }
.sidebar-list-heading-note { color: var(--fg-faint); font-weight: var(--wght-regular); letter-spacing: 0; text-transform: none; }
.sidebar-folders-heading { padding-top: 12px; border-top: 1px solid var(--border); margin-top: 12px; }
.workspace-pinned-section > .session-items .workspace-tree-row { padding-left: 12px; }


/* Roomier (owner, 2026-10-02; dense 26px since 2026-10-01): 32px rows at the
   14px label size, in Bare CLI
   too. A palette
   may raise the selected row as its own surface (--sidebar-selected-fill, with
   --sidebar-selected-edge as its hairline); the others keep the neutral fill.
   A palette that raises the row also clears its parent's tint
   (--sidebar-parent-fill, owner 2026-09-28), so the project never outweighs
   the workspace selected inside it. */
.sidebar {
  --sidebar-row-h: 32px;
  --sidebar-row-selected: var(--sidebar-selected-fill, color-mix(in srgb, var(--fg) 19%, transparent));
  --sidebar-project-selected: var(--sidebar-parent-fill, color-mix(in srgb, var(--fg) 10%, transparent));
  --sidebar-label-size: var(--text-label);
  --sidebar-row-hover: color-mix(in srgb, var(--fg) 7%, transparent);
}
.workspace-tree-row.is-selected,
.sidebar .session-row.is-selected {
  box-shadow: 0 0 0 var(--border-w) var(--sidebar-selected-edge, transparent);
}

/* The full-window product rail now owns the hiddenInset drag region. The
   sidebar begins directly below it as a bounded workspace panel. */
.sidebar button,
.sidebar input { -webkit-app-region: no-drag; }

/* Settings stays below the scrolling workspace list, like an account row:
   one quiet row (owner, 2026-11-15). "Settings" on the left; while providers
   need setup, an amber dot and "N providers need setup" on its right, a
   second button in the same row that opens Models & providers. The version
   lives in Settings and the ⌘, hint in the tooltip. */
.sidebar-footer {
  flex: none;
  margin-top: auto;
  padding: 6px;
  border-top: 1px solid var(--border);
  container: sidebar-footer / inline-size;
}
/* The row carries the fill, so it reaches under the notice too. Settings keeps
   its whole word and takes the free width; the notice's words give way. */
.sidebar-footer-row {
  display: flex;
  align-items: center;
  min-height: 34px;
  border-radius: var(--radius-control);
}
.sidebar-footer-row:hover:has(> .sidebar-settings-button:not(:disabled)) { background: var(--sidebar-row-hover); }
.sidebar-footer-row:has(> .sidebar-settings-button[aria-current="page"]) { background: var(--sidebar-row-selected); }
.sidebar-settings-button {
  display: flex;
  flex: 1 0 auto;
  align-items: center;
  align-self: stretch;
  padding: 0 10px;
  border: 0;
  border-radius: var(--radius-control);
  background: transparent;
  color: var(--fg);
  font: inherit;
  text-align: left;
  cursor: pointer;
}
.sidebar-footer-row:hover > .sidebar-settings-button:not(:disabled),
.sidebar-settings-button[aria-current="page"] { color: var(--fg-strong); }
.sidebar-settings-button:focus-visible { outline: var(--focus-ring-w) solid var(--focus); outline-offset: calc(-1 * var(--focus-ring-offset)); }
.sidebar-settings-button:disabled { color: var(--fg-faint); cursor: default; }
.sidebar-settings-label { white-space: nowrap; }
/* Amber: something to set up, not a failure (owner, 2026-11-15; the danger
   tier before). The words give way first; the dot always stays. */
.sidebar-footer-alert {
  display: flex;
  flex: 0 1 auto;
  align-items: center;
  gap: 6px;
  min-width: 0;
  min-height: 26px;
  margin-right: 4px;
  padding: 0 6px;
  color: var(--status-waiting);
  background: transparent;
  border: 0;
  border-radius: var(--radius-control);
  font: inherit;
  font-size: var(--text-ui);
  text-align: left;
  cursor: pointer;
}
.sidebar-footer-alert:hover:not(:disabled) { text-decoration: underline; text-underline-offset: 2px; }
.sidebar-footer-alert:focus-visible { outline: var(--focus-ring-w) solid var(--focus); outline-offset: calc(-1 * var(--focus-ring-offset)); }
.sidebar-footer-alert:disabled { cursor: default; }
.sidebar-footer-alert-dot { flex: none; width: 6px; height: 6px; border-radius: var(--radius-pill); background: currentColor; }
.sidebar-footer-alert-label { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
/* A narrow row says the count in fewer words ("2 need setup"); Bare CLI's
   wider monospace needs that sooner. Narrower still, the dot alone. */
.sidebar-footer-alert-label.is-short { display: none; }
@container sidebar-footer (max-width: 240px) {
  .sidebar-footer-alert:has(> .is-short) > .sidebar-footer-alert-label:not(.is-short) { display: none; }
  .sidebar-footer-alert-label.is-short { display: inline; }
}
@container sidebar-footer (max-width: 300px) {
  :root[data-chrome="cli"] .sidebar-footer-alert:has(> .is-short) > .sidebar-footer-alert-label:not(.is-short) { display: none; }
  :root[data-chrome="cli"] .sidebar-footer-alert-label.is-short { display: inline; }
}
@container sidebar-footer (max-width: 180px) {
  .sidebar-footer-alert-label,
  .sidebar-footer-alert-label.is-short,
  :root[data-chrome="cli"] .sidebar-footer-alert-label.is-short { display: none; }
}

/* One header row: the pane's name, then its actions as quiet icon buttons.
   New chat, Filter, Project options and New workspace share one 24px shape;
   the shortcuts live in the tooltips. */
.sidebar-primary-nav {
  display: flex;
  flex: none;
  align-items: center;
  gap: 2px;
  height: 38px;
  padding: 4px 8px 2px 12px;
}
.sidebar-section-label {
  flex: 1 1 auto;
  min-width: 0;
  overflow: hidden;
  color: var(--fg);
  font-size: var(--text-ui);
  font-weight: var(--wght-strong);
  text-overflow: ellipsis;
  white-space: nowrap;
}
.sidebar-section-actions {
  display: flex;
  flex: none;
  align-items: center;
  gap: 2px;
}
.sidebar-nav-button,
.sidebar-section-action,
.sidebar-mobile-close {
  display: inline-flex;
  flex: none;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  padding: 0;
  font: inherit;
  line-height: 1;
  color: var(--fg-dim);
  background: transparent;
  border: 0;
  border-radius: var(--radius-control);
  cursor: pointer;
}
.sidebar-nav-button:hover,
.sidebar-section-action:hover:not(:disabled),
.sidebar-mobile-close:hover {
  color: var(--fg-strong);
  background: var(--row-hover);
}
.sidebar-section-action:disabled { color: var(--fg-faint); cursor: default; background: transparent; }
/* The app's ONE vertical-list selected look: a fill, plus the strong text
   tier. The accent hue belongs to the horizontal tab strips' underline. */
.sidebar-nav-button.is-active {
  color: var(--fg-strong);
  background: var(--row-selected);
}
.sidebar-nav-button.is-active .sidebar-nav-icon { color: var(--fg-strong); }
.sidebar-nav-button:first-child:not(.is-active) { color: var(--fg); }
.sidebar-nav-icon {
  display: inline-flex;
  flex: none;
  align-items: center;
  justify-content: center;
  width: var(--icon-size);
  height: var(--icon-size);
}
/* The button's name, for the accessibility tree; the glyph is the visible part. */
.sidebar-nav-label {
  position: absolute;
  width: 1px;
  height: 1px;
  padding: 0;
  margin: -1px;
  overflow: hidden;
  clip: rect(0, 0, 0, 0);
  white-space: nowrap;
  border: 0;
}
.sidebar .codicon-search::before { content: "\\ea6d"; }

/* Workspace rows are a nested tree under the project, not chat-row clones.
   Busy is the one extra state a workspace row can take; chat rows are never
   disabled. */
.sidebar .workspace-tree-row:disabled {
  color: var(--fg-faint);
  cursor: default;
  background: transparent;
}

/* Revealed like the row actions: on hover, on keyboard focus, or while open.
   A resting ⋯ on every project header was the busiest thing in the pane. */
.project-menu-button {
  display:inline-flex; flex:none; align-items:center; justify-content:center;
  position:absolute; right:6px;
  width:24px; height:24px; padding:0; color:var(--fg-dim);
  background:transparent; border:0; border-radius:var(--radius-control); cursor:pointer;
  opacity:0;
  transition:opacity var(--motion-fast) linear;
}
.group-header-row:hover .project-menu-button,
.group-header-row:focus-within .project-menu-button,
.project-menu-button.is-open { opacity:1; }
.project-menu-button:hover,
.project-menu-button:focus-visible,
.project-menu-button.is-open {
  color:var(--fg-strong); background:var(--row-hover);
}

.archived-projects {
  flex:none; margin:0 6px 6px; overflow:hidden; background:var(--surface);
  border:var(--border-w) solid var(--border); border-radius:var(--radius-container);
}
.archived-projects-header {
  display:flex; align-items:center; gap:6px; min-height:28px; padding:0 6px 0 8px;
  color:var(--fg-muted); font-size:var(--text-meta);
}
.archived-projects-title { color:var(--fg-strong); font-weight:var(--wght-strong); }
.archived-projects-spacer { flex:1 1 auto; }
.archived-projects-close {
  display:inline-flex; align-items:center; justify-content:center; width:24px;
  height:24px; padding:0; color:var(--fg-dim); background:transparent;
  border:0; border-radius:var(--radius-control); cursor:pointer;
}
.archived-projects-close:hover { color:var(--fg-strong); background:var(--row-hover); }
.archived-projects-list { margin:0; padding:0 4px 4px; list-style:none; }
.archived-project-row {
  display:grid; grid-template-columns:minmax(0, 1fr) auto; align-items:center; gap:6px;
  min-height:30px; padding:3px 4px; border-top:var(--border-w) solid var(--border);
}
.archived-project-copy { min-width:0; }
.archived-project-name, .archived-project-path {
  display:block; overflow:hidden; text-overflow:ellipsis; white-space:nowrap;
}
.archived-project-name { color:var(--fg); font-size:var(--text-ui); }
.archived-project-path { margin-top:1px; color:var(--fg-dim); font-size:var(--text-meta); }
.archived-project-restore {
  display:inline-flex; align-items:center; justify-content:center; height:var(--control-h);
  padding:0 7px; color:var(--fg-muted); background:transparent;
  border:var(--border-w) solid var(--border-strong); border-radius:var(--radius-control);
  font:inherit; font-size:var(--text-meta); cursor:pointer;
}
.archived-project-restore:hover { color:var(--fg-strong); background:var(--row-hover); }

/* flex: none is load-bearing. .session-list is a scrolling flex column, so a
   shrinkable group is compressed by its siblings once the content outgrows the
   pane and its <li> children spill over the next group instead of scrolling.
   .session-items needs no guard of its own: an unshrinkable group is sized by
   its content, so nothing inside it is ever under shrink pressure. */
.group { display: flex; flex: none; flex-direction: column; min-width: 0; }
.group + .group { margin-top: 10px; }

/* Project labels carry a quieter fill than the selected workspace. Sticky rows
   passing underneath are occluded by --chrome-bg in solid mode and clipped
   below the sticky label in glass mode. */
.group-header-row {
  position: sticky;
  top: 0;
  z-index: 2;
  display: flex;
  align-items: center;
  width: 100%;
  height: var(--sidebar-row-h);
  padding: 0;
  margin-bottom: 3px;
  background-color: var(--chrome-bg);
}
.group-header {
  display:flex; flex:1 1 auto; align-items:center; gap:7px; min-width:0; height:100%;
  padding:0 8px;
  font: inherit;
  font-size: var(--sidebar-label-size);
  font-weight: var(--wght-regular);
  color: var(--fg);
  text-align: left;
  background: transparent;
  border: 0;
  border-radius: var(--radius-container);
  cursor: pointer;
}
.group-header-row:hover {
  background-image: none;
}
.group-header-row:hover .group-header { color:var(--fg-strong); background:var(--sidebar-row-hover); }
.group-header.is-current {
  color:var(--fg-strong);
  background:var(--sidebar-project-selected);
  font-weight:var(--wght-strong);
}
.group-header-row:hover .group-header.is-current { background:var(--sidebar-parent-hover, var(--sidebar-project-selected)); }
.group-header-row:has(.project-menu-button):hover .group-count,
.group-header-row:has(.project-menu-button):focus-within .group-count { visibility:hidden; }
.group-chevron { color: var(--fg-dim);
  transition: transform var(--motion-base) var(--ease-out); }
.group-chevron.is-collapsed { transform: rotate(-90deg); }
.group-name {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
/* A disambiguating parent segment is text, so it stays --fg-dim rather than the
   non-text-only --fg-faint tier. */
.group-parent { flex: none; color: var(--fg-dim); }
.group-meta { display: flex; flex: none; align-items: center; gap:7px; margin-left: auto; }
/* .group-count shares the workspace count chip below. */
/* A collapsed group must never silently hide something that needs the user —
   and never by hue alone. Waiting and failed take the warning and error glyphs
   the rows already use, so the three states differ by SHAPE
   before they differ by colour; only the quiet "someone is working here" stays
   a dot. All three are aria-hidden because the header's accessible name states
   the same counts in words (groupAccessibleName), which is what a collapsed
   group owes a screen reader. */
.group-attention-waiting { flex: none; color: var(--status-waiting); }
.group-attention-failed { flex: none; color: var(--status-failed); }
.group-attention-working {
  flex: none;
  width: var(--dot-size);
  height: var(--dot-size);
  border-radius: 50%;
  background: var(--status-working);
}
.group-header-row .project-menu-button { margin-left:0; }

.project-confirm {
  margin:2px 4px 5px 8px; padding:7px; color:var(--fg); background:var(--surface);
  border:var(--border-w) solid var(--border-strong); border-radius:var(--radius-control);
  font-size:var(--text-meta); line-height:var(--lh-ui);
}
.project-confirm-name { color:var(--fg-strong); font-weight:var(--wght-strong); overflow-wrap:anywhere; }
.project-confirm-acts { display:flex; gap:6px; margin-top:7px; }

.session-items { list-style: none; margin: 0; padding: 0; }
/* A project's rows fold open to their natural height and shut to nothing, in
   step with the chevron's quarter turn. Clipped only while folding, so focus
   rings and hover fills are whole the rest of the time. A flex item's automatic
   minimum is its content height, even clipped, hence the explicit zero. */
.session-items.is-entering,
.session-items.is-leaving { min-height: 0; overflow: clip; interpolate-size: allow-keywords; }
.session-items.is-entering { animation: session-items-open var(--motion-base) var(--ease-out); }
.session-items.is-leaving {
  pointer-events: none;
  animation: session-items-shut var(--motion-base) var(--ease-out) both;
}
@keyframes session-items-open { from { height: 0; opacity: 0; } }
@keyframes session-items-shut { to { height: 0; opacity: 0; } }
@media (prefers-reduced-motion: reduce) {
  /* Not the global 0.01ms, whose first frame would paint the list folded. */
  .session-items.is-entering, .session-items.is-leaving { animation: none; }
}
.workspace-tree-item { margin: 0; padding: 0; list-style: none; }
.workspace-tree-row {
  display: flex;
  align-items: center;
  gap: 7px;
  width: 100%;
  height: var(--sidebar-row-h);
  padding: 0 8px 0 22px;
  font: inherit;
  font-size: var(--sidebar-label-size);
  font-weight: var(--wght-prose);
  color: var(--fg);
  text-align: left;
  background: transparent;
  border: 0;
  border-radius: var(--radius-container);
  cursor: pointer;
}
.workspace-tree-row:hover { color: var(--fg-strong); background: var(--sidebar-row-hover); }
.workspace-tree-row.is-selected {
  color: var(--fg-strong);
  background: var(--sidebar-row-selected);
  font-weight: var(--wght-strong);
}
.workspace-tree-ticker {
  display: grid;
  flex: none;
  grid-template-columns: repeat(2, 3px);
  grid-template-rows: repeat(3, 3px);
  gap: 2px;
  width: 8px;
  height: 13px;
  color: var(--fg);
}
.workspace-tree-ticker > i {
  background: currentColor;
  border-radius: calc(var(--radius-control) / 4);
  animation: workspace-ticker-step 1s steps(6) infinite;
  animation-play-state: var(--ambient-play-state);
}
.workspace-tree-ticker > i:nth-child(2) { animation-delay: -833ms; }
.workspace-tree-ticker > i:nth-child(4) { animation-delay: -667ms; }
.workspace-tree-ticker > i:nth-child(6) { animation-delay: -500ms; }
.workspace-tree-ticker > i:nth-child(5) { animation-delay: -333ms; }
.workspace-tree-ticker > i:nth-child(3) { animation-delay: -167ms; }
@keyframes workspace-ticker-step {
  0%, 100% { opacity: 1; }
  50% { opacity: .25; }
}
@media (prefers-reduced-motion: reduce) {
  .workspace-tree-ticker > i { animation: none; opacity: 1; }
}
.workspace-tree-title {
  flex: 1 1 auto;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
/* Count chip: a soft filled pill, so the digit is not a stray number. Tinted
   from the TEXT tier rather than --surface-raised, which sits within a point
   or two of the pane's own background in several themes and vanished. The
   digit stays --fg-dim: the count is furniture, not the row's title. The
   project header's pane total is the same fact one level up, so it wears the
   same chip. */
.group-count,
.workspace-tree-count {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  flex: none;
  min-width: 20px;
  height: 20px;
  padding: 0 6px;
  color: var(--fg-dim);
  font-size: var(--text-meta);
  font-weight: var(--wght-regular);
  font-variant-numeric: tabular-nums;
  line-height: 1;
  background: color-mix(in srgb, var(--fg-dim) 14%, transparent);
  border-radius: var(--radius-container);
  box-sizing: border-box;
}
.workspace-tree-row.is-selected .workspace-tree-count { color: var(--fg); }
/* A child row's count is plain digits: the chip stays on the project row, where
   it summarises a collapsed group (owner, 2026-09-12). Padding is kept so the
   digits still align with the project chip above. */
.workspace-tree-row .workspace-tree-count { background: transparent; }
/* One pip per pane in the workspace, in the pane's status colour, where the
   count used to be (owner, 2026-10-01): the row says what its panes are
   doing without being opened. Idle pips are the faint text tier. */
.workspace-tree-pips {
  display: inline-flex;
  flex: none;
  align-items: center;
  gap: 2px;
  height: 20px;
  padding: 0 2px 0 4px;
}
.workspace-tree-pips > i {
  width: 5px;
  height: 9px;
  border-radius: calc(var(--radius-control) / 2);
  background: color-mix(in srgb, var(--fg-dim) 38%, transparent);
}
/* An agent pane's pip is tinted by its provider (owner, 2026-10-02), so Codex
   and Claude read apart at a glance: working takes the provider's own colour,
   idle a faint wash of it. A plain shell has no provider, so its working pip
   is the neutral strong tier — the status orange read as Claude. Needs you,
   done and failed keep their status colours. */
.workspace-tree-pips > i[data-provider] {
  background: color-mix(in srgb, var(--provider-accent, var(--fg-dim)) 50%, transparent);
}
.workspace-tree-pips > i.is-working { background: var(--fg-muted); }
.workspace-tree-pips > i.is-working[data-provider] {
  background: var(--provider-accent, var(--status-working));
}
.workspace-tree-pips > i.is-waiting { background: var(--status-waiting); }
.workspace-tree-pips > i.is-done { background: var(--status-done); }
.workspace-tree-pips > i.is-failed { background: var(--status-failed); }
.workspace-tree-pips-more {
  margin-left: 2px;
  color: var(--fg-dim);
  font-size: var(--text-meta);
  font-variant-numeric: tabular-nums;
}
.workspace-tree-count-label {
  position: absolute;
  width: 1px;
  height: 1px;
  padding: 0;
  margin: -1px;
  overflow: hidden;
  clip: rect(0, 0, 0, 0);
  white-space: nowrap;
  border: 0;
}
.session-list-flat .workspace-tree-row { padding-left: 8px; }
.session-show-more { min-height:26px; padding:1px 4px 2px 47px; }
.session-show-more-button {
  width:100%; min-height:23px; padding:0 7px; color:var(--fg-dim);
  background:transparent; border:0; border-radius:var(--radius-control);
  font:inherit; font-size:var(--text-meta); text-align:left; cursor:pointer;
  transition:color var(--motion-fast) linear,
    background-color var(--motion-fast) linear;
}
.session-show-more-button:hover { color:var(--fg); background:var(--row-hover); }
.session-show-more-button:focus-visible { outline:var(--focus-ring-w) solid var(--focus); outline-offset:var(--focus-ring-offset); }
/* One project: no header row, so no nesting step either. */
.session-list-flat .session-row { padding-left: 8px; }
.sidebar .session-list { flex: 1 1 auto; padding: 0 8px 10px; }
.sidebar .session-row {
  height: var(--sidebar-row-h);
  gap: 8px;
  font-size: var(--sidebar-label-size);
  /* Two 24px action buttons, not one. Without the second slot a long title
     runs under the archive button the moment the row is hovered. */
  padding-right: calc(var(--icon-size) * 2 + 16px);
  padding-left: 26px;
  border-radius: var(--radius-container);
}

/* Keep the neutral selection visible while typing in the workspace. Specificity is
   (0,4,0) against theme.css's unconditional .session-row.is-selected at (0,2,0),
   which is what lets it win the paint without !important. */
.session-list:not(:focus-within) .session-row.is-selected {
  background: var(--sidebar-row-selected);
}
.sidebar .session-row.is-selected {
  background: var(--sidebar-row-selected);
  font-weight: var(--wght-strong);
}
.sidebar .session-row:hover:not(.is-selected) { background: var(--sidebar-row-hover); }

.session-item { position: relative; }
.session-fail { color: var(--status-failed); }
.session-wait { color: var(--status-waiting); }
.session-limit { color: var(--status-waiting); }

/* ---- needs attention ----------------------------------------------------
   The pinned section, and the treatment its rows carry.

   This is the pane's one job: say which agent is blocked on the user, where the
   eye already is. Four signals, and losing any one still leaves three — the
   the same discipline a waiting card gets:
     1. position — lifted out of its project group to the top of the pane, so
        no collapse toggle can hide it;
     2. a 2px rail down the row, in --status-waiting;
     3. a glyph that differs by SHAPE (warning triangle / error circle) and not
        only by hue;
     4. the WORDS, in a pill on the row itself, in every state — they used to
        exist only as a tooltip. */
.attention {
  display: flex;
  flex: none;
  flex-direction: column;
  max-height: 45%;
  padding: 0 8px 8px;
  overflow-y: auto;
  /* Its own scroller stacked over the session list: a wheel gesture that
     reaches its end must not carry on into the list underneath. */
  overscroll-behavior: contain;
}
/* Sticky, because this section scrolls inside itself once it is capped: the
   words have to stay on screen for the rows underneath them, which is the whole
   complaint against the tooltip they replace. Opaque --chrome-bg for the same
   reason the group header is opaque — rows pass underneath it. */
.attention-head {
  position: sticky;
  top: 0;
  z-index: 2;
  display: flex;
  flex: none;
  align-items: center;
  gap: 6px;
  height: 28px;
  padding: 0 4px;
  color: var(--fg-muted);
  background-color: var(--chrome-bg);
  font-size: var(--text-ui);
  font-weight: var(--wght-regular);
}
.attention-head-glyph { flex: none; color: var(--status-waiting); }
/* On .session-item rather than on the row button, because theme.css already
   spends the button's inset shadow on the selected row's accent rail — one of
   the two would have had to lose. z-index lifts it over the row's own fill,
   which is opaque in the light themes when selected. */
.session-item.is-waiting::before,
.session-item.is-failed::before {
  content: "";
  position: absolute;
  z-index: 1;
  top: 4px;
  bottom: 4px;
  left: 0;
  width: 2px;
  background: var(--status-waiting);
}
.session-item.is-failed::before { background: var(--status-failed); }

/* The word, on the row. Deliberately NOT coloured text: --status-waiting at
   11px over a selected row measures 3.6-4.0:1 across the light themes, under
   AA. The status colour carries on the 1px border and the rail, which are
   graphics and answer to 3:1; the word itself stays --fg-strong, which is AA on
   every fill this row can take, in every theme. */
.session-flag {
  display: inline-flex;
  flex: none;
  align-items: center;
  height: 16px;
  padding: 0 6px;
  color: var(--fg-strong);
  font-size: var(--text-meta);
  white-space: nowrap;
  background: transparent;
  border: var(--border-w) solid var(--border-strong);
  border-radius: var(--radius-control);
}
.session-flag-waiting { border-color: var(--status-waiting); }
.session-flag-failed { border-color: var(--status-failed); }

/* ---- loading ------------------------------------------------------------
   The shape of the rows that are coming, not a spinner and not a sentence: the
   same 26px row, the same 16px status column (it reuses theme.css's own .dot
   geometry), the same provider tag, the same title lane. "No sessions yet" is
   a claim, and on a cold start it is usually a false one. */
.session-skeleton {
  display: flex;
  align-items: center;
  gap: 7px;
  height: var(--row-h);
  padding: 0 24px;
}
/* Tinted from the TEXT tier, not from --surface-raised. In several themes the
   raised surface is within a point or two of the sidebar's own background, so a
   skeleton painted with it was invisible in every theme and exactly invisible
   in two — a loading state nobody could see. color-mix keeps it a hint of the
   text that is about to arrive, in whatever theme is active. */
.session-skeleton-block,
.session-skeleton-dot::before {
  background: color-mix(in srgb, var(--fg-dim) 22%, transparent);
  /* One skeleton pulse for the app (keyframes in theme.css): the transcript's
     placeholder runs the same name and the same 1.6s, so a sidebar loading
     beside a loading transcript breathes once rather than twice at two tempos. */
  animation: skeleton-pulse 1.6s steps(10) infinite;
  animation-play-state: var(--ambient-play-state);
}
.session-skeleton-block { border-radius: var(--radius-control); }
.session-skeleton-tag { flex: none; width: 16px; height: 16px; }
.session-skeleton-title { flex: 1 1 auto; height: 8px; }

/* ---- search -------------------------------------------------------------
   The Search destination reveals this field in place, rather than jumping to a
   second pane and losing the repository context beneath it. */
.sidebar-search {
  position: relative;
  display: flex;
  flex: none;
  flex-direction: column;
  align-items: stretch;
  padding: 0 6px 8px;
}
.sidebar-search-hint {
  margin: 5px 2px 0;
  color: var(--fg-dim);
  font-size: var(--text-meta);
  line-height: var(--lh-ui);
}
/* The hand-off row from a filter that matched nothing to the search that reads
   stored transcripts. One row, in the empty state, only while a query stands. */
.sidebar-search-handoff {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  max-width: 100%;
  min-height: var(--row-h);
  margin-top: 10px;
  padding: 0 9px;
  color: var(--fg);
  font: inherit;
  font-size: var(--text-ui);
  text-align: left;
  background: transparent;
  border: var(--border-w) solid var(--border-strong);
  border-radius: var(--radius-control);
  cursor: pointer;
}
.sidebar-search-handoff:hover { color: var(--fg-strong); background: var(--row-hover); }
.sidebar-search-input {
  width: 100%;
  height: 28px;
  padding: 0 28px 0 9px;
  font: inherit;
  font-size: var(--text-ui);
  color: var(--input-fg);
  background: var(--input-bg);
  border: var(--border-w) solid var(--input-border);
  border-radius: var(--radius-container);
}
.sidebar-search-input::placeholder { color: var(--input-placeholder); }
/* type="search" for the semantics, without Chromium's own clear glyph: this
   component draws its own, positioned in the token grid, and two ✕ in one field
   is a bug report. Electron is the only target, so the prefixed rule is enough. */
.sidebar-search-input::-webkit-search-cancel-button { display: none; }
.sidebar-search-clear {
  position: absolute;
  top: 2px;
  right: 10px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  padding: 0;
  font: inherit;
  line-height: 1;
  color: var(--fg-muted);     /* a control: --fg-faint is under the 3:1 minimum */
  background: transparent;
  border: 0;
  border-radius: var(--radius-control);
  cursor: pointer;
}
.sidebar-search-clear:hover { color: var(--fg-strong); }

/* Progress over ~170 sessions, one line, in the row the click came from. The
   bar is a graphic; the count beside it is the fact. */
.sidebar-progress {
  display: flex;
  flex: none;
  flex-direction: column;
  gap: 2px;
  padding: 4px 8px;
  border-bottom: var(--border-w) solid var(--border);
  font-size: var(--text-meta);
  color: var(--fg-dim);
}
.sidebar-progress-bar {
  height: 4px;
  overflow: hidden;
  background: var(--surface-raised);
  border-radius: var(--radius-control);
}
.sidebar-progress-fill { height: 100%; background: var(--accent); }

/* What the import did, and what the last action refused to do. Both are
   statements about something the user just asked for, so both are dismissible
   rather than timed: a sentence that vanishes on its own cannot be re-read. */
.sidebar-say {
  display: flex;
  flex: none;
  align-items: flex-start;
  gap: 6px;
  padding: 6px 6px 6px 8px;
  border-bottom: var(--border-w) solid var(--border);
  font-size: var(--text-meta);
  line-height: var(--lh-ui);
  color: var(--fg-muted);
}
.sidebar-say-error { color: var(--danger-fg); }
.sidebar-say-text { flex: 1 1 auto; min-width: 0; overflow-wrap: anywhere; }
.sidebar-say-close {
  display: inline-flex;
  flex: none;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  padding: 0;
  font: inherit;
  line-height: 1;
  color: var(--fg-muted);
  background: transparent;
  border: 0;
  border-radius: var(--radius-control);
  cursor: pointer;
}
.sidebar-say-close:hover { color: var(--fg-strong); background: var(--row-hover); }

/* ---- per-row actions ----------------------------------------------------
   TWO actions, split by REVERSIBILITY rather than by frequency. Archiving here
   hides the chat in Interface alone — the engine's own store is untouched and
   Library > Archived puts it back — so it is safe to spend one click on and it
   gets its own button, unconfirmed. Everything the app cannot undo (archiving
   in the engine's store, deleting the transcript) stays behind the "…" and
   behind a confirm, because the cost of a mis-click there is not symmetric.
   The row itself still belongs to its primary job: switching to the session. */
.session-menu,
.session-archive {
  position: absolute;
  top: 50%;
  transform: translateY(-50%);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  padding: 0;
  font: inherit;
  line-height: 1;
  /* --fg-faint is 2.63:1 over the selected row, a fail for a control;
     --fg-muted is 5.14:1 there. */
  color: var(--fg-muted);
  background: transparent;
  border: 0;
  border-radius: var(--radius-control);
  cursor: pointer;
  opacity: 0;
  transition: opacity var(--motion-fast) linear;
}
.session-menu { right: 4px; }
.session-archive { right: 28px; }
.session-item:hover .session-menu,
.session-item:focus-within .session-menu,
.session-item:hover .session-archive,
.session-item:focus-within .session-archive,
.session-archive:focus-visible,
.session-menu:focus-visible,
.session-menu.is-open { opacity: 1; }
.session-menu:hover, .session-menu.is-open,
.session-archive:hover { color: var(--fg); background: var(--row-hover); }

/* Portaled beside the ⋯, not inline under the row. An expander in this
   narrow scroller wraps every label, clips the delete confirm on the last
   rows, and reads as a second list instead of a menu. */
.row-menu {
  position: fixed;
  z-index: 100;
  display: flex;
  flex-direction: column;
  box-sizing: border-box;
  min-width: 200px;
  padding: 4px;
  color: var(--fg);
  background: var(--dropdown-bg);
  border: var(--border-w) solid var(--border-strong);
  border-radius: var(--radius-overlay);
  box-shadow: var(--overlay-shadow);
  font-family: var(--font-ui);
}
.row-menu-item {
  min-height: 28px;
  padding: 5px 10px;
  font: inherit;
  font-size: var(--text-ui);
  line-height: var(--lh-ui);
  color: var(--fg);
  text-align: left;
  background: transparent;
  border: 0;
  border-radius: var(--radius-control);
  cursor: pointer;
}
.row-menu-item:hover:not(:disabled) { background: var(--row-hover); }
.row-menu-item:disabled,
.row-menu-item[aria-disabled="true"] {
  color:var(--fg-faint); background:transparent; cursor:default;
}
.row-menu-danger { color: var(--danger-fg); }

/* Renaming happens in place: the row's own title becomes the field, so there is
   no dialog and no doubt about which session is being renamed. */
.session-rename {
  width: 100%;
  height: var(--row-h);
  padding: 0 6px 0 var(--indent);
  font: inherit;
  font-size: var(--text-body);
  color: var(--input-fg);
  background: var(--input-bg);
  border: var(--border-w) solid var(--focus);
  border-radius: var(--radius-control);
}
${SESSION_PEEK_CSS}
`;

function sidebarRowLists(
  groups: SidebarGroup[] | undefined,
  archivedProjects: readonly ArchivedProject[],
  protectedProjects: readonly string[],
  projectCount: number | undefined,
) {
  const protectedProjectSet = new Set(protectedProjects);
  const sourceGroupList = groups ?? [];
  const archivedPaths = new Set(archivedProjects
    .filter(project => !protectedProjectSet.has(project.projectPath))
    .map(project => project.projectPath));
  const groupList = sourceGroupList.filter(group => !archivedPaths.has(group.projectPath));
  // Counted BEFORE filtering so hiding a project never changes the kind of
  // list that is rendered.
  const archivedInListing = sourceGroupList.filter(
    group => archivedPaths.has(group.projectPath),
  ).length;
  const showHeaders = Math.max(0,
    (projectCount ?? sourceGroupList.length) - archivedInListing) > 0;
  const listedRows = groupList.flatMap(group => group.rows);
  const attentionWaitingRows = groupList
    .flatMap(group => group.rows.filter(row => row.badge === "waiting"));
  const attentionFailedRows = groupList
    .flatMap(group => group.rows.filter(row => row.badge === "failed"));
  const attentionRows = [...attentionWaitingRows, ...attentionFailedRows];
  let selectedRow: string | null = null;
  for (const g of groupList) {
    const hit = g.rows.find(r => r.selected);
    if (hit !== undefined) { selectedRow = `chat:${hit.key}`; break; }
  }
  const rowByKey = new Map(listedRows.map(row => [row.key, row]));
  return {
    protectedProjectSet, sourceGroupList, archivedPaths, groupList, showHeaders,
    attentionRows, selectedRow, rowByKey,
  };
}

type SidebarWorkspaceRecord = {
  readonly id: string;
  readonly name: string;
  readonly firstTerminalTitle?: string;
  readonly openTerminalCount: number;
  readonly openTerminalIds?: readonly string[];
  readonly terminalIds: readonly string[];
  readonly cwd?: string;
};

function sidebarWorkspaceIndex(
  groupList: readonly SidebarGroup[],
  records: readonly SidebarWorkspaceRecord[] | undefined,
  terminalProjects: Readonly<Record<string, string>>,
  archivedPaths: ReadonlySet<string>,
  pinnedProjects: readonly string[],
) {
  const workspaceProject = new Map<string, string>();
  // Only a chat-backed group is a repository root (shared/engine.ts:
  // `projectRoot`) and may claim saved cwds beneath it. A terminal-only group
  // can be the folder that merely CONTAINS projects — a shell launched in
  // ~/Desktop/Projects — and letting it own descendants filed every saved
  // workspace under that folder until its PTYs reopened. An exact match still
  // lands in that group through the cwd itself.
  const knownProjectPaths = [
    ...new Set([
      ...groupList.filter(group => group.rows.length > 0).map(group => group.projectPath),
      ...archivedPaths,
    ]),
  ].sort((left, right) => right.length - left.length);
  for (const record of records ?? []) {
    const project = workspaceSidebarProjectPath(
      record.terminalIds, terminalProjects, record.cwd, knownProjectPaths);
    if (project !== undefined && !archivedPaths.has(project)) {
      workspaceProject.set(record.id, project);
    }
  }
  const workspacesByProject = new Map<string, SidebarWorkspaceRecord[]>();
  for (const record of records ?? []) {
    const project = workspaceProject.get(record.id);
    if (project === undefined) continue;
    const owned = workspacesByProject.get(project);
    if (owned === undefined) workspacesByProject.set(project, [record]);
    else owned.push(record);
  }
  const listedProjectPaths = new Set(groupList.map(group => group.projectPath));
  const listedGroups = [...groupList];
  for (const project of [...workspaceProject.values(), ...pinnedProjects]) {
    if (listedProjectPaths.has(project) || archivedPaths.has(project)) continue;
    listedProjectPaths.add(project);
    listedGroups.push({
      projectPath: project,
      projectName: projectNameOf(project),
      rows: [],
      counts: { total: 0, waiting: 0, working: 0 },
    });
  }
  // A workspace-only project takes its alphabetical slot, not the tail.
  listedGroups.sort(compareSidebarGroups);
  return { workspaceProject, workspacesByProject, listedGroups };
}

/** A default `[]` literal would mint a new array per render and defeat the row memo. */
const NO_PROTECTED_PROJECTS: readonly string[] = [];

/** The session-row callbacks: one committed object whose members never change
 * identity (useCommittedActions), so a row re-renders only for its own props. */
type SidebarRowActions = {
  readonly openPeekFor: (key: string, element: HTMLElement) => void;
  readonly releasePeek: () => void;
  readonly showPeek: (key: string, anchor: DOMRect) => void;
  readonly hidePeek: () => void;
  readonly closeRowControls: () => void;
  readonly commitRename: (row: SidebarRow) => void;
  readonly restoreRowActionFocus: (key: string) => void;
  readonly onSelect: (key: string) => void;
  readonly onArchive: (key: string) => void;
  readonly onOpenInNewPane: (key: string) => void;
  readonly setRenameDraft: (draft: string) => void;
  readonly setRenameFor: (key: string | null) => void;
  readonly setMenuFor: (key: string | null) => void;
  readonly setConfirmFor: (confirm: Confirm | null) => void;
  readonly setAuxMenu: (menu: SidebarAuxMenu | null) => void;
};

export interface SidebarSessionRowProps {
  readonly row: SidebarRow;
  /** The engine's rate-limit message for this row, if any. */
  readonly limited: string | undefined;
  readonly menuOpen: boolean;
  /** Renaming in place; only ever true when lifecycle actions exist. */
  readonly renaming: boolean;
  /** The rename field's text while renaming, "" otherwise. */
  readonly renameDraft: string;
  readonly peeking: boolean;
  readonly showHeaders: boolean;
  readonly uid: string;
  readonly menuTriggers: { readonly current: Map<string, HTMLButtonElement> };
  readonly menuAnchor: { current: DOMRect | null };
  /** Present exactly when the caller can split: `actions.onOpenInNewPane`. */
  readonly onOpenInNewPane: ((key: string) => void) | undefined;
  readonly actions: SidebarRowActions;
}

/**
 * The fields of SidebarRow a session row draws or hands to an action. The
 * listing rebuilds every row object on each registry change, so the row memo
 * compares these instead of identity; a field the row starts reading must be
 * added here. lastActivity, status, external and worktree are the peek card's
 * and the menu's, which read the current row themselves.
 */
function sameDrawnRow(a: SidebarRow, b: SidebarRow): boolean {
  return a.key === b.key && a.title === b.title && a.engine === b.engine
    && a.badge === b.badge && a.cloud === b.cloud && a.selected === b.selected;
}

/** The row memo: the drawn row fields, and every other prop by identity. */
export function sameSidebarSessionRow(a: SidebarSessionRowProps,
    b: SidebarSessionRowProps): boolean {
  const names = Object.keys(a) as (keyof SidebarSessionRowProps)[];
  return names.length === Object.keys(b).length
    && names.every(name => name === "row" || Object.is(a[name], b[name]))
    && sameDrawnRow(a.row, b.row);
}

/**
 * One session row. Memoized: a hover, a rename keystroke or a streamed token
 * re-renders the Sidebar many times a second, and only the rows whose own
 * props changed are drawn again.
 */
export const SidebarSessionRow = memo(function SidebarSessionRow({ row: r, limited, menuOpen,
    renaming, renameDraft, peeking, showHeaders, uid, menuTriggers, menuAnchor, onOpenInNewPane,
    actions }: SidebarSessionRowProps): ReactNode {
  const { openPeekFor, releasePeek, showPeek, hidePeek, closeRowControls, commitRename,
    restoreRowActionFocus, onSelect, onArchive, setRenameDraft, setRenameFor, setMenuFor,
    setConfirmFor, setAuxMenu } = actions;
  const menuId = `${uid}-row-menu`;
  // Blocked-on-you and stopped are the two states this pane exists to
  // surface, so they are the two that get the rail, the shape and the word.
  const attention = isAttentionRow(r);
  const label = statusWord(r.badge);
  const peekId = `${uid}-peek`;
  return (
    // The action-menu button is a sibling of the row button, not a child:
    // nesting one button inside another is invalid HTML.
    <li className={attention ? `session-item is-${r.badge}` : "session-item"}>
      {renaming ? (
        // In place, on the row itself: no dialog, and no doubt about which
        // session is being renamed.
        <input
          className="session-rename"
          value={renameDraft}
          aria-label={`Rename ${r.title}`}
          autoFocus
          onChange={e => { setRenameDraft(e.target.value); }}
          // Blur CANCELS rather than commits: a rename writes to the
          // engine's own store, and a click anywhere else must not be a
          // silent write.
          onBlur={() => { setRenameFor(null); }}
          onKeyDown={e => {
            if (e.key === "Enter") {
              e.preventDefault();
              commitRename(r);
            } else if (e.key === "Escape") {
              e.preventDefault();
              setRenameFor(null);
            }
          }}
        />
      ) : (
        <>
          <button
            type="button"
            className={r.selected ? "session-row is-selected" : "session-row"}
            aria-current={r.selected || undefined}
            // The card DESCRIBES the row; it never renames it. Everything the
            // accessible name carries is still on the first line or in the
            // status glyph's own label.
            aria-describedby={peeking ? peekId : undefined}
            onPointerEnter={event => {
              if (event.pointerType === "touch") return;
              openPeekFor(r.key, event.currentTarget);
            }}
            onPointerLeave={releasePeek}
            onBlur={releasePeek}
            onClick={event => {
              // Picking a row puts every open row control away. A menu or a
              // delete confirmation left armed on the row above the one now
              // selected is a click away from acting on the wrong session.
              closeRowControls();
              const openPane = onOpenInNewPane;
              if (openPane !== undefined
                && sessionRowActivateAction(event, true) === "pane") {
                openPane(r.key);
                return;
              }
              onSelect(r.key);
            }}
            onKeyDown={e => {
              // Space is the preview, Enter is the session. Keyboard parity
              // with the hover card: the one gesture a pointer has that a
              // keyboard did not is a rest, so the focused row gets a key for
              // it. preventDefault is load-bearing — Space on a <button> would
              // otherwise fire click on keyup and switch session instead.
              if (e.key === " ") {
                e.preventDefault();
                if (peeking) hidePeek();
                else showPeek(r.key, e.currentTarget.getBoundingClientRect());
                return;
              }
              if (e.key === "Escape" && peeking) {
                e.preventDefault();
                hidePeek();
                return;
              }
              // ArrowLeft walks out to the group header, the way a tree row
              // walks out to its parent. A lifted row has no group header
              // above it, and `closest` returning null is the guard.
              if (e.key !== "ArrowLeft" || !showHeaders) return;
              const header = e.currentTarget
                .closest(".group")
                ?.querySelector<HTMLButtonElement>(".group-header");
              if (header === null || header === undefined) return;
              e.preventDefault();
              header.focus();
            }}
          >
            {attention ? (
              // Waiting and failed each deserve a SHAPE, not just a colour —
              // a triangle and a circled cross, distinguishable with no hue
              // at all. aria-hidden because the pill beside the title carries
              // the same word as text: exactly one announcement per row.
              <span
                className={r.badge === "failed"
                  ? "codicon codicon-error session-fail"
                  : "codicon codicon-warning session-wait"}
                aria-hidden="true"
                title={label}
              />
            ) : (
              <span
                className={r.badge === "working"
                  ? "dot dot-working dot-pulse"
                  : `dot dot-${r.badge}`}
                data-provider={providerLogoKind(r.engine)}
                role="img"
                aria-label={label}
                title={label}
              />
            )}
            {/* One line per row: the dot or glyph, the title, and the word
                for the two states that need the user. Provider, age, diff
                and worktree live on the hover card, not on the row. */}
            <span className="session-title" title={r.title}>{r.title}</span>
            {limited !== undefined && (
              // The engine's own wording as the tooltip — it knows when the
              // limit resets and we would only be paraphrasing it worse.
              <span
                className="codicon codicon-warning session-limit"
                role="img"
                aria-label="Rate limited"
                title={limited}
              />
            )}
            {r.cloud && (
              <span
                className="codicon codicon-cloud badge-cloud"
                role="img"
                aria-label="Cloud session"
                title="Cloud session"
              />
            )}
            {attention && (
              // The words, on the row, in every state. They used to be a
              // tooltip, which is no signal at all for the one fact this
              // pane exists to carry.
              <span className={`session-flag session-flag-${r.badge}`}>{label}</span>
            )}
          </button>
          <button
            type="button"
            className="session-archive"
            aria-label={`Hide ${r.title}`}
            title={`Hide — removes this chat from ${productLabel()} only. It stays in the engine's own store, and Library > Archived puts it back.`}
            onClick={e => {
              // No confirm, deliberately: this is the one row action the app
              // can undo by itself, and a dialog on every hide is what made
              // clearing the list a chore. The irreversible pair keep theirs.
              e.stopPropagation();
              setMenuFor(null);
              setConfirmFor(null);
              onArchive(r.key);
              restoreRowActionFocus(r.key);
            }}
          >
            {/* eye-closed, not the archive box: "Archive" is spent on the
                engine-store action in the ⋯ menu, and one glyph may not mean
                both the reversible hide and the irreversible archive. */}
            <span className="codicon codicon-eye-closed" aria-hidden="true" />
          </button>
          <button
            ref={node => {
              if (node === null) menuTriggers.current.delete(r.key);
              else menuTriggers.current.set(r.key, node);
            }}
            type="button"
            className={menuOpen ? "session-menu is-open" : "session-menu"}
            aria-label={`Actions for ${r.title}`}
            aria-haspopup="menu"
            aria-expanded={menuOpen}
            aria-controls={menuOpen ? menuId : undefined}
            title="Session actions"
            onClick={e => {
              e.stopPropagation();
              if (menuOpen) {
                setMenuFor(null);
                return;
              }
              menuAnchor.current = e.currentTarget.getBoundingClientRect();
              setConfirmFor(null);
              setAuxMenu(null);
              setMenuFor(r.key);
            }}
          >
            <span className="codicon codicon-ellipsis" aria-hidden="true" />
          </button>
        </>
      )}
    </li>
  );
}, sameSidebarSessionRow);

// Presentational only: no window.iface, no store — the caller owns session
// state and hands down groups plus callbacks. The one piece of state this
// component keeps is its own: which project groups are collapsed.
export function Sidebar(
  { groups, projectCount, protectedProjects = NO_PROTECTED_PROJECTS,
    pinnedProjects = NO_PROTECTED_PROJECTS, onPinProject,
    terminalProjects = {}, pinnedWorkspaces = NO_PROTECTED_PROJECTS, onPinWorkspace,
    loading = false,
    rateLimits, diffStats = {},
    peeks = {}, onSelect, agentMenu, terminalWorkspaces, terminalsActive = false,
    onOpenInNewPane, codeWorkspaces = false,
    onNew, onNewProject, newLabel = "New thread", sectionLabel = "Workspaces",
    zeroStateText, newChatActive = false,
    onNewInProject, onSearchTranscripts, onArchive,
    hidden, hideable, importable = 0, onToggleFilters, query, onQuery, searching,
    hiddenMatches, lifecycle, importer, focusOnMobileOpen = false, backgroundInert = false,
    onCloseMobile,
    settingsActive = false, onSettings, settingsDisabled = false, glass = false,
    footerAlert, onFooterAlert, threadInbox, threadLive, threadBridges }: SidebarProps,
) {
  const asideRef = useRef<HTMLElement>(null);
  const searchButtonRef = useRef<HTMLButtonElement>(null);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuTriggers = useRef(new Map<string, HTMLButtonElement>());
  const menuAnchor = useRef<DOMRect | null>(null);
  const [menuBox, setMenuBox] = useState<AppSelectPosition | null>(null);
  const auxMenuRef = useRef<HTMLDivElement>(null);
  const auxMenuTrigger = useRef<HTMLButtonElement | null>(null);
  const auxMenuAnchor = useRef<DOMRect | null>(null);
  const [auxMenuBox, setAuxMenuBox] = useState<AppSelectPosition | null>(null);
  const [auxMenu, setAuxMenu] = useState<SidebarAuxMenu | null>(null);
  const [collapsed, setCollapsed] = useState<string[]>(readCollapsed);
  const [revealedProjects, setRevealedProjects] = useState<string[]>([]);
  const [archivedProjects, setArchivedProjects] = useState<ArchivedProject[]>(readArchivedProjects);
  const [archivedProjectsOpen, setArchivedProjectsOpen] = useState(false);
  const [confirmProjectPath, setConfirmProjectPath] = useState<string | null>(null);
  const [searchOpen, setSearchOpen] = useState(() => (query ?? "") !== "");
  // Which row has its action menu open, which is being renamed, and which is
  // one click from being deleted. All three are single-valued on purpose: two
  // open menus, or a rename and a delete confirmation at once, would be two
  // pending decisions about two different sessions on this pane.
  const [menuFor, setMenuFor] = useState<string | null>(null);
  const [renameFor, setRenameFor] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  // WHICH consequential action is armed on which row, not just which row.
  // Delete is permanent, provider archive leaves this app, and local Hide may
  // close a live connection. All three therefore get an explicit second step.
  const [confirmFor, setConfirmFor] = useState<Confirm | null>(null);
  // The hover preview: which row it is describing, where it goes, and the two
  // timers that make it a considered gesture rather than a twitch.
  const [peekFor, setPeekFor] = useState<string | null>(null);
  const [peekBox, setPeekBox] = useState<SessionPeekPosition | null>(null);
  const peekRef = useRef<HTMLDivElement>(null);
  const peekAnchor = useRef<DOMRect | null>(null);
  const peekTimer = useRef<number | null>(null);
  // Whether a card is currently on screen. It is a ref and not the state above
  // because the pointerenter of the next row has to read it in the same tick
  // the previous row's leave scheduled, and a re-render is a tick too late.
  const peekShowing = useRef(false);
  const uid = useId();
  const {
    protectedProjectSet, sourceGroupList, archivedPaths, groupList, showHeaders: sourceShowHeaders,
    attentionRows, selectedRow, rowByKey,
  } = useMemo(
    () => sidebarRowLists(groups, archivedProjects, protectedProjects, projectCount),
    [groups, archivedProjects, protectedProjects, projectCount],
  );
  const { workspacesByProject, listedGroups } = useMemo(
    () => sidebarWorkspaceIndex(
      groupList, terminalWorkspaces?.records, terminalProjects, archivedPaths,
      searching || (query ?? "").trim() !== "" ? NO_PROTECTED_PROJECTS : pinnedProjects),
    [groupList, terminalWorkspaces?.records, terminalProjects, archivedPaths, pinnedProjects, searching, query],
  );

  const showHeaders = sourceShowHeaders || listedGroups.length > 0;
  const duplicateNames = useMemo(() => {
    const seen = new Set<string>();
    return new Set(listedGroups.filter(group => {
      if (seen.has(group.projectName)) return true;
      seen.add(group.projectName);
      return false;
    }).map(group => group.projectName));
  }, [listedGroups]);

  useEffect(() => {
    const updated = withoutProtectedArchivedProjects(archivedProjects, protectedProjectSet);
    if (updated.length === archivedProjects.length) return;
    writeArchivedProjects(updated);
    setArchivedProjects(updated);
    if (updated.length === 0) setArchivedProjectsOpen(false);
  }, [archivedProjects, protectedProjectSet]);
  useEffect(() => {
    if (!focusOnMobileOpen) return;
    asideRef.current?.querySelector<HTMLElement>("button, input, [tabindex]")?.focus();
  }, [focusOnMobileOpen]);

  useEffect(() => {
    if (!searchOpen) return;
    searchInputRef.current?.focus();
  }, [searchOpen]);

  useEffect(() => {
    if (onQuery === undefined || settingsActive) return;
    const openSearch = (event: KeyboardEvent): void => {
      if (!(event.metaKey || event.ctrlKey) || event.shiftKey
        || event.altKey || event.key.toLowerCase() !== "k") return;
      // Ctrl+K is kill-to-end-of-line inside a shell, and this is a WINDOW
      // listener, so without the same terminal test App's handler makes it
      // would open this field over a pane the user is typing in. ⌘K is still
      // the app's everywhere, terminals included.
      if (shellOwnsChord(event, event.target instanceof Element
        && event.target.closest(".terminal-center") !== null)) return;
      if (document.querySelector(FOREGROUND_DIALOG_SELECTOR) !== null) return;
      event.preventDefault();
      setSearchOpen(true);
    };
    window.addEventListener("keydown", openSearch);
    return () => { window.removeEventListener("keydown", openSearch); };
  }, [onQuery, settingsActive]);

  const restoreRowActionFocus = (key: string): void => {
    requestAnimationFrame(() => {
      const exact = menuTriggers.current.get(key);
      if (exact?.isConnected === true) {
        exact.focus();
        return;
      }
      asideRef.current?.querySelector<HTMLElement>(
        ".session-row.is-selected, .session-row, .sidebar-nav-button",
      )?.focus();
    });
  };

  useAnchoredOverlay(menuFor, menuRef, menuAnchor, setMenuBox, positionAppSelect, 200);

  useEffect(() => {
    if (menuFor === null || menuBox === null) return;
    // The focused item becomes the menu's one Tab stop, so it must be one
    // that can take focus: a disabled first item would leave it with none.
    menuRef.current?.querySelector<HTMLButtonElement>(
      '[role="menuitem"]:not(:disabled)',
    )?.focus();
  }, [menuBox, menuFor]);

  useAnchoredOverlay(auxMenu, auxMenuRef, auxMenuAnchor, setAuxMenuBox,
    positionAppSelect, 200);

  useEffect(() => {
    if (auxMenu === null || auxMenuBox === null) return;
    // Positioning removes visibility:hidden, but the item can inherit the old
    // value for a frame. Focus only once it is visible, while the opener still
    // owns focus; never steal it back after the user moves elsewhere.
    const openerFocus = document.activeElement;
    const deadline = performance.now() + 1000;
    let frame = 0;
    const focusVisibleItem = (): void => {
      if (document.activeElement !== openerFocus) return;
      const item = auxMenuRef.current?.querySelector<HTMLButtonElement>(
        '[role="menuitem"]:not(:disabled)',
      );
      if (item && getComputedStyle(item).visibility === "visible") item.focus();
      else if (performance.now() < deadline) frame = requestAnimationFrame(focusVisibleItem);
    };
    frame = requestAnimationFrame(focusVisibleItem);
    return () => cancelAnimationFrame(frame);
  }, [auxMenu, auxMenuBox]);

  useEffect(() => {
    if (auxMenu === null) return;
    const onPointer = (event: PointerEvent): void => {
      const target = event.target;
      if (!(target instanceof Node)) return;
      if (auxMenuRef.current?.contains(target) === true) return;
      if (auxMenuTrigger.current?.contains(target) === true) return;
      setAuxMenu(null);
    };
    const onKey = (event: KeyboardEvent): void => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      const trigger = auxMenuTrigger.current;
      setAuxMenu(null);
      requestAnimationFrame(() => { trigger?.focus(); });
    };
    window.addEventListener("pointerdown", onPointer);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("pointerdown", onPointer);
      window.removeEventListener("keydown", onKey);
    };
  }, [auxMenu]);

  useEffect(() => {
    if (menuFor === null) return;
    const onPointer = (event: PointerEvent): void => {
      const target = event.target;
      if (!(target instanceof Node)) return;
      if (menuRef.current?.contains(target) === true) return;
      if (target instanceof Element && target.closest(".session-menu") !== null) return;
      setMenuFor(null);
    };
    const onKey = (event: KeyboardEvent): void => {
      if (event.key === "Escape") {
        event.preventDefault();
        const key = menuFor;
        setMenuFor(null);
        restoreRowActionFocus(key);
      }
    };
    window.addEventListener("pointerdown", onPointer);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("pointerdown", onPointer);
      window.removeEventListener("keydown", onKey);
    };
  }, [menuFor]);

  const clearPeekTimer = (): void => {
    if (peekTimer.current === null) return;
    window.clearTimeout(peekTimer.current);
    peekTimer.current = null;
  };

  const showPeek = (key: string, anchor: DOMRect): void => {
    // FIRST, always. Whatever this card was opened by, a close scheduled by an
    // earlier pointer-leave is still armed — and a card opened with Space
    // seconds later would be dismissed by it 120ms after it appeared.
    clearPeekTimer();
    peekAnchor.current = anchor;
    peekShowing.current = true;
    setPeekBox(null);
    setPeekFor(key);
  };

  const hidePeek = (): void => {
    clearPeekTimer();
    peekShowing.current = false;
    setPeekFor(null);
  };

  /**
   * The pointer has come to rest on a row. The first card costs a deliberate
   * pause; every card after it, while one is still open, is instant — crossing
   * the list to compare two agents must not make the user wait twice.
   */
  const openPeekFor = (key: string, element: HTMLElement): void => {
    clearPeekTimer();
    const anchor = element.getBoundingClientRect();
    if (peekShowing.current) {
      showPeek(key, anchor);
      return;
    }
    peekTimer.current = window.setTimeout(() => {
      peekTimer.current = null;
      showPeek(key, anchor);
    }, SESSION_PEEK_HOVER_MS);
  };

  // Deferred, not immediate: the row being left fires before the row being
  // entered, so an instant close would end the preview run on every step down
  // the list and charge the full delay again for the next row.
  const releasePeek = (): void => {
    clearPeekTimer();
    peekTimer.current = window.setTimeout(() => {
      peekTimer.current = null;
      hidePeek();
    }, SESSION_PEEK_LEAVE_MS);
  };

  useEffect(() => clearPeekTimer, []);

  useAnchoredOverlay(peekFor, peekRef, peekAnchor, setPeekBox, positionSessionPeek);

  useEffect(() => {
    if (peekFor === null) return;
    const onKey = (event: KeyboardEvent): void => {
      if (event.key !== "Escape") return;
      hidePeek();
    };
    window.addEventListener("keydown", onKey);
    return () => { window.removeEventListener("keydown", onKey); };
  }, [peekFor]);

  // Scrolling moves the row out from under its own card, so the rect this was
  // measured against stops being true the moment the list moves.
  //
  // UNCONDITIONAL, and it clears the timer as well as the card. The rect is
  // captured at pointerenter and the card opens 350ms later; a scroll inside
  // that window would otherwise place the card against a row that has since
  // moved — and there is no open card at that instant for a peekFor-guarded
  // listener to have been registered for.
  // Only scrolls that can move a row count — the sidebar's own lists and its
  // ancestors. A streaming transcript scrolls itself every frame and would
  // otherwise keep the card from ever opening.
  useEffect(() => {
    const onScroll = (event: Event): void => {
      const target = event.target;
      const inSidebar = target instanceof Node && asideRef.current?.contains(target) === true;
      if (!inSidebar && !scrollMovesAnchor(target, asideRef.current)) return;
      clearPeekTimer();
      hidePeek();
    };
    window.addEventListener("scroll", onScroll, true);
    return () => { window.removeEventListener("scroll", onScroll, true); };
  }, []);

  // One exit from every open row control, so Escape means the same thing
  // everywhere and no action is left half-armed behind a click elsewhere.
  const closeRowControls = (): void => {
    setMenuFor(null);
    setConfirmFor(null);
    setRenameFor(null);
    setConfirmProjectPath(null);
    setAuxMenu(null);
    hidePeek();
  };

  const openAuxMenu = (next: SidebarAuxMenu, trigger: HTMLButtonElement,
      pointer?: { clientX: number; clientY: number }): void => {
    const sameMenu = auxMenu?.kind === next.kind
      && (next.kind === "projects" || next.kind === "create"
        || (next.kind === "project" && auxMenu.kind === "project"
          && auxMenu.projectPath === next.projectPath)
        || (next.kind === "workspace" && auxMenu.kind === "workspace"
          && auxMenu.workspaceId === next.workspaceId));
    // A second right-click on the same workspace repositions rather than
    // toggling shut — that is how a context menu behaves.
    if (sameMenu && next.kind !== "workspace") {
      setAuxMenu(null);
      return;
    }
    setMenuFor(null);
    setConfirmFor(null);
    setRenameFor(null);
    setConfirmProjectPath(null);
    hidePeek();
    auxMenuTrigger.current = trigger;
    auxMenuAnchor.current = pointer === undefined
      ? trigger.getBoundingClientRect()
      : new DOMRect(pointer.clientX, pointer.clientY, 0, 0);
    setAuxMenuBox(null);
    setAuxMenu(next);
  };

  const startRename = (row: SidebarRow): void => {
    setMenuFor(null);
    setConfirmFor(null);
    setRenameDraft(row.title);
    setRenameFor(row.key);
  };

  const commitRename = (row: SidebarRow): void => {
    const title = renameDraft.trim();
    setRenameFor(null);
    restoreRowActionFocus(row.key);
    // An empty box is a cancel, not a request to erase the title, and a title
    // that did not change is not worth a write to the engine's store.
    if (title === "" || title === row.title || lifecycle === undefined) return;
    lifecycle.onRename(row.key, title);
  };

  const applyRowConfirm = (): void => {
    if (confirmFor === null) return;
    const { key, kind } = confirmFor;
    setConfirmFor(null);
    if (lifecycle === undefined) return;
    if (kind === "engine") lifecycle.onEngineArchive(key);
    else lifecycle.onDelete(key);
    restoreRowActionFocus(key);
  };

  // Every listed project remains a visible workspace row, even when there is
  // only one. The row is the stable owner for both chats and terminals and
  // prevents the hierarchy from changing shape as a second project appears.
  // The rows are LIFTED, not copied: each renders once, so one ⋯ menu, one
  // rename field and one menu-trigger ref per session, and no row is ever shown
  // twice in a 288px pane.
  const lifting = !loading && attentionRows.length > 0 && threadInbox === undefined;
  const menuRow = menuFor === null ? null : rowByKey.get(menuFor) ?? null;
  // A row that has left the listing — archived, filtered out, or renamed away
  // under an open card — takes its preview with it rather than leaving a card
  // describing a session that is no longer on screen.
  const peekRow = peekFor === null ? null : rowByKey.get(peekFor) ?? null;
  const confirmRow = confirmFor === null
    ? null : rowByKey.get(confirmFor.key) ?? null;
  const menuOps = menuRow === null || lifecycle === undefined ? null : {
    rename: lifecycle.can(menuRow.engine, "renameSession"),
    fork: lifecycle.can(menuRow.engine, "forkSession"),
    remove: lifecycle.can(menuRow.engine, "deleteSession"),
    adopt: lifecycle.can(menuRow.engine, "adoptSession"),
    archive: lifecycle.can(menuRow.engine, "setArchived"),
  };

  // The filter command's copy and the zero-state's, both derived in the view-model
  // from the counts this render was built with, where they are tested. The
  // search is passed on so an empty pane answers the question the user just
  // asked rather than explaining a filter they did not touch.
  const hiddenTotal = hidden.external + hidden.trivial;
  const filterLabel = sidebarFilterLabel(hidden);
  const allListedProjectsArchived = sourceGroupList.length > 0
    && groupList.length === 0
    && sourceGroupList.every(group => archivedPaths.has(group.projectPath));
  const showSearch = onQuery !== undefined && (searchOpen || (query ?? "") !== "");
  const filterQuery = (query ?? "").trim();
  const empty = sidebarEmptyState(hidden, { searching, hiddenMatches });
  // Only the true zero-state belongs to a surface. Filter and search copy
  // continues to describe the filtering fact the view-model established.
  const emptyText = empty.action === "new" && zeroStateText !== undefined
    ? zeroStateText : empty.text;
  const emptyHint = empty.hint;
  const auxProject = auxMenu?.kind === "project"
    ? listedGroups.find(group => group.projectPath === auxMenu.projectPath) ?? null
    : null;
  const auxWorkspace = auxMenu?.kind === "workspace"
    ? terminalWorkspaces?.records.find(record => record.id === auxMenu.workspaceId) ?? null
    : null;
  // The inbox lists no project groups, so project archive has nothing to restore there.
  const showArchivedProjects = threadInbox === undefined && archivedProjects.length > 0;
  const hasProjectsMenu = showArchivedProjects || hideable > 0
    || (importer !== undefined && importable > 0 && !importer.running);

  const setGroupCollapsed = (projectPath: string, next: boolean): void => {
    if (collapsed.includes(projectPath) === next) return;
    const updated = next
      ? [...collapsed, projectPath]
      : collapsed.filter(p => p !== projectPath);
    writeCollapsed(updated);
    setCollapsed(updated);
  };
  // Folded when every listed project is; one open project offers Collapse all.
  const allGroupsCollapsed = listedGroups.length > 0
    && listedGroups.every(group => collapsed.includes(group.projectPath));
  const groupNoun = codeWorkspaces ? "folders" : "projects";
  const setAllGroupsCollapsed = (next: boolean): void => {
    const updated = withAllProjectsCollapsed(collapsed,
      listedGroups.map(group => group.projectPath), next);
    writeCollapsed(updated);
    setCollapsed(updated);
  };

  const archiveProject = (group: SidebarGroup): void => {
    // Recheck after confirmation so work that started while the dialog was open
    // remains visible instead of being concealed by a stale decision.
    if (projectArchiveBlocked(group, protectedProjectSet)) {
      setConfirmProjectPath(null);
      return;
    }
    const updated = withArchivedProject(archivedProjects, {
      projectPath: group.projectPath,
      projectName: group.projectName,
    });
    writeArchivedProjects(updated);
    setArchivedProjects(updated);
    setConfirmProjectPath(null);
  };

  const restoreProject = (projectPath: string): void => {
    const updated = withoutArchivedProject(archivedProjects, projectPath);
    writeArchivedProjects(updated);
    setArchivedProjects(updated);
    if (updated.length === 0) setArchivedProjectsOpen(false);
  };

  // The selection arrives from outside this component, so the force-expand
  // watches the selected key rather than a click here: a group that receives
  // the selection opens, and the change persists. It is deliberately NOT a
  // render-time override — that would make the chevron inert on the one group
  // the user is looking at. Collapsing a group by hand after selecting inside
  // it is allowed and sticks, exactly as collapsing a folder over the active
  // file does in VS Code; it reopens the next time the selection moves in.
  //
  // RATIFIED DEVIATION from brief §6, which asked for a render-time override
  // ("a group containing the selection renders expanded"). Reviewed and kept as
  // built — the brief was amended, not this code. Do not "fix" it back: a
  // render-time override re-derives expansion on every render and so makes the
  // chevron dead on precisely the group the user is looking at.
  // undefined, not null: the first pass after mount must count as a change, so
  // a restored selection is never left hidden under a persisted collapse.
  const seenSelection = useRef<string | null | undefined>(undefined);
  useEffect(() => {
    if (seenSelection.current === selectedRow) return;
    seenSelection.current = selectedRow;
    if (selectedRow === null) return;
    const owner = groupList.find(g => g.rows.some(r => `chat:${r.key}` === selectedRow));
    if (owner === undefined || !collapsed.includes(owner.projectPath)) return;
    const updated = collapsed.filter(p => p !== owner.projectPath);
    writeCollapsed(updated);
    setCollapsed(updated);
  }, [groupList, collapsed, selectedRow]);

  // Every row shares one actions object whose members call this render's
  // callbacks, so a new closure per render never redraws a memoized row.
  const rowActions = useCommittedActions<SidebarRowActions>({
    openPeekFor, releasePeek, showPeek, hidePeek, closeRowControls, commitRename,
    restoreRowActionFocus, onSelect, onArchive,
    onOpenInNewPane: key => { onOpenInNewPane?.(key); },
    setRenameDraft, setRenameFor, setMenuFor, setConfirmFor, setAuxMenu,
  });

  /**
   * ONE renderer for both places a session row can appear: inside its project
   * group, and lifted into the pinned "Needs attention" section above them.
   *
   * A lifted row is not a shortcut copy of the row — it IS the row, rendered
   * somewhere else — which is what keeps every per-row control single-valued:
   * one ⋯ trigger in `menuTriggers`, one rename field, one `aria-current`. A
   * blocked agent therefore loses no lifecycle action by being lifted, and no
   * session is ever drawn twice in a 288px pane.
   */
  const renderSessionRow = (r: SidebarRow) => (
    <SidebarSessionRow key={r.key} row={r} limited={rateLimits[r.key]}
      menuOpen={menuFor === r.key}
      renaming={renameFor === r.key && lifecycle !== undefined}
      renameDraft={renameFor === r.key ? renameDraft : ""}
      peeking={peekFor === r.key} showHeaders={showHeaders} uid={uid}
      menuTriggers={menuTriggers} menuAnchor={menuAnchor}
      onOpenInNewPane={onOpenInNewPane === undefined ? undefined : rowActions.onOpenInNewPane}
      actions={rowActions} />
  );

  // A workspace is filed with the project that owns its first terminal. The
  // unfiltered terminal projection already resolves session lineage and nested
  // cwd paths to one canonical project; reuse that stable answer instead of
  // deriving ownership from search-filtered rows. Only project-owned records
  // appear in the sidebar's existing Workspaces section.
  const renderWorkspaceRow = (record: SidebarWorkspaceRecord,
      terminals: readonly SidebarTerminalRow[]) => {
    if (terminalWorkspaces === undefined) return null;
    const current = terminalsActive && record.id === terminalWorkspaces.activeId;
    const terminalById = new Map(terminals.map(terminal => [terminal.id, terminal]));
    const members = record.terminalIds
      .map(id => terminalById.get(id))
      .filter((terminal): terminal is SidebarTerminalRow => terminal !== undefined);
    const badge = workspaceSidebarBadge(members);
    const done = workspaceSidebarDone(members);
    const displayTitle = workspaceSidebarTitle(
      record.name, members, record.firstTerminalTitle,
    );
    const label = done ? DONE_WORD : statusWord(badge);
    const count = record.openTerminalCount;
    // One pip per open pane (owner, 2026-10-01): a closed tab's terminal is
    // not a pane on screen, so it never takes a pip; the row's own glyph
    // still says when a hidden one needs you.
    const pips = workspaceSidebarPips(record.openTerminalIds === undefined ? members
      : record.openTerminalIds.map(id => terminalById.get(id))
        .filter((terminal): terminal is SidebarTerminalRow => terminal !== undefined));
    return (
      <li key={`workspace:${record.id}`} className="workspace-tree-item">
        <button
          type="button"
          className={current ? "workspace-tree-row is-selected" : "workspace-tree-row"}
          data-terminal-workspace-id={record.id}
          aria-current={current ? "page" : undefined}
          title={displayTitle}
          disabled={terminalWorkspaces.busy}
          onClick={() => {
            closeRowControls();
            terminalWorkspaces.onSelect(record.id);
          }}
          onDoubleClick={() => { terminalWorkspaces.onRename(record.id); }}
          onContextMenu={event => {
            if (terminalWorkspaces.busy) return;
            event.preventDefault();
            event.stopPropagation();
            openAuxMenu({ kind: "workspace", workspaceId: record.id },
              event.currentTarget, event);
          }}
          onKeyDown={event => {
            if (terminalWorkspaces.busy) return;
            if (event.key === "ContextMenu"
              || (event.key === "F10" && event.shiftKey)) {
              event.preventDefault();
              event.stopPropagation();
              openAuxMenu({ kind: "workspace", workspaceId: record.id },
                event.currentTarget);
            }
          }}
        >
          {badge === "failed" || badge === "waiting" ? (
            <span
              className={badge === "failed"
                ? "codicon codicon-error session-fail"
                : "codicon codicon-warning session-wait"}
              role="img"
              aria-label={label}
              title={label}
            />
          ) : (
            <span
              className={badge === "working"
                ? "dot dot-working"
                : done ? "dot dot-done" : "dot dot-idle"}
              role="img"
              aria-label={label}
              title={label}
            />
          )}
          {badge === "working" && (
            <span className="workspace-tree-ticker" aria-hidden="true"><i /><i /><i /><i /><i /><i /></span>
          )}
          <span className="workspace-tree-title">{displayTitle}</span>
          {pips.shown.length > 0 ? (
            <span className="workspace-tree-pips" aria-hidden="true">
              {pips.shown.map((pip, at) => (
                <i key={at} className={`is-${pip.light}`}
                  data-provider={pip.profileId === undefined ? undefined : providerLogoKind(pip.profileId)} />
              ))}
              {pips.more > 0 && <span className="workspace-tree-pips-more">+{pips.more}</span>}
            </span>
          ) : <span className="workspace-tree-count" aria-hidden="true">{count}</span>}
          <span className="workspace-tree-count-label">
            {count} open terminal{count === 1 ? "" : "s"}
          </span>
        </button>
      </li>
    );
  };

  const renderProject = (g: SidebarGroup, i: number): ReactNode => {
    // Keys, collapsed state and every lookup ride on projectPath —
    // projectName is a display label and is not unique.
    const listId = `${uid}-g${i}`;
    const isCollapsed = showHeaders && collapsed.includes(g.projectPath);
    const parent = duplicateNames.has(g.projectName)
      ? parentSegment(g.projectPath)
      : "";
    // What the group still lists once its blocked and failed rows have
    // been lifted to the pinned section. The accessible name retains
    // the full project total and says how many need the user even though
    // the resting visual header deliberately shows neither aggregate.
    const groupRows = lifting ? g.rows.filter(row => !isAttentionRow(row)) : g.rows;
    const visibleGroup = visibleSidebarProjectRows(
      groupRows,
      revealedProjects.includes(g.projectPath),
      searching === true || (query ?? "").trim() !== "",
    );
    const failedHere = g.rows.filter(row => row.badge === "failed").length
      + (g.terminals?.filter(terminal => terminal.state === "failed").length ?? 0);
    const terminalWaiting = g.terminals?.some(terminal =>
      (terminal.state === "starting" || terminal.state === "running")
      && terminal.activity === "waiting") === true;
    const terminalStarting = g.terminals?.some(terminal =>
      terminal.activity === "working") === true;
    const projectWorkspaces = workspacesActiveFirst(
      workspacesByProject.get(g.projectPath) ?? [], g.terminals ?? []);
    const isCurrent = g.rows.some(row => row.selected)
      || (terminalsActive && projectWorkspaces.some(record => record.id === terminalWorkspaces?.activeId));
    const groupItemCount = terminalWorkspaces !== undefined
      ? projectWorkspaces.reduce((sum, record) => sum + record.openTerminalCount, 0)
      : g.counts.total;
    const attention = g.counts.waiting > 0 || terminalWaiting
      ? "waiting"
      : failedHere > 0
        ? "failed"
        : g.counts.working > 0 || terminalStarting ? "working" : null;
    return (
      <div key={g.projectPath} className="group" data-sidebar-project={g.projectPath}>
        {showHeaders && (
          <div className="group-header-row">
            <button
              type="button"
              className={isCurrent ? "group-header is-current" : "group-header"}
              aria-expanded={!isCollapsed}
              aria-controls={listId}
              // A collapsed group's rows leave the DOM, so everything a
              // screen reader can learn about what is inside it has to
              // be in this one string. Without it the app's most
              // important signal was carried by the hue of a 6px
              // aria-hidden dot, and a collapsed group announced only
              // its project name.
              aria-label={`${pinnedProjects.includes(g.projectPath) ? "Pinned project. " : ""}${groupAccessibleName(g, parent)}`}
              // Every real group's tooltip is its path. "Other" has no
              // path — its identity is a sentinel that is not printable —
              // so it explains itself instead.
              title={g.projectPath === OTHER_PROJECT ? OTHER_PROJECT_HINT : g.projectPath}
              onClick={() => { setGroupCollapsed(g.projectPath, !isCollapsed); }}
              onKeyDown={e => {
                // Enter and Space come free with a real button; the tree
                // arrows do not.
                if (e.key === "ArrowRight" && isCollapsed) {
                  e.preventDefault();
                  setGroupCollapsed(g.projectPath, false);
                } else if (e.key === "ArrowLeft" && !isCollapsed) {
                  e.preventDefault();
                  setGroupCollapsed(g.projectPath, true);
                }
              }}
            >
              <span
                className={isCollapsed
                  ? "codicon codicon-chevron-down group-chevron is-collapsed"
                  : "codicon codicon-chevron-down group-chevron"}
                aria-hidden="true"
              />
              <span className="group-name">{g.projectName}</span>
              {parent !== "" && <span className="group-parent">· {parent}</span>}
              <span className="group-meta">
                {/* Shape first, hue second: the two states that need
                    the user take the same warning and error glyphs
                    their rows take, so a collapsed group
                    reads correctly with no colour vision at all. */}
                {attention === "working" ? (
                  <span className="group-attention-working" aria-hidden="true" />
                ) : attention !== null && (
                  <span
                    className={attention === "failed"
                      ? "codicon codicon-sm codicon-error group-attention-failed"
                      : "codicon codicon-sm codicon-warning group-attention-waiting"}
                    aria-hidden="true"
                  />
                )}
                {/* No chip for nothing: a "0" on every chats-only project
                    is noise, and the accessible name already carries the
                    totals. */}
                {groupItemCount > 0 && (
                  <span className="group-count" aria-hidden="true">{groupItemCount}</span>
                )}
              </span>
            </button>
            {g.projectPath !== OTHER_PROJECT && g.projectPath !== "" && (
              <button type="button"
                className={auxMenu?.kind === "project"
                  && auxMenu.projectPath === g.projectPath
                  ? "project-menu-button is-open" : "project-menu-button"}
                aria-label={`Project actions for ${g.projectName} at ${g.projectPath}`}
                aria-haspopup="menu"
                aria-expanded={auxMenu?.kind === "project"
                  && auxMenu.projectPath === g.projectPath}
                aria-controls={auxMenu?.kind === "project"
                  && auxMenu.projectPath === g.projectPath
                  ? `${uid}-aux-menu` : undefined}
                title={`Actions for ${g.projectName} at ${g.projectPath}`}
                onClick={event => {
                  openAuxMenu({ kind: "project", projectPath: g.projectPath },
                    event.currentTarget);
                }}>
                <span className="codicon codicon-ellipsis" aria-hidden="true" />
              </button>
            )}
          </div>
        )}
        {confirmProjectPath === g.projectPath
          && !projectArchiveBlocked(g, protectedProjectSet) && (
          <div className="project-confirm" role="alertdialog"
            aria-label={`Archive project ${g.projectName}`}
            onKeyDown={event => {
              if (event.key !== "Escape") return;
              event.preventDefault();
              setConfirmProjectPath(null);
            }}>
            <div>
              Archive <span className="project-confirm-name">{g.projectName}</span>?
              Nothing in the project is stopped or deleted. This only hides it in {productLabel()},
              and you can restore it from Archived projects.
            </div>
            <div className="project-confirm-acts">
              <button type="button" className="btn"
                onClick={() => { archiveProject(g); }}>
                Archive project
              </button>
              <button type="button" className="btn" autoFocus
                onClick={() => { setConfirmProjectPath(null); }}>
                Cancel
              </button>
            </div>
          </div>
        )}
        {/* Collapsed groups leave the DOM rather than hide, so keyboard
            traversal and screen readers skip them entirely. */}
        <FoldingRows open={!isCollapsed}>{!isCollapsed && (
          <ul id={listId} className="session-items">
            {projectWorkspaces.filter(record => !pinnedWorkspaces.includes(record.id)).map(record =>
              renderWorkspaceRow(record, g.terminals ?? []))}
            {visibleGroup.rows.map(renderSessionRow)}
            {visibleGroup.hidden > 0 && (
              <li className="session-show-more">
                <button type="button" className="session-show-more-button"
                  aria-label={`Show ${visibleGroup.hidden} more sessions in ${g.projectName || "this project"}`}
                  onClick={() => {
                    setRevealedProjects(paths => paths.includes(g.projectPath)
                      ? paths : [...paths, g.projectPath]);
                  }}>
                  Show {visibleGroup.hidden} more
                </button>
              </li>
            )}
          </ul>
        )}</FoldingRows>
      </div>
    );
  };
  const hasPinnedProjects = listedGroups.some(group => pinnedProjects.includes(group.projectPath));
  const pinnedSection = (codeWorkspaces && terminalWorkspaces !== undefined) || hasPinnedProjects;
  // What the sticky-label clipping measures: the list's own children and
  // each group's label and rows. A render that changes none of them (a hover,
  // a keystroke, a streamed token) leaves the measured nodes bound.
  const headerClipKey = useMemo(() => JSON.stringify([loading, showHeaders, pinnedSection,
    codeWorkspaces, listedGroups.map(group => [group.projectPath,
      pinnedProjects.includes(group.projectPath), collapsed.includes(group.projectPath)])]),
  [codeWorkspaces, collapsed, listedGroups, loading, pinnedProjects, pinnedSection, showHeaders]);
  useSidebarHeaderClipping(asideRef, glass, headerClipKey);

  return (
    <aside ref={asideRef}
      className="pane sidebar is-floating"
      inert={backgroundInert ? true : undefined}
      aria-hidden={backgroundInert || undefined}
      role={focusOnMobileOpen ? "dialog" : undefined}
      data-mobile-sidebar-dialog={focusOnMobileOpen ? "true" : undefined}
      aria-label="Sessions" aria-modal={focusOnMobileOpen || undefined}
      onKeyDown={event => {
        if (!focusOnMobileOpen) return;
        if (event.key === "Escape") {
          if (document.querySelector(FOREGROUND_DIALOG_SELECTOR) !== null) return;
          event.preventDefault();
          event.stopPropagation();
          onCloseMobile?.();
          return;
        }
        if (event.key !== "Tab") return;
        const selector =
          'button:not([disabled]), input:not([disabled]), [href], [tabindex]:not([tabindex="-1"])';
        // Row and project menus are portaled to document.body so they can clear
        // the drawer's clipping boundary. Portal key events still bubble through
        // this React handler, but a query rooted at <aside> cannot see their menu
        // items. Treat the visible portal as the final part of the drawer's
        // logical focus order so Tab cannot escape into the obscured workspace.
        // Both portals live under document.body, outside <aside>, so the two
        // queries can never return the same node.
        const portalRoots = [auxMenuRef.current, menuRef.current]
          .filter((root): root is HTMLDivElement => root !== null);
        const focusable = [
          ...(asideRef.current?.querySelectorAll<HTMLElement>(selector) ?? []),
          ...portalRoots.flatMap(root => [...root.querySelectorAll<HTMLElement>(selector)]),
        ].filter(node => node.offsetParent !== null);
        if (focusable.length === 0) return;
        const first = focusable[0];
        const last = focusable[focusable.length - 1];
        const current = focusable.indexOf(document.activeElement as HTMLElement);
        if (portalRoots.length > 0) {
          event.preventDefault();
          const next = current < 0
            ? (event.shiftKey ? focusable.length - 1 : 0)
            : (current + (event.shiftKey ? -1 : 1) + focusable.length) % focusable.length;
          focusable[next]?.focus();
        } else if (event.shiftKey && document.activeElement === first) {
          event.preventDefault();
          last.focus();
        } else if (!event.shiftKey && document.activeElement === last) {
          event.preventDefault();
          first.focus();
        }
      }}>
      <style>{SIDEBAR_CSS}</style>

      {/* Agent identities lead; the existing task navigation follows below. */}
      {agentMenu}

      {/* One header row: the pane's name, then every pane-level action as a
          quiet icon. New chat and Filter keep their names for the
          accessibility tree and their shortcuts in the tooltip; project
          maintenance stays behind the one overflow. */}
      <nav className="sidebar-primary-nav" aria-label="Sessions">
        <span className="sidebar-section-label">{sectionLabel}</span>
        {threadInbox !== undefined && <ThreadScopePicker inbox={threadInbox} />}
        <span className="sidebar-section-actions">
          <button
            type="button"
            className={newChatActive ? "sidebar-nav-button is-active" : "sidebar-nav-button"}
            aria-label={newLabel}
            aria-current={newChatActive ? "page" : undefined}
            title={codeWorkspaces && onNewProject ? "New workspace or project" : `${newLabel} (⌘N)`}
            aria-haspopup={codeWorkspaces && onNewProject ? "menu" : undefined}
            aria-expanded={codeWorkspaces && onNewProject ? auxMenu?.kind === "create" : undefined}
            aria-controls={auxMenu?.kind === "create" ? `${uid}-aux-menu` : undefined}
            onClick={codeWorkspaces && onNewProject
              ? event => { openAuxMenu({ kind: "create" }, event.currentTarget); }
              : onNew}
          >
            {/* Every surface creates with the same +: the speech bubbles already
                mean a chat context and a subagent message elsewhere. */}
            <span className="codicon codicon-add sidebar-nav-icon" aria-hidden="true" />
            <span className="sidebar-nav-label">{newLabel}</span>
          </button>
          {onQuery !== undefined && (
            <button
              ref={searchButtonRef}
              type="button"
              className={showSearch ? "sidebar-nav-button is-active" : "sidebar-nav-button"}
              aria-expanded={showSearch}
              aria-controls={showSearch ? `${uid}-search` : undefined}
              title={`Filter ${sectionLabel.toLowerCase()} (⌘K)`}
              onClick={() => {
                if (showSearch) searchInputRef.current?.focus();
                else setSearchOpen(true);
              }}
            >
              <span className="codicon codicon-search sidebar-nav-icon" aria-hidden="true" />
              <span className="sidebar-nav-label">Filter {sectionLabel.toLowerCase()}</span>
            </button>
          )}
          {threadInbox === undefined && showHeaders && !loading && listedGroups.length > 0 && (
            <button
              type="button"
              className="sidebar-section-action"
              aria-label={`${allGroupsCollapsed ? "Expand" : "Collapse"} all ${groupNoun}`}
              title={`${allGroupsCollapsed ? "Expand" : "Collapse"} all ${groupNoun}`}
              onClick={() => { setAllGroupsCollapsed(!allGroupsCollapsed); }}
            >
              <span className={allGroupsCollapsed
                ? "codicon codicon-expand-all" : "codicon codicon-collapse-all"} aria-hidden="true" />
            </button>
          )}
          {hasProjectsMenu && (
            <button
              type="button"
              className="sidebar-section-action"
              aria-label="Project options"
              aria-haspopup="menu"
              aria-expanded={auxMenu?.kind === "projects"}
              aria-controls={auxMenu?.kind === "projects" ? `${uid}-aux-menu` : undefined}
              title="Project options"
              onClick={event => { openAuxMenu({ kind: "projects" }, event.currentTarget); }}
            >
              <span className="codicon codicon-ellipsis" aria-hidden="true" />
            </button>
          )}
          {terminalWorkspaces !== undefined && !codeWorkspaces && (
            <button
              type="button"
              className="sidebar-section-action"
              aria-label="New terminal workspace"
              title="New terminal workspace"
              disabled={!terminalWorkspaces.canCreate || terminalWorkspaces.busy}
              onClick={terminalWorkspaces.onCreate}
            >
              <span className="codicon codicon-add" aria-hidden="true" />
            </button>
          )}
          {focusOnMobileOpen && onCloseMobile !== undefined && (
            <button type="button" className="sidebar-mobile-close"
              aria-label="Close sessions" title="Close sessions" onClick={onCloseMobile}>
              <span className="codicon codicon-close" aria-hidden="true" />
            </button>
          )}
        </span>
      </nav>

      {/* Cursor exposes Search as a destination row. Interface keeps its useful
          live filtering directly below that row, but only while selected. */}
      {showSearch && onQuery !== undefined && (
        <div className="sidebar-search">
          <input
            id={`${uid}-search`}
            ref={searchInputRef}
            type="search"
            className="sidebar-search-input"
            value={query ?? ""}
            placeholder={`Filter ${sectionLabel.toLowerCase()}…`}
            aria-label={`Filter ${sectionLabel.toLowerCase()}`}
            onChange={e => onQuery(e.target.value)}
            onKeyDown={e => {
              if (e.key !== "Escape") return;
              e.preventDefault();
              e.stopPropagation();
              if ((query ?? "") !== "") {
                onQuery("");
                return;
              }
              setSearchOpen(false);
              searchButtonRef.current?.focus();
            }}
          />
          {(query ?? "") === "" && (
            <p className="sidebar-search-hint">Type to filter {sectionLabel.toLowerCase()}.</p>
          )}
          {(query ?? "") !== "" && (
            <button
              type="button"
              className="sidebar-search-clear"
              aria-label="Clear search"
              title="Clear search"
              onClick={() => {
                onQuery("");
                searchInputRef.current?.focus();
              }}
            >
              <span className="codicon codicon-sm codicon-close" aria-hidden="true" />
            </button>
          )}
        </div>
      )}

      {/* Progress of the import. It replaces nothing and pushes the list down:
          ~170 sessions is long enough that a silent pane reads as a hang. */}
      {importer !== undefined && importer.running && (
        <div className="sidebar-progress" role="status">
          <span>{importProgressLabel(importer)}</span>
          <div className="sidebar-progress-bar">
            <div
              className="sidebar-progress-fill"
              style={{
                width: importer.total > 0
                  ? `${Math.round((importer.done / importer.total) * 100)}%`
                  : "0%",
              }}
            />
          </div>
        </div>
      )}

      {/* What the last import did — including the part that is only local. */}
      {importer !== undefined && importer.note !== null && !importer.running && (
        <div className="sidebar-say" role="status">
          <span className="sidebar-say-text">{importer.note}</span>
          <button
            type="button"
            className="sidebar-say-close"
            aria-label="Dismiss"
            onClick={importer.onDismissNote}
          >
            <span className="codicon codicon-sm codicon-close" aria-hidden="true" />
          </button>
        </div>
      )}

      {/* Why the last lifecycle action did not happen, in the engine's own
          words. role="alert": it answers a click the user just made. */}
      {lifecycle !== undefined && lifecycle.error !== null && (
        <div key={lifecycle.error} className="sidebar-say sidebar-say-error" role="alert"
          onAnimationEnd={lifecycle.onDismissError}>
          <span className="codicon codicon-sm codicon-error" aria-hidden="true" />
          <span className="sidebar-say-text">{lifecycle.error}</span>
          <button
            type="button"
            className="sidebar-say-close"
            aria-label="Dismiss"
            onClick={lifecycle.onDismissError}
          >
            <span className="codicon codicon-sm codicon-close" aria-hidden="true" />
          </button>
        </div>
      )}

      {/* THE PINNED SECTION — the pane's reason to exist.
          Above Projects, above every collapse toggle, and never hidden by one:
          the agents blocked on the user and the ones that have stopped, lifted
          out of their groups exactly the way this pane's own Needs attention
          section lifts them out of Working and Recent. With 138 sessions across
          16 groups the alternative was a 6px dot on a row two twisties deep.
          Deliberately NOT role="status". App.tsx owns the single live region for
          this fact; a second one here made a screen reader say the same arrival
          twice. This header also enters the
          DOM together with its content on 0→1, so it never announced the
          transition reliably anyway — the announcement belongs to the region
          that is always mounted. */}
      {lifting && (
        <section className="attention" aria-labelledby={`${uid}-attention`}>
          <div className="attention-head" id={`${uid}-attention`}>
            <span className="codicon codicon-sm codicon-warning attention-head-glyph"
              aria-hidden="true" />
            Needs attention
          </div>
          <ul className="session-items">{attentionRows.map(renderSessionRow)}</ul>
        </section>
      )}

      {archivedProjectsOpen && showArchivedProjects && (
        <section id={`${uid}-archived-projects`} className="archived-projects"
          aria-labelledby={`${uid}-archived-projects-title`}>
          <header className="archived-projects-header">
            <span id={`${uid}-archived-projects-title`} className="archived-projects-title">
              Archived projects
            </span>
            <span className="archived-projects-spacer" />
            <button type="button" className="archived-projects-close"
              aria-label="Close archived projects" title="Close"
              onClick={() => { setArchivedProjectsOpen(false); }}>
              <span className="codicon codicon-close" aria-hidden="true" />
            </button>
          </header>
          <ul className="archived-projects-list">
            {archivedProjects.map(project => (
              <li key={project.projectPath} className="archived-project-row">
                <span className="archived-project-copy" title={project.projectPath}>
                  <span className="archived-project-name">{project.projectName}</span>
                  <span className="archived-project-path">{project.projectPath}</span>
                </span>
                <button type="button" className="archived-project-restore"
                  aria-label={`Restore ${project.projectName}`}
                  onClick={() => { restoreProject(project.projectPath); }}>
                  Restore
                </button>
              </li>
            ))}
          </ul>
        </section>
      )}

      {loading ? (
        /* The shape of what is coming, not a spinner and not a sentence. The
           rows are aria-hidden — six identical "loading" announcements are
           worse than none — and the list itself carries the one status a
           screen reader needs. */
        <div className="session-list session-list-flat">
          <ul className="session-items" role="status" aria-label="Loading sessions">
            {SKELETON_TITLE_WIDTHS.map(width => (
              <li key={width} className="session-skeleton" aria-hidden="true">
                <span className="dot session-skeleton-dot" />
                <span className="session-skeleton-block session-skeleton-tag" />
                <span className="session-skeleton-block session-skeleton-title"
                  style={{ width }} />
              </li>
            ))}
          </ul>
        </div>
      ) : threadInbox !== undefined ? (
        <ThreadInbox inbox={threadInbox} onSelect={onSelect} onOpenInNewPane={onOpenInNewPane} {...threadBridges}
          onArchive={onArchive} lifecycle={lifecycle} diffStats={diffStats} peeks={peeks} live={threadLive}
          emptyText={emptyText} emptyHint={emptyHint} filterQuery={filterQuery}
          onSearchTranscripts={onSearchTranscripts} />
      ) : listedGroups.length === 0 ? (
        <div className="empty">
          {/* Three different facts, and saying the wrong one is a bug report:
              with a filter on, "no sessions yet" would be false — and it is the
              likeliest state of all on a first run, when no session on the
              machine carries Interface's marker yet and the origin filter
              empties the pane by itself. An unexplained empty sidebar reads as
              a broken app, so the reason and the way out are both stated here.

              Sentences and nothing else. The zero-session CTA lives in the
              right pane, which is the app's one welcome surface — rendering a
              second accent button here put the same sentence over the same
              button twice, side by side, on the first screen a new user sees.
              The way in from this pane is the "+" in the part header above, and
              when a filter is what emptied the list the Show all button
              directly above this line is the more useful action anyway. */}
          <p className="empty-text">
            {allListedProjectsArchived ? `All projects are archived in ${productLabel()}.` : emptyText}
          </p>
          {(allListedProjectsArchived || emptyHint !== null) && <p className="empty-hint">
            {allListedProjectsArchived
              ? "Open Project options, then Archived projects, to restore one."
              : emptyHint}
          </p>}
          {/* This field filters the rows already listed; the Library searches
              stored transcripts. They stop competing here: a filter that found
              nothing hands its query to the search that reads further. */}
          {onSearchTranscripts !== undefined && filterQuery !== "" && (
            <button type="button" className="sidebar-search-handoff"
              onClick={() => { onSearchTranscripts(filterQuery); }}>
              <span className="codicon codicon-sm codicon-search" aria-hidden="true" />
              <span>Search transcripts for “{filterQuery}”</span>
            </button>
          )}
        </div>
      ) : (
        <div className={showHeaders ? "session-list" : "session-list session-list-flat"}>
          {pinnedSection && <section className="workspace-pinned-section" aria-label="Pinned workspaces and projects">
            <h3 className="sidebar-list-heading">Pinned{pinnedWorkspaces.length === 0 && !hasPinnedProjects
              && <span className="sidebar-list-heading-note" title="Pin a workspace or project from its menu.">none yet</span>}</h3>
            <ul className="session-items">
              {listedGroups.flatMap(group => (workspacesByProject.get(group.projectPath) ?? [])
                .filter(record => pinnedWorkspaces.includes(record.id))
                .map(record => renderWorkspaceRow(record, group.terminals ?? [])))}
            </ul>
            {listedGroups.map((g, i) => pinnedProjects.includes(g.projectPath) ? renderProject(g, i) : null)}
          </section>}
          {codeWorkspaces && <h3 className="sidebar-list-heading sidebar-folders-heading">Folders</h3>}
          {listedGroups.map((g, i) => pinnedProjects.includes(g.projectPath) ? null : renderProject(g, i))}
        </div>
      )}

      {auxMenu !== null && (auxMenu.kind === "create" || auxMenu.kind === "projects" || auxProject !== null
        || auxWorkspace !== null) && (() => {
        const closeToTrigger = (): void => {
          const trigger = auxMenuTrigger.current;
          setAuxMenu(null);
          requestAnimationFrame(() => { trigger?.focus(); });
        };
        const menu = (
          <div
            ref={auxMenuRef}
            id={`${uid}-aux-menu`}
            className="row-menu sidebar-aux-menu is-floating"
            role="menu"
            aria-label={auxMenu.kind === "create" ? "New workspace or project" : auxMenu.kind === "projects"
              ? "Project options"
              : auxMenu.kind === "workspace"
                ? `Actions for ${auxWorkspace?.name ?? "workspace"}`
                : `Actions for ${auxProject?.projectName ?? "project"}${auxProject === null ? "" : ` at ${auxProject.projectPath}`}`}
            onFocus={event => { roveMenuTabIndex(event.currentTarget, event.target); }}
            onKeyDown={event => {
              menuKeyNav(event, '[role="menuitem"]:not(:disabled)', () => {
                event.stopPropagation();
                closeToTrigger();
              });
            }}
            style={auxMenuBox === null
              ? { visibility: "hidden", top: 0, left: 0 }
              : { top: auxMenuBox.top, left: auxMenuBox.left }}
          >
            {auxMenu.kind === "create" ? (
              <>
                <button type="button" className="row-menu-item" role="menuitem" tabIndex={-1}
                  disabled={terminalWorkspaces?.busy || terminalWorkspaces?.canCreate === false}
                  onClick={() => { setAuxMenu(null); onNew(); }}>
                  New workspace
                </button>
                <button type="button" className="row-menu-item" role="menuitem" tabIndex={-1}
                  disabled={terminalWorkspaces?.busy || terminalWorkspaces?.canCreate === false}
                  onClick={() => {
                    auxMenuTrigger.current?.focus();
                    setAuxMenu(null);
                    onNewProject?.();
                  }}>
                  New project…
                </button>
              </>
            ) : auxMenu.kind === "workspace" && auxWorkspace !== null
              && terminalWorkspaces !== undefined ? (
              <>
                <button type="button" className="row-menu-item" role="menuitem" tabIndex={-1}
                  disabled={terminalWorkspaces.busy}
                  onClick={() => {
                    setAuxMenu(null);
                    terminalWorkspaces.onRename(auxWorkspace.id);
                  }}>
                  Rename workspace…
                </button>
                {onPinWorkspace && <button type="button" className="row-menu-item" role="menuitem" tabIndex={-1}
                  disabled={terminalWorkspaces.busy || (!pinnedWorkspaces.includes(auxWorkspace.id) && pinnedWorkspaces.length >= 128)}
                  onClick={() => {
                    closeToTrigger();
                    onPinWorkspace(auxWorkspace.id, !pinnedWorkspaces.includes(auxWorkspace.id));
                    requestAnimationFrame(() => document.querySelector<HTMLElement>(
                      `[data-terminal-workspace-id="${auxWorkspace.id}"]`)?.focus());
                  }}>
                  {pinnedWorkspaces.includes(auxWorkspace.id) ? "Unpin workspace" : "Pin workspace"}
                </button>}
                <button type="button" className="row-menu-item row-menu-danger" role="menuitem" tabIndex={-1}
                  disabled={terminalWorkspaces.busy}
                  title="Remove this saved workspace and stop the terminals in it."
                  onClick={() => {
                    setAuxMenu(null);
                    terminalWorkspaces.onDelete(auxWorkspace.id);
                  }}>
                  Delete workspace…
                </button>
              </>
            ) : auxMenu.kind === "projects" ? (
              <>
                {showArchivedProjects && (
                  <button type="button" className="row-menu-item" role="menuitem" tabIndex={-1}
                    aria-label={`${archivedProjectsOpen ? "Hide" : "Show"} archived projects`}
                    title={`${archivedProjects.length} archived project${archivedProjects.length === 1 ? "" : "s"}. Archive only hides projects in ${productLabel()}.`}
                    onClick={() => {
                      setArchivedProjectsOpen(open => !open);
                      closeToTrigger();
                    }}>
                    {archivedProjectsOpen ? "Hide archived projects" : "Archived projects"}
                  </button>
                )}
                {hideable > 0 && (
                  <button type="button" className="row-menu-item" role="menuitem" tabIndex={-1}
                    aria-label={hiddenTotal > 0
                      ? `Show all sessions — ${hiddenTotal} hidden`
                      : `Hide ${hideable} sessions`}
                    title={`${filterLabel}. ${productLabel()} hides external and throwaway sessions under ${TRIVIAL_SIZE}; active and failed sessions remain visible.`}
                    onClick={() => {
                      onToggleFilters();
                      closeToTrigger();
                    }}>
                    {hiddenTotal > 0 ? "Show all sessions" : "Hide inactive sessions"}
                  </button>
                )}
                {importer !== undefined && importable > 0 && !importer.running && (
                  <button type="button" className="row-menu-item" role="menuitem" tabIndex={-1}
                    aria-label={`Import ${importable} sessions`}
                    title={`Mark the ${importable} sessions on this machine that ${productLabel()} has not claimed yet. Nothing is copied or moved.`}
                    onClick={() => {
                      importer.onImportAll();
                      closeToTrigger();
                    }}>
                    Import sessions…
                  </button>
                )}
              </>
            ) : auxProject !== null && (
              <>
                {onPinProject && <button type="button" className="row-menu-item" role="menuitem" tabIndex={-1}
                  disabled={!pinnedProjects.includes(auxProject.projectPath) && pinnedProjects.length >= 128}
                  onClick={() => {
                    const path = auxProject.projectPath;
                    closeToTrigger();
                    onPinProject(path, !pinnedProjects.includes(path));
                    requestAnimationFrame(() => {
                      const group = [...document.querySelectorAll<HTMLElement>("[data-sidebar-project]")]
                        .find(node => node.dataset.sidebarProject === path);
                      // The moved menu button can be hover-hidden; focus its visible project row.
                      const next = group?.querySelector<HTMLElement>(".group-header")
                        ?? asideRef.current?.querySelector<HTMLElement>("button:not(:disabled)");
                      next?.focus();
                    });
                  }}>
                  {pinnedProjects.includes(auxProject.projectPath) ? "Unpin project" : "Pin project"}
                </button>}
                <button type="button" className="row-menu-item" role="menuitem" tabIndex={-1}
                  title={`Start a new chat in ${auxProject.projectPath}`}
                  onClick={() => {
                    setAuxMenu(null);
                    onNewInProject(auxProject.projectPath);
                  }}>
                  New chat
                </button>
                <button type="button" className="row-menu-item" role="menuitem" tabIndex={-1}
                  disabled={projectArchiveBlocked(auxProject, protectedProjectSet)}
                  title={projectArchiveBlocked(auxProject, protectedProjectSet)
                    ? "Finish or stop active agents and terminal work before archiving this project"
                    : `Archive project — hides it only in ${productLabel()}`}
                  onClick={() => {
                    if (projectArchiveBlocked(auxProject, protectedProjectSet)) return;
                    setAuxMenu(null);
                    setConfirmProjectPath(auxProject.projectPath);
                  }}>
                  Archive project…
                </button>
              </>
            )}
          </div>
        );
        return typeof document === "undefined" ? menu : createPortal(menu, document.body);
      })()}

      {/* The hover/Space preview. Portaled for the same reason the row menu is:
          the pane is a 288px scroller, and a card drawn inside it would be
          clipped by the very list it is previewing. Read-only by design — an
          approval is answered in the session, never from a
          surface that appears under the pointer. */}
      {peekRow !== null && renameFor === null && (() => {
        const meta = sidebarRowMeta(peekRow, diffStats[peekRow.key]);
        const card = (
          <SessionPeek
            cardRef={peekRef}
            id={`${uid}-peek`}
            title={peekRow.title}
            badge={peekRow.badge}
            status={meta.status}
            age={meta.age}
            diff={meta.diff}
            worktree={meta.worktree}
            facts={peeks[peekRow.key] ?? {}}
            box={peekBox}
          />
        );
        return typeof document === "undefined" ? card : createPortal(card, document.body);
      })()}

      {menuRow !== null && renameFor === null && (() => {
        const menu = (
          <div
            ref={menuRef}
            id={`${uid}-row-menu`}
            className="row-menu is-floating"
            role="menu"
            aria-label={`Actions for ${menuRow.title}`}
            onFocus={event => { roveMenuTabIndex(event.currentTarget, event.target); }}
            onKeyDown={event => {
              // Same predicate as the aux menu: a disabled item is not a place
              // focus may land, and an arrow key that stops on one strands the
              // keyboard on an item that cannot be pressed.
              menuKeyNav(event, '[role="menuitem"]:not(:disabled)', () => {
                setMenuFor(null);
                restoreRowActionFocus(menuRow.key);
              });
            }}
            style={menuBox === null
              ? { visibility: "hidden", top: 0, left: 0 }
              : { top: menuBox.top, left: menuBox.left }}
          >
            {onOpenInNewPane !== undefined && (
              <button type="button" className="row-menu-item" role="menuitem" tabIndex={-1}
                title="Keep the current session visible and add this one beside it."
                onClick={() => {
                  setMenuFor(null);
                  onOpenInNewPane(menuRow.key);
                  restoreRowActionFocus(menuRow.key);
                }}>
                Open in new pane
              </button>
            )}
            {menuOps?.rename && lifecycle !== undefined && (
              <button type="button" className="row-menu-item" role="menuitem" tabIndex={-1}
                title="Retitles the session in the engine's own store, so the CLI sees it too."
                onClick={() => { startRename(menuRow); }}>
                Rename…
              </button>
            )}
            {menuOps?.fork && lifecycle !== undefined && (
              <button type="button" className="row-menu-item" role="menuitem" tabIndex={-1}
                title="Copy this session so two approaches can diverge from the same history."
                onClick={() => {
                  setMenuFor(null);
                  lifecycle.onFork(menuRow.key);
                  restoreRowActionFocus(menuRow.key);
                }}>
                Fork
              </button>
            )}
            {menuOps?.adopt && lifecycle !== undefined && (
              <button type="button" className="row-menu-item" role="menuitem" tabIndex={-1}
                title={menuRow.external
                  ? `Mark this session as ${productLabel()}'s, so it lists here with the chats you started.`
                  : `Drop ${productLabel()}'s mark. The session stays in the engine's store, untouched.`}
                onClick={() => {
                  setMenuFor(null);
                  lifecycle.onAdopt(menuRow.key, menuRow.external);
                  restoreRowActionFocus(menuRow.key);
                }}>
                {menuRow.external ? <>Add to <ProductName /></> : <>Remove from <ProductName /></>}
              </button>
            )}
            {menuOps?.archive && lifecycle !== undefined && (
              <button type="button" className="row-menu-item" role="menuitem" tabIndex={-1}
                title={`Archives in ${menuRow.engine}'s own store as well as here, and only ${menuRow.engine}'s own CLI can undo it. Hide from ${productLabel()} only affects this app.`}
                onClick={() => {
                  setMenuFor(null);
                  setConfirmFor({ key: menuRow.key, kind: "engine" });
                }}>
                Archive in {menuRow.engine}…
              </button>
            )}
            {/* No confirm and no ellipsis: same action, same name and same
                friction as the row's own Hide button. It is reversible from
                Library > Archived, which is what the reversibility rule above
                reserves an unconfirmed click for. */}
            <button type="button" className="row-menu-item" role="menuitem" tabIndex={-1}
              title={`Hide this session in ${productLabel()}. It stays in the engine's own store and can be restored from Library > Archived.`}
              onClick={() => {
                setMenuFor(null);
                onArchive(menuRow.key);
                restoreRowActionFocus(menuRow.key);
              }}>
              Hide
            </button>
            {menuOps?.remove && lifecycle !== undefined && (
              <button type="button" className="row-menu-item row-menu-danger" role="menuitem" tabIndex={-1}
                title="Permanently removes the transcript from the engine's store."
                onClick={() => {
                  setMenuFor(null);
                  setConfirmFor({ key: menuRow.key, kind: "delete" });
                }}>
                Delete…
              </button>
            )}
          </div>
        );
        return typeof document === "undefined" ? menu : createPortal(menu, document.body);
      })()}

      {confirmFor !== null && confirmRow !== null && (
        <ConfirmDialog
          open
          {...sessionConfirmDialog(
            confirmRow,
            confirmFor.kind,
            lifecycle?.liveKeys.has(confirmRow.key) === true,
          )}
          onConfirm={applyRowConfirm}
          onCancel={() => {
            const key = confirmFor.key;
            setConfirmFor(null);
            restoreRowActionFocus(key);
          }}
        />
      )}

      {onSettings !== undefined && (
        <footer className="sidebar-footer">
          <div className="sidebar-footer-row">
            <button type="button" className="sidebar-settings-button" aria-label="Settings"
              aria-current={settingsActive ? "page" : undefined}
              title="Settings (⌘,)" disabled={settingsDisabled}
              onClick={event => { event.currentTarget.focus(); onSettings(); }}>
              <span className="sidebar-settings-label">Settings</span>
            </button>
            {footerAlert !== undefined && (
              <button type="button" className="sidebar-footer-alert"
                aria-label={footerAlert.accessibleName} title={footerAlert.description}
                disabled={settingsDisabled} onClick={onFooterAlert ?? onSettings}>
                <span className="sidebar-footer-alert-dot" aria-hidden="true" />
                <span className="sidebar-footer-alert-label">{footerAlert.label}</span>
                {footerAlert.shortLabel !== undefined && footerAlert.shortLabel !== footerAlert.label && (
                  <span className="sidebar-footer-alert-label is-short" aria-hidden="true">{footerAlert.shortLabel}</span>
                )}
              </button>
            )}
          </div>
        </footer>
      )}
    </aside>
  );
}
