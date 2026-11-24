import { useEffect, useState } from "react";
import type { OrchestrationSnapshotDto } from "../../shared/ipc";
import { ORCHESTRATOR_ROLE } from "../../shared/orchestration";
import { orchestrationStatusWord } from "../canvas-layout";

type FeedAgent = OrchestrationSnapshotDto["agents"][number];

export interface AgentFeedRow {
  agentId: string;
  name: string;
  engineId: string;
  state: string;
  /** Present only when the agent has a provider session to open. */
  sessionKey?: string;
  /** A worker under an orchestrator, as opposed to a lead or a lone session. */
  worker: boolean;
  /** ISO 8601. The clock starts when the agent was created, not when it was seen. */
  createdAt: string;
  /** ISO 8601. What a settled agent's elapsed time is measured to. */
  updatedAt: string;
}

/** States whose clock is still running. Everything else is done and frozen. */
const LIVE_STATES = new Set(["planned", "starting", "ready", "working", "waiting"]);
const TICK_MS = 1_000;

/** Only live states reach a row, so this is the whole vocabulary. */
function tone(state: string): string {
  return state === "waiting" ? "waiting" : "working";
}

/**
 * `2m14s`, not `134s` and not `00:02:14`. The number people read off a feed is
 * "how long has this been going", and past an hour the seconds stop mattering.
 */
export function elapsedLabel(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1_000));
  const hours = Math.floor(total / 3_600);
  const minutes = Math.floor(total % 3_600 / 60);
  const seconds = total % 60;
  if (hours > 0) return `${hours}h${String(minutes).padStart(2, "0")}m`;
  if (minutes > 0) return `${minutes}m${String(seconds).padStart(2, "0")}s`;
  return `${seconds}s`;
}

/**
 * What is running right now, newest first — a spawn is an event, and the thing
 * that just happened is what the human is looking for. Orchestrators are not
 * hoisted above their workers: that would bury the worker that just started
 * under a lead that has been sitting there all session.
 *
 * Settled agents are dropped rather than greyed out. The durable record keeps
 * every agent this host has ever run, including ones a previous app run
 * quarantined, so a feed that showed them was a list of yesterday's sessions
 * with today's at the top — and it never emptied, so it never went away.
 */
export function agentFeedRows(snapshot: OrchestrationSnapshotDto | null): AgentFeedRow[] {
  const agents: readonly FeedAgent[] = snapshot?.agents ?? [];
  return [...agents]
    .filter(agent => LIVE_STATES.has(agent.state))
    .sort((left, right) => right.createdAt.localeCompare(left.createdAt))
    .map(agent => ({
      agentId: agent.agentId,
      name: agent.name ?? agent.agentId,
      engineId: agent.engineId,
      state: agent.state,
      ...(agent.sessionKey === undefined ? {} : { sessionKey: agent.sessionKey }),
      worker: agent.parentAgentId !== undefined && agent.role !== ORCHESTRATOR_ROLE,
      createdAt: agent.createdAt,
      updatedAt: agent.updatedAt,
    }));
}

export interface AgentFeedProps {
  rows: readonly AgentFeedRow[];
  onOpen: (sessionKey: string) => void;
  /** The app is showing a modal surface; the feed must not sit on top of it. */
  hidden?: boolean;
}

/**
 * Every agent this app is running, wherever the human happens to be looking.
 * The canvas draws the same agents as a graph, but only for the project it has
 * selected and only while the canvas is the visible pane — so an orchestrator
 * spawning three workers while the human reads a diff was, until this panel,
 * something they had to go and look for.
 */
export function AgentFeed(props: AgentFeedProps) {
  const [collapsed, setCollapsed] = useState(false);
  const [now, setNow] = useState(() => Date.now());
  const running = props.rows.length > 0;
  // Nothing on screen means nothing to tick, and the panel is gone anyway.
  useEffect(() => {
    if (!running) return;
    const id = setInterval(() => { setNow(Date.now()); }, TICK_MS);
    return () => { clearInterval(id); };
  }, [running]);
  if (props.rows.length === 0 || props.hidden === true) return null;
  const live = props.rows.length;
  return <section className="agent-feed is-floating" aria-label="Running agents">
    <style>{AGENT_FEED_CSS}</style>
    <header className="agent-feed-head">
      <button type="button" className="agent-feed-toggle"
        aria-expanded={!collapsed}
        onClick={() => { setCollapsed(value => !value); }}>
        <span className={`codicon codicon-sm codicon-chevron-${collapsed ? "up" : "down"}`}
          aria-hidden="true" />
        <span className="agent-feed-title">Agents</span>
        <span className="agent-feed-count">{live}</span>
      </button>
    </header>
    {!collapsed && <ul className="agent-feed-rows">
      {props.rows.map(row => {
        // Every row here is running, so every clock runs to now.
        const started = Date.parse(row.createdAt);
        const elapsed = Number.isNaN(started) ? "" : elapsedLabel(now - started);
        const key = row.sessionKey;
        const label = `${row.name}, ${row.engineId}, ${orchestrationStatusWord(row.state)}`;
        return <li key={row.agentId}
          className={row.worker ? "agent-feed-row agent-feed-worker" : "agent-feed-row"}>
          <button type="button" aria-label={label} disabled={key === undefined}
            onClick={() => { if (key !== undefined) props.onOpen(key); }}>
            <span className={`agent-feed-dot agent-feed-${tone(row.state)}`}
              aria-hidden="true" />
            <span className="agent-feed-name">{row.name}</span>
            <span className="agent-feed-engine">{row.engineId}</span>
            <span className="agent-feed-state">{orchestrationStatusWord(row.state)}</span>
            <span className="agent-feed-elapsed">{elapsed}</span>
          </button>
        </li>;
      })}
    </ul>}
  </section>;
}

export const AGENT_FEED_CSS = `
.agent-feed { position:fixed; right:16px; bottom:16px; z-index:6; display:flex;
  flex-direction:column; width:min(340px, calc(100vw - 32px)); max-height:min(46vh, 420px);
  overflow:hidden; border:var(--border-w) solid var(--border-strong);
  border-radius:var(--radius-overlay); background:var(--menu-bg);
  box-shadow:var(--overlay-shadow); color:var(--fg); font-family:var(--font-ui);
  font-size:var(--text-meta); }
.agent-feed-head { flex:none; }
.agent-feed-toggle { display:flex; align-items:center; gap:6px; width:100%; height:var(--control-h);
  padding:0 8px; border:none; background:transparent; color:var(--fg); font:inherit;
  cursor:pointer; text-align:left; }
.agent-feed-toggle:hover { background:var(--row-hover); }
/* Inset: .agent-feed clips with overflow:hidden and these buttons run its full
   width, so an outset ring is cropped on three sides. Same token, negated. */
.agent-feed-toggle:focus-visible, .agent-feed-row button:focus-visible {
  outline:var(--focus-ring-w) solid var(--focus);
  outline-offset:calc(-1 * var(--focus-ring-offset)); }
.agent-feed-title { font-weight:var(--wght-strong); letter-spacing:var(--track-caps);
  text-transform:uppercase; }
.agent-feed-count { margin-left:auto; color:var(--fg-dim); font-variant-numeric:tabular-nums; }
.agent-feed-rows { min-height:0; margin:0; padding:4px; overflow-y:auto; list-style:none;
  border-top:var(--border-w) solid var(--border); }
.agent-feed-row button { display:grid;
  grid-template-columns:8px minmax(0,1fr) auto auto auto; column-gap:8px; align-items:center;
  width:100%; padding:5px 6px; border:none; border-radius:var(--radius-control);
  background:transparent; color:var(--fg); font:inherit; cursor:pointer; text-align:left; }
.agent-feed-row button:hover:not(:disabled) { background:var(--row-hover); }
.agent-feed-row button:disabled { cursor:default; }
.agent-feed-worker button { padding-left:16px; }
.agent-feed-dot { width:8px; height:8px; border-radius:var(--radius-pill);
  background:currentColor; }
.agent-feed-working { color:var(--status-working); }
.agent-feed-waiting { color:var(--status-waiting); }
.agent-feed-name { min-width:0; overflow:hidden; color:var(--fg); text-overflow:ellipsis;
  white-space:nowrap; }
.agent-feed-engine, .agent-feed-state { color:var(--fg-dim); }
.agent-feed-elapsed { color:var(--fg-dim); font-variant-numeric:tabular-nums; }
@media (max-width:900px) { .agent-feed { right:8px; bottom:8px; } }
`;
