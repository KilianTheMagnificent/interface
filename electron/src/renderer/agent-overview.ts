/**
 * The agent overview: every agent pane across every saved workspace, grouped
 * by what it is doing now (owner's BridgeMind dashboard reference, 2026-09-23).
 *
 * Pure. The terminal center already holds every fact this needs — the host
 * registry, the saved workspaces and the renderer-derived live activity — and
 * hands them in; nothing here reads the DOM, storage or the bridge.
 */
import type { TerminalSummary, TerminalWorkspaceRecord } from "../shared/ipc";
import type { TerminalLiveActivity } from "./terminal-activity";
import {
  savedLayoutFacts, terminalIdsInWorkspaceState, type TerminalWorkspaceState,
} from "./terminal-workspace";
import { workspaceSidebarTitle } from "./sidebar-view";

/** Needs the user, busy, finished a turn nobody has seen, quiet, or no longer running. */
export type AgentOverviewStatus = "waiting" | "working" | "done" | "idle" | "stopped";
/** The chips above the list. Stopped panes only show under All. */
export type AgentOverviewFilter = "all" | "waiting" | "working" | "done" | "idle";
/** The two ways the list is grouped: by what each agent is doing, or by workspace. */
export type AgentOverviewGrouping = "status" | "project";

export interface AgentOverviewTerminal {
  readonly id: string;
  readonly title: string;
  readonly state: TerminalSummary["state"];
  readonly activity?: TerminalLiveActivity;
  /** Finished a turn nobody has focused since (the blue done light). */
  readonly done?: boolean;
  /** Launch profile, including one the host recognized in a shell pane. */
  readonly profileId?: string;
  readonly sessionKey?: string;
}

export interface AgentOverviewWorkspace {
  readonly id: string;
  /** The name the sidebar prints for this workspace. */
  readonly name: string;
  /** Saved membership first, then any tab only its layout holds. */
  readonly terminalIds: readonly string[];
  /** Terminals with a tab in this workspace's layout. */
  readonly attachedIds: ReadonlySet<string>;
}

export interface AgentOverviewRow {
  readonly terminalId: string;
  readonly title: string;
  readonly profileId: string;
  readonly sessionKey?: string;
  readonly workspaceId: string;
  readonly workspaceName: string;
  readonly status: AgentOverviewStatus;
  /** Host state, so a stopped row can say Exited or Failed. */
  readonly state: TerminalSummary["state"];
}

export interface AgentOverviewCounts {
  readonly all: number;
  readonly waiting: number;
  readonly working: number;
  readonly done: number;
  readonly idle: number;
  readonly stopped: number;
}

export interface AgentOverviewGroup {
  readonly status: AgentOverviewStatus;
  readonly rows: readonly AgentOverviewRow[];
}

/** Most urgent first: what needs you, then what is moving, then the rest. */
export const AGENT_OVERVIEW_ORDER: readonly AgentOverviewStatus[] =
  ["waiting", "working", "done", "idle", "stopped"];

/**
 * The same folding the pane glyph uses: starting reads as busy, and a resting
 * agent whose finished turn nobody has looked at yet is done (blue).
 */
export function agentOverviewStatus(state: TerminalSummary["state"],
    activity: TerminalLiveActivity | undefined, done = false): AgentOverviewStatus {
  if (state === "exited" || state === "failed") return "stopped";
  if (activity === "waiting") return "waiting";
  if (state === "starting" || activity === "working") return "working";
  return done ? "done" : "idle";
}

/** Agents are panes with a launch profile; a plain shell is not one. */
export function isAgentProfile(profileId: string | undefined): profileId is string {
  return profileId !== undefined && profileId !== "" && profileId !== "shell";
}

/** A saved workspace as the overview reads it, named as the sidebar names it. */
export function agentOverviewWorkspace(record: TerminalWorkspaceRecord,
    layout: Pick<TerminalWorkspaceState, "panes">): AgentOverviewWorkspace {
  const attachedIds = terminalIdsInWorkspaceState(layout);
  const saved = record.terminals.map(entry => entry.id);
  const savedIds = new Set(saved);
  return {
    id: record.id,
    name: workspaceSidebarTitle(record.name, [], record.terminals[0]?.title),
    terminalIds: [...saved, ...[...attachedIds].filter(id => !savedIds.has(id))],
    attachedIds,
  };
}

const savedOverviewWorkspaces = new WeakMap<TerminalWorkspaceRecord, AgentOverviewWorkspace>();

/**
 * A workspace that is not open, read from its saved layout. Records are
 * immutable and the roster keeps every record it does not change, so each
 * saved layout is parsed once, not on every splitter frame or focus click.
 */
export function savedAgentOverviewWorkspace(record: TerminalWorkspaceRecord): AgentOverviewWorkspace {
  let workspace = savedOverviewWorkspaces.get(record);
  if (workspace === undefined) {
    workspace = agentOverviewWorkspace(record, savedLayoutFacts(record.layout).state);
    savedOverviewWorkspaces.set(record, workspace);
  }
  return workspace;
}

/**
 * One row per agent terminal, in workspace order then saved pane order, so
 * rows never reshuffle as activity changes. A running agent whose tab was
 * closed stays listed (choosing it reopens the tab); a finished one only while
 * its tab is still open.
 */
export function agentOverviewRows(workspaces: readonly AgentOverviewWorkspace[],
    terminals: ReadonlyMap<string, AgentOverviewTerminal>): AgentOverviewRow[] {
  const rows: AgentOverviewRow[] = [];
  const seen = new Set<string>();
  for (const workspace of workspaces) {
    for (const id of workspace.terminalIds) {
      const terminal = terminals.get(id);
      if (terminal === undefined || seen.has(id) || !isAgentProfile(terminal.profileId)) continue;
      const status = agentOverviewStatus(terminal.state, terminal.activity, terminal.done === true);
      if (status === "stopped" && !workspace.attachedIds.has(id)) continue;
      seen.add(id);
      rows.push({
        terminalId: id,
        title: terminal.title.trim() || "Terminal",
        profileId: terminal.profileId,
        ...(terminal.sessionKey === undefined ? {} : { sessionKey: terminal.sessionKey }),
        workspaceId: workspace.id,
        workspaceName: workspace.name,
        status,
        state: terminal.state,
      });
    }
  }
  return rows;
}

export function agentOverviewCounts(rows: readonly AgentOverviewRow[]): AgentOverviewCounts {
  const counts = { all: rows.length, waiting: 0, working: 0, done: 0, idle: 0, stopped: 0 };
  for (const row of rows) counts[row.status] += 1;
  return counts;
}

/** Non-empty groups in urgency order; a filter keeps only its own group. */
export function agentOverviewGroups(rows: readonly AgentOverviewRow[],
    filter: AgentOverviewFilter): AgentOverviewGroup[] {
  return AGENT_OVERVIEW_ORDER
    .filter(status => filter === "all" || status === filter)
    .map(status => ({ status, rows: rows.filter(row => row.status === status) }))
    .filter(group => group.rows.length > 0);
}

/** "2 need you · 1 working · 9 idle": the parts that are not zero. */
export function agentOverviewSummary(counts: AgentOverviewCounts): string {
  if (counts.all === 0) return "No agent panes running";
  const parts = [
    counts.waiting > 0 ? `${counts.waiting.toString()} ${counts.waiting === 1 ? "needs" : "need"} you` : "",
    counts.working > 0 ? `${counts.working.toString()} working` : "",
    counts.done > 0 ? `${counts.done.toString()} done` : "",
    counts.idle > 0 ? `${counts.idle.toString()} idle` : "",
    counts.stopped > 0 ? `${counts.stopped.toString()} stopped` : "",
  ];
  return parts.filter(part => part !== "").join(" · ");
}

/** A row whose title, workspace or provider contains the query, ignoring case. */
export function agentOverviewMatches(row: AgentOverviewRow, query: string, provider: string): boolean {
  const needle = query.trim().toLocaleLowerCase();
  if (needle === "") return true;
  return [row.title, row.workspaceName, provider]
    .some(text => text.toLocaleLowerCase().includes(needle));
}

/** One group per workspace, in the order the rows list them. */
export interface AgentOverviewProjectGroup {
  readonly workspaceId: string;
  readonly name: string;
  readonly rows: readonly AgentOverviewRow[];
}

/** The Project grouping: workspaces in roster order, each row once. */
export function agentOverviewProjectGroups(rows: readonly AgentOverviewRow[]): AgentOverviewProjectGroup[] {
  const groups = new Map<string, { name: string; rows: AgentOverviewRow[] }>();
  for (const row of rows) {
    const group = groups.get(row.workspaceId);
    if (group === undefined) groups.set(row.workspaceId, { name: row.workspaceName, rows: [row] });
    else group.rows.push(row);
  }
  return [...groups].map(([workspaceId, group]) => ({ workspaceId, ...group }));
}

/** When an agent entered its current status; null when that was before we looked. */
export interface AgentOverviewSince {
  readonly status: AgentOverviewStatus;
  readonly since: number | null;
}

/**
 * Carries each agent's "since" forward. A status first seen is dated null,
 * because the agent may have been in it long before this window opened, and
 * an invented clock is worse than none; only a change this window watched
 * starts one. Terminals no longer listed are forgotten. Returns the same map
 * when nothing moved.
 */
export function agentOverviewSince(previous: ReadonlyMap<string, AgentOverviewSince>,
    entries: readonly { readonly id: string; readonly status: AgentOverviewStatus }[],
    now: number): ReadonlyMap<string, AgentOverviewSince> {
  let changed = entries.length !== previous.size;
  const next = new Map<string, AgentOverviewSince>();
  for (const entry of entries) {
    const before = previous.get(entry.id);
    if (before === undefined) {
      next.set(entry.id, { status: entry.status, since: null });
      changed = true;
    } else if (before.status !== entry.status) {
      next.set(entry.id, { status: entry.status, since: now });
      changed = true;
    } else next.set(entry.id, before);
  }
  return changed ? next : previous;
}

/** "<1m", "4m", "1h 5m", "3d": how long, for a card's clock. */
export function agentOverviewElapsed(since: number | null | undefined, now: number): string | null {
  if (since === null || since === undefined) return null;
  const minutes = Math.floor(Math.max(0, now - since) / 60_000);
  if (minutes < 1) return "<1m";
  if (minutes < 60) return `${minutes.toString()}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return minutes % 60 === 0 ? `${hours.toString()}h` : `${hours.toString()}h ${(minutes % 60).toString()}m`;
  return `${Math.floor(hours / 24).toString()}d`;
}
