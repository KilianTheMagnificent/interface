import { useEffect, useState, type AnimationEvent, type ReactNode } from "react";
import {
  agentOverviewCounts, agentOverviewElapsed, agentOverviewGroups, agentOverviewMatches,
  agentOverviewProjectGroups, agentOverviewSummary, AGENT_OVERVIEW_ORDER,
  type AgentOverviewFilter, type AgentOverviewGrouping, type AgentOverviewRow,
  type AgentOverviewSince, type AgentOverviewStatus,
} from "../agent-overview";
import { presenceClass, type PresencePhase } from "../presence";
import { DONE_WORD, statusWord } from "./status-words";
import { TerminalProfileIcon } from "./TerminalMenu";

const STATUS_LABEL: Readonly<Record<AgentOverviewStatus, string>> = {
  waiting: statusWord("waiting"),
  working: statusWord("working"),
  done: DONE_WORD,
  idle: statusWord("idle"),
  stopped: "Stopped",
};

const FILTERS: readonly AgentOverviewFilter[] = ["all", "waiting", "working", "done", "idle"];

/** What the column shows: the agents, or its Browser. */
export type AgentOverviewTab = "agents" | "browser";

/** How many of an agent's last lines each live card shows. */
const TAIL_LINES: Readonly<Record<AgentOverviewStatus, number>> = {
  waiting: 3, working: 2, done: 1, idle: 0, stopped: 0,
};

/** The cards' clock prints whole minutes; half a minute keeps it honest. */
const CLOCK_MS = 30_000;

function filterLabel(filter: AgentOverviewFilter): string {
  return filter === "all" ? "All" : STATUS_LABEL[filter];
}

/** The shared status glyph. Idle and exited draw none, as in the sidebar. */
function dotClass(row: Pick<AgentOverviewRow, "status" | "state">): string | null {
  if (row.status === "stopped") return row.state === "failed" ? "dot dot-failed" : null;
  return row.status === "idle" ? null : `dot dot-${row.status}`;
}

function rowWord(row: AgentOverviewRow): string {
  if (row.status !== "stopped") return STATUS_LABEL[row.status];
  return row.state === "failed" ? "Failed" : "Exited";
}

export interface AgentOverviewProps {
  readonly rows: readonly AgentOverviewRow[];
  /** "Claude Code" for "claude": the launcher roster's own label. */
  readonly profileLabel: (profileId: string) => string;
  /** Go to the pane: its workspace, its tab, then one pulse. */
  readonly onLocate: (row: AgentOverviewRow) => void;
  readonly onClose: () => void;
  /** Each agent's last screen lines, oldest first (the live cards). */
  readonly tails?: ReadonlyMap<string, readonly string[]>;
  /** When each agent entered its status; null when that was before we looked. */
  readonly since?: ReadonlyMap<string, AgentOverviewSince>;
  /** End these idle agents after the user confirms. Absent: no Close idle. */
  readonly onCloseIdle?: (terminalIds: readonly string[]) => void;
  /** The Tasks tab beside Agents. Absent: no tab. */
  readonly onOpenTasks?: () => void;
  /** The Browser tab's body. Absent: no Browser tab. */
  readonly browser?: ReactNode;
  /** The tab showing; Agents when absent or there is no Browser. */
  readonly tab?: AgentOverviewTab;
  readonly onTabChange?: (tab: AgentOverviewTab) => void;
  /** Sliding in after the toggle, or out (inert) after a close. */
  readonly phase?: PresencePhase;
  readonly onAnimationEnd?: (event: AnimationEvent<Element>) => void;
  /** Test-only; the chips start on All. */
  readonly initialFilter?: AgentOverviewFilter;
  /** Test-only; the list starts grouped by status. */
  readonly initialGrouping?: AgentOverviewGrouping;
  /** Test-only clock. */
  readonly now?: number;
}

/**
 * Every agent pane across every workspace (owner's BridgeMind dashboard
 * reference, 2026-09-23; dense live cards, 2026-10-01). A card shows what its
 * agent is doing now — its last screen lines and how long it has been at it —
 * and choosing one goes to that pane. The rows come from the terminal center;
 * this only draws them. Its Browser tab (owner, 2026-10-14) shows a Browser
 * in the column instead, kept loaded behind Agents once it has been opened.
 */
export function AgentOverview({ rows, profileLabel, onLocate, onClose, tails, since, onCloseIdle,
    onOpenTasks, browser, tab = "agents", onTabChange, phase = "still", onAnimationEnd,
    initialFilter = "all", initialGrouping = "status", now: fixedNow }: AgentOverviewProps) {
  const [filter, setFilter] = useState<AgentOverviewFilter>(initialFilter);
  const [grouping, setGrouping] = useState<AgentOverviewGrouping>(initialGrouping);
  const [query, setQuery] = useState("");
  const [confirmingIdle, setConfirmingIdle] = useState(false);
  const showing = browser === undefined ? "agents" : tab;
  const [clock, setClock] = useState(() => Date.now());
  useEffect(() => {
    if (fixedNow !== undefined || showing !== "agents" || rows.length === 0) return undefined;
    const timer = window.setInterval(() => { setClock(Date.now()); }, CLOCK_MS);
    return () => { window.clearInterval(timer); };
  }, [fixedNow, rows.length, showing]);
  const now = fixedNow ?? clock;
  // Its page loads the first time Browser is chosen, and stays loaded behind Agents.
  const [browserOpened, setBrowserOpened] = useState(showing === "browser");
  if (showing === "browser" && !browserOpened) setBrowserOpened(true);

  const counts = agentOverviewCounts(rows);
  const shown = rows.filter(row => agentOverviewMatches(row, query, profileLabel(row.profileId)));
  const idleIds = rows.filter(row => row.status === "idle").map(row => row.terminalId);
  const meter = AGENT_OVERVIEW_ORDER.filter(status => status !== "stopped" && counts[status] > 0);

  const card = (row: AgentOverviewRow) => {
    const provider = profileLabel(row.profileId);
    const dot = dotClass(row);
    const elapsed = agentOverviewElapsed(since?.get(row.terminalId)?.since, now);
    const take = TAIL_LINES[row.status];
    const lines = take === 0 ? [] : (tails?.get(row.terminalId) ?? []).slice(-take);
    const compact = row.status === "idle" || row.status === "stopped";
    return (
      <li key={row.terminalId}>
        <button type="button"
          className={`agent-overview-row is-${row.status}${compact ? " is-compact" : ""}`}
          data-terminal-id={row.terminalId}
          aria-label={`${row.title}, ${provider}, ${row.workspaceName}, ${rowWord(row)}${elapsed === null ? "" : `, ${elapsed}`}`}
          title={`Go to ${row.title}`}
          onClick={() => { onLocate(row); }}>
          <span className="agent-overview-row-head">
            <span className="agent-overview-row-icon" aria-hidden="true">
              <TerminalProfileIcon profileId={row.profileId} sessionKey={row.sessionKey}
                providerLabel={provider} size={14} />
              {dot !== null && <span className={`${dot} agent-overview-row-light`} />}
            </span>
            <span className="agent-overview-row-text">
              <span className="agent-overview-row-title">{row.title}</span>
              <span className="agent-overview-row-meta">{provider} · {row.workspaceName}</span>
            </span>
            <span className="badge badge-quiet agent-overview-row-status">
              {dot !== null && <span className={dot} aria-hidden="true" />}
              {rowWord(row)}{elapsed === null || compact ? "" : ` ${elapsed}`}
            </span>
          </span>
          {lines.length > 0 && (row.status === "done"
            ? <span className="agent-overview-row-summary">{lines[lines.length - 1]}</span>
            : (
              <span className="agent-overview-row-tail" aria-hidden="true">
                {lines.map((line, at) => <span key={at}>{line}</span>)}
              </span>
            ))}
          {row.status === "waiting" && (
            <span className="agent-overview-row-open" aria-hidden="true">Open pane <kbd>⏎</kbd></span>
          )}
        </button>
      </li>
    );
  };

  const statusGroups = agentOverviewGroups(shown, filter);
  const projectGroups = agentOverviewProjectGroups(filter === "all" ? shown
    : shown.filter(row => row.status === filter));
  const empty = grouping === "status" ? statusGroups.length === 0 : projectGroups.length === 0;

  return (
    <aside className={`agent-overview${presenceClass(phase)}`} aria-label="Live panes"
      inert={phase === "leaving" || undefined} onAnimationEnd={onAnimationEnd}>
      <div className="agent-overview-tabs" role="group" aria-label="Overview">
        <button type="button" className={`agent-overview-tab${showing === "agents" ? " is-active" : ""}`}
          aria-pressed={showing === "agents"} onClick={() => { onTabChange?.("agents"); }}>
          <span className="codicon codicon-layout-grid" aria-hidden="true" />
          Panes<span className="agent-overview-tab-count">{counts.all}</span>
        </button>
        {onOpenTasks !== undefined && (
          <button type="button" className="agent-overview-tab" aria-pressed="false" onClick={onOpenTasks}
            title="Open the Agent tab's task list">
            <span className="codicon codicon-checklist" aria-hidden="true" />Agent tasks
          </button>
        )}
        {browser !== undefined && (
          <button type="button" className={`agent-overview-tab${showing === "browser" ? " is-active" : ""}`}
            aria-pressed={showing === "browser"} onClick={() => { onTabChange?.("browser"); }}>
            <span className="codicon codicon-globe" aria-hidden="true" />Browser
          </button>
        )}
        <button type="button" className="terminal-center-action agent-overview-close"
          aria-label="Close live panes" title="Close" onClick={onClose}>
          <span className="codicon codicon-close" aria-hidden="true" />
        </button>
      </div>
      {showing === "agents" && <>
      <header className="agent-overview-head">
        <span className="agent-overview-total">{counts.all}</span>
        <span className="agent-overview-heading">
          <strong>{counts.all === 1 ? "live pane" : "live panes"}</strong>
          <span>{agentOverviewSummary(counts)}</span>
        </span>
        <span className="agent-overview-grouping" role="group" aria-label="Group panes by">
          {(["status", "project"] as const).map(option => (
            <button key={option} type="button" aria-pressed={grouping === option}
              className={grouping === option
                ? "agent-overview-grouping-option is-active" : "agent-overview-grouping-option"}
              onClick={() => { setGrouping(option); }}>
              {option === "status" ? "Status" : "Project"}
            </button>
          ))}
        </span>
      </header>
      <div className="agent-overview-meter" aria-hidden="true">
        {meter.map(status => (
          <span key={status} className={`is-${status}`} style={{ flexGrow: counts[status] }} />
        ))}
      </div>
      <div className="agent-overview-filters" role="group" aria-label="Show panes">
        {FILTERS.map(option => {
          const dot = option === "all" ? null : dotClass({ status: option, state: "running" });
          return (
            <button key={option} type="button"
              className={option === filter ? "agent-overview-filter is-active" : "agent-overview-filter"}
              aria-pressed={option === filter}
              onClick={() => { setFilter(option); }}>
              {dot !== null && <span className={dot} aria-hidden="true" />}
              <span>{filterLabel(option)}</span>
              <span className="agent-overview-filter-count">{counts[option]}</span>
            </button>
          );
        })}
      </div>
      <div className="agent-overview-tools">
        <label className="agent-overview-search">
          <span className="codicon codicon-search" aria-hidden="true" />
          <span className="terminal-sr-only">Search panes</span>
          <input type="search" value={query} placeholder="Search panes"
            onChange={event => { setQuery(event.currentTarget.value); }} />
        </label>
        {onCloseIdle !== undefined && idleIds.length > 0 && (
          <button type="button" className="agent-overview-close-idle"
            aria-expanded={confirmingIdle}
            onClick={() => { setConfirmingIdle(open => !open); }}>
            <span className="codicon codicon-close" aria-hidden="true" />
            Close idle ({idleIds.length})
          </button>
        )}
      </div>
      {confirmingIdle && onCloseIdle !== undefined && idleIds.length > 0 && (
        <div className="agent-overview-confirm" role="group" aria-label="Close idle panes"
          onKeyDown={event => { if (event.key === "Escape") { event.stopPropagation(); setConfirmingIdle(false); } }}>
          <span>
            End {idleIds.length} idle {idleIds.length === 1 ? "pane" : "panes"}? Their
            conversations stay in Saved sessions.
          </span>
          <span className="agent-overview-confirm-actions">
            <button type="button" className="btn" onClick={() => { setConfirmingIdle(false); }}>Cancel</button>
            <button type="button" className="btn btn-primary"
              onClick={() => { setConfirmingIdle(false); onCloseIdle(idleIds); }}>End them</button>
          </span>
        </div>
      )}
      <div className="agent-overview-list">
        {empty && (
          <p className="empty agent-overview-empty">
            {counts.all === 0 ? "No agent panes are running in any workspace."
              : query.trim() !== "" ? "No panes match."
                : `No panes are ${filterLabel(filter).toLowerCase()}.`}
          </p>
        )}
        {grouping === "status" ? statusGroups.map(group => (
          <section key={group.status} className={`agent-overview-group is-${group.status}`}
            aria-label={`${STATUS_LABEL[group.status]}, ${group.rows.length.toString()}`}>
            <h3 className="agent-overview-group-label">
              {group.status !== "idle" && group.status !== "stopped"
                && <span className={`dot dot-${group.status}`} aria-hidden="true" />}
              <span>{STATUS_LABEL[group.status]}</span>
              <span className="agent-overview-group-count">{group.rows.length}</span>
            </h3>
            <ul className="agent-overview-rows">
              {group.rows.map(row => card(row))}
            </ul>
          </section>
        )) : projectGroups.map(group => (
          <section key={group.workspaceId} className="agent-overview-group"
            aria-label={`${group.name}, ${group.rows.length.toString()}`}>
            <h3 className="agent-overview-group-label">
              <span>{group.name}</span>
              <span className="agent-overview-group-count">{group.rows.length}</span>
            </h3>
            <ul className="agent-overview-rows">
              {group.rows.map(row => card(row))}
            </ul>
          </section>
        ))}
      </div>
      </>}
      {browserOpened && browser !== undefined && (
        <div className="agent-overview-browser" hidden={showing !== "browser"}>{browser}</div>
      )}
    </aside>
  );
}
