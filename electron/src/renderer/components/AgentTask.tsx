import { WorkingIndicator } from "./WorkingIndicator";
import { useContext, useId, useMemo, useRef, useState, type ReactNode } from "react";
import {
  sessionKey, type EngineEvent, type PlanEntry, type ProviderCommand, type SessionSummary,
} from "../../shared/engine";
import {
  agentProgressFromItems, agentTaskItems, type AgentProgress, type AgentStepState,
} from "../agent-task";
import { latestPlanEntries, type ConvItem } from "../conversation-reducer";
import { conversationProjectionFor } from "../conversation-projection";
import { PaneVisibleContext } from "./ConversationPaneSurface";
import { formatDiffStat, type DiffStat } from "../diff-stats";
import { lastSegment } from "../display-paths";
import { formatTurnElapsed } from "../session-activity";
import { statusWord } from "./status-words";

export interface AgentTaskProps {
  session: SessionSummary;
  /**
   * The task's live event log, the one the conversation below is projected
   * from. Progress and Files read it, with `history`, only while the Activity
   * inspector is open and on screen; a streamed frame costs a closed or hidden
   * inspector nothing.
   */
  events: EngineEvent[];
  /**
   * The stored transcript from before this window's live log, and the newest
   * plan it reported. Absent while it loads and for a task this app run
   * started. Progress reads the items past then live, not the live log alone,
   * or a session the app did not start has an empty panel over a finished task.
   */
  history?: { readonly items: readonly ConvItem[]; readonly plan?: readonly PlanEntry[] };
  /** Undefined is not yet reported; an empty array is an advertised empty catalog. */
  commands?: readonly ProviderCommand[];
  engineLabel: string;
  live: boolean;
  /** When the running turn started (ms epoch), or null when idle. */
  turnStartedAt: number | null;
  now: number;
  onStop: () => void;
  onOpenDiff: (path: string) => void;
  /**
   * Lines added/removed per touched path, from the caller's own count — the
   * fallback, not the answer. What the ITEMS reported wins where they have one:
   * they carry history and live both, while this map is summed from the live
   * event log alone and so undercounts a session the app did not start. A path
   * neither measures prints nothing.
   */
  diffStats?: Readonly<Record<string, DiffStat>>;
  /** The transcript pane the App already builds for a tile. */
  children: ReactNode;
  /** Opens the Activity inspector on mount, for static renders. The app starts it closed. */
  initialInspectorOpen?: boolean;
}

/**
 * Text under the task's own folder, shown relative to it — what Conversation's
 * `pathRoot` already does for a transcript row, and for the same reason: 40-odd
 * identical leading characters on every line are not information. Anything not
 * under that folder is left whole, and the absolute form stays on the `title`.
 */
function relativeTo(root: string, text: string): string {
  const base = root.replace(/[\\/]+$/u, "");
  return base !== "" && (text.startsWith(`${base}/`) || text.startsWith(`${base}\\`))
    ? text.slice(base.length + 1) : text;
}

/* One glyph per step state. Running takes the working-status dot rather than
   codicon-modifier-spin: a task can have several calls in flight at once, and
   the spinner is this app's one-at-a-time affordance (see codicons.css). */
const STEP_GLYPH: Record<AgentStepState, string> = {
  ok: "codicon-check",
  failed: "codicon-error",
  running: "codicon-circle-filled",
  ended: "codicon-circle-filled",
};

const NO_DIFF_STATS: Readonly<Record<string, DiffStat>> = {};
const NO_ITEMS: readonly ConvItem[] = [];

/**
 * The Agent surface's task view: what the agent was asked to do, how far it
 * has got and what it touched. Conversation remains the primary work surface;
 * the user can inspect activity without hiding the composer or approvals.
 */
export function AgentTask({ session, events, history, commands, engineLabel, live, turnStartedAt,
    now, onStop, onOpenDiff, diffStats = NO_DIFF_STATS, children,
    initialInspectorOpen = false }: AgentTaskProps) {
  const [inspectorOpen, setInspectorOpen] = useState(initialInspectorOpen);
  const inspectorId = useId();
  const visible = useContext(PaneVisibleContext);
  const key = sessionKey(session.engine, session.sessionId);
  // Read only while someone can see it. The projection is the session's shared
  // one, so reading it here costs the pane below nothing extra.
  const computed = useMemo(() => inspectorOpen && visible
    ? agentProgressFromItems(
      agentTaskItems(history?.items ?? NO_ITEMS, conversationProjectionFor(key)(events)),
      latestPlanEntries(events) ?? history?.plan, session.status)
    : null, [inspectorOpen, visible, key, events, history, session.status]);
  // An open inspector on a hidden surface keeps what it last drew, and with it
  // its scroll position, until it is shown again. A derived-value cache: any
  // render that wrote it wrote a progress that was true of its own inputs.
  const drawn = useRef<AgentProgress | null>(null);
  drawn.current = inspectorOpen ? computed ?? drawn.current : null;
  const progress = drawn.current;
  const running = live && (session.status === "working" || session.status === "waiting");
  const elapsed = running && turnStartedAt !== null ? formatTurnElapsed(now - turnStartedAt) : "";

  return (
    <section className="agent-task" aria-label="Agent task">
      <style>{AGENT_TASK_CSS}</style>

      <header className="agent-task-head">
        <h1 className="agent-task-title">{session.title}</h1>
        <div className="agent-task-meta">
          <span className="agent-task-chip" title={session.cwd}>
            <span className="codicon codicon-sm codicon-folder" aria-hidden="true" />
            {lastSegment(session.cwd) || session.cwd}
          </span>
          <span className="agent-task-sep" aria-hidden="true">·</span>
          {/* An empty model is not an unknown one: Codex and Grok both report
              `model: ""`, and the registry passes it through unnormalised, so
              this guards both the way engineLine() in AgentCanvas.tsx does. */}
          <span className="agent-task-chip">
            {session.model !== undefined && session.model !== ""
              ? `${engineLabel} · ${session.model}` : engineLabel}
          </span>
          <span className="agent-task-sep" aria-hidden="true">·</span>
          <span className={`agent-task-status is-${session.status}`}>
            {live && session.status === "working" && <WorkingIndicator className="working-refined-only" />}
            {elapsed === "" ? statusWord(session.status) : `${statusWord(session.status)} · ${elapsed}`}
          </span>
          <button type="button" className="agent-task-inspect"
            aria-expanded={inspectorOpen} aria-controls={inspectorId}
            onClick={() => { setInspectorOpen(open => !open); }}>
            <span className="codicon codicon-sm codicon-list-tree" aria-hidden="true" />
            Activity &amp; tools
          </button>
          {running && (
            <button type="button" className="agent-task-stop" aria-label="Stop the task"
              onClick={onStop}>
              <span className="codicon codicon-sm codicon-debug-stop" aria-hidden="true" />
              Stop
            </button>
          )}
        </div>
      </header>

      <div className="agent-task-body">
      <section className="agent-task-conversation" aria-label="Conversation">
        {children}
      </section>
      <aside id={inspectorId} className="agent-task-inspector" aria-label="Activity and tools"
        hidden={!inspectorOpen}>
      {progress !== null && <>
      <section className="agent-task-progress" aria-label="Progress">
        <h2 className="agent-task-section">Progress</h2>
        {progress.plan !== undefined && progress.plan.length > 0 && (
          <ol className="agent-task-plan">
            {progress.plan.map((entry, index) => (
              <li key={index} className={`agent-task-plan-item is-${entry.status}`}>
                {entry.content}
              </li>
            ))}
          </ol>
        )}
        {progress.steps.length > 0 ? (
          <ol className="agent-task-steps">
            {progress.steps.map(step => (
              <li key={step.key} className={`agent-task-step is-${step.state}`}>
                <WorkingIndicator working={step.state === "running"}
                  className={`codicon codicon-sm ${STEP_GLYPH[step.state]} agent-task-step-mark`}
                  aria-hidden="true" />
                <span className="agent-task-step-tool">{step.tool}</span>
                <span className="agent-task-step-label" title={step.label}>
                  {relativeTo(session.cwd, step.label)}
                </span>
              </li>
            ))}
          </ol>
        ) : <p className="agent-task-empty">{running ? "Starting…" : "No activity yet."}</p>}
      </section>

      {progress.files.length > 0 && (
        <section className="agent-task-files" aria-label="Files touched">
          <h2 className="agent-task-section">Files touched</h2>
          <ul>
            {progress.files.map(path => {
              // Items first: they span history AND live, so they answer for a
              // file touched before this window opened. `diffStats` only backs
              // up a path the items left unmeasured.
              const stat = progress.fileStats[path] ?? diffStats[path];
              return (
                <li key={path}>
                  <button type="button" className="agent-task-file" title={path}
                    onClick={() => { onOpenDiff(path); }}>
                    {relativeTo(session.cwd, path)}
                  </button>
                  {stat !== undefined && formatDiffStat(stat) !== "" && (<>
                    <span className="agent-task-file-add">+{stat.added}</span>
                    <span className="agent-task-file-del">−{stat.removed}</span>
                  </>)}
                </li>
              );
            })}
          </ul>
        </section>
      )}

      <section className="agent-task-commands" aria-label="Skills and commands">
        <h2 className="agent-task-section" title={`Type / in the conversation to use a command reported by ${engineLabel}. Tool access is managed by ${engineLabel}; Activity shows the tools used in this task.`}>Skills &amp; commands</h2>
        {commands === undefined ? <p className="agent-task-empty">{engineLabel} has not reported commands for this task.</p>
          : commands.length === 0 ? <p className="agent-task-empty">No commands advertised for this task.</p>
          : <>
            <ul>{commands.map(command => <li key={command.name}>
              <code>/{command.name}</code>
              <span>{command.description}</span>
            </li>)}</ul>
          </>}
      </section>
      </>}
      </aside>
      </div>
    </section>
  );
}

/* ---- styles --------------------------------------------------------------
   Beside the markup they style, per this project's convention, and every value
   is a token: no colour literal appears here. --success-fg does not exist in
   theme.css, so a settled step's check takes the quiet --fg-muted tier rather
   than inventing a hue. */
export const AGENT_TASK_CSS = `
.agent-task { display:flex; flex-direction:column; gap:12px; height:100%; padding:16px 18px;
  box-sizing:border-box; min-height:0; min-width:0; overflow:hidden; container-type:inline-size;
  background:var(--editor-bg); color:var(--fg); font-family:var(--font-ui); font-size:var(--text-body); }
.agent-task-body { display:flex; flex:1; gap:18px; min-height:0; min-width:0; }
.agent-task-conversation { display:flex; flex:1; min-width:0; min-height:0; }
.agent-task-conversation > * { flex:1; min-width:0; min-height:0; }
.agent-task-inspector { display:flex; flex-direction:column; gap:20px; width:280px;
  max-width:38%; flex:none; min-height:0; overflow:auto; padding:4px 0 4px 16px;
  border-left:var(--border-w) solid var(--border); }
.agent-task-inspector[hidden] { display:none; }
.agent-task-inspect { display:inline-flex; align-items:center; gap:6px; margin-left:auto;
  min-height:var(--control-h); padding:0 9px; color:var(--fg-muted); background:transparent;
  border:var(--border-w) solid transparent; border-radius:var(--radius-control);
  font:inherit; cursor:pointer; }
.agent-task-inspect:hover, .agent-task-inspect[aria-expanded="true"] { background:var(--row-hover); color:var(--fg-strong); }
.agent-task-inspect:focus-visible { outline:var(--focus-ring-w) solid var(--focus); outline-offset:var(--focus-ring-offset); }
.agent-task-commands { display:flex; flex-direction:column; gap:8px; }
.agent-task-commands ul { list-style:none; margin:0; padding:0; display:flex; flex-direction:column; gap:12px; }
.agent-task-commands li { display:flex; flex-direction:column; gap:3px; overflow-wrap:anywhere; font-size:var(--text-meta); }
.agent-task-commands code { color:var(--fg-strong); font-family:var(--font-mono); }
.agent-task-commands li > span { color:var(--fg-dim); }
@container (max-width:760px) {
  .agent-task-body { flex-direction:column; gap:12px; }
  .agent-task-inspector { width:auto; max-width:none; max-height:40%; padding:12px 0 0;
    border-left:0; border-top:var(--border-w) solid var(--border); }
}

.agent-task-head { display:flex; flex:none; flex-direction:column; gap:8px; }
.agent-task-title { margin:0; overflow-wrap:anywhere; color:var(--fg-strong); font-size:var(--text-h3);
  font-weight:var(--wght-strong); }
.agent-task-meta { display:flex; flex-wrap:wrap; align-items:center; gap:6px;
  font-size:var(--text-meta); }
/* Truncation only: the capsule is theme.css's .badge-quiet. This chip used to
   fill itself with --row-selected, the interaction wash, which made an inert
   metadata chip read as a selected row. */
.agent-task-chip { display:inline-flex; align-items:center; gap:4px; max-width:280px; overflow:hidden;
  color:var(--fg-muted); text-overflow:ellipsis; white-space:nowrap; }
.agent-task-sep { color:var(--fg-faint); }
.agent-task-status { display:inline-flex; align-items:center; padding:2px 8px;
  color:var(--fg-dim); font-variant-numeric:tabular-nums; }
.agent-task-status.is-working { color:var(--status-working); }
.agent-task-status.is-waiting { color:var(--status-waiting); font-weight:var(--wght-strong); }
.agent-task-status.is-failed { color:var(--danger-fg); }
.agent-task-stop { display:inline-flex; align-items:center; gap:4px; min-height:var(--control-h);
  padding:0 9px; color:var(--fg-muted); background:transparent;
  border:var(--border-w) solid var(--border-strong); border-radius:var(--radius-control);
  font:inherit; font-size:var(--text-meta); cursor:pointer; }
.agent-task-stop:hover { color:var(--danger-fg); background:var(--row-hover); }
.agent-task-stop:focus-visible, .agent-task-file:focus-visible {
  outline:var(--focus-ring-w) solid var(--focus); outline-offset:var(--focus-ring-offset); }

.agent-task-progress { display:flex; flex:none; flex-direction:column; gap:8px; }
/* The quiet section head, on the type and weight the transcript's own summary
   uses — so Progress, Files touched and Conversation read as three sections of
   one view rather than as loose lines with a control at the bottom. */
.agent-task-section { margin:0; color:var(--fg-muted); font-size:var(--text-meta);
  font-weight:var(--wght-strong); }
.agent-task-plan, .agent-task-steps { display:flex; flex-direction:column; margin:0; padding:0;
  list-style:none; }
.agent-task-plan-item { padding:1px 0; color:var(--fg-dim); font-size:var(--text-meta); }
.agent-task-plan-item.is-in_progress { color:var(--fg-strong); }
.agent-task-plan-item.is-completed { color:var(--fg-faint); text-decoration:line-through; }

.agent-task-step { display:grid; grid-template-columns:auto auto minmax(0,1fr); gap:8px;
  align-items:center; padding:1px 0; font-size:var(--text-meta); }
.agent-task-step-mark { color:var(--fg-faint); }
.agent-task-step.is-ok .agent-task-step-mark { color:var(--fg-muted); }
.agent-task-step.is-failed .agent-task-step-mark { color:var(--danger-fg); }
.agent-task-step.is-running .agent-task-step-mark { color:var(--status-working); }
.agent-task-step-tool { color:var(--fg-muted); font-weight:var(--wght-strong); }
.agent-task-step-label { min-width:0; overflow:hidden; color:var(--fg-dim);
  white-space:nowrap; text-overflow:ellipsis; }
.agent-task-step.is-failed .agent-task-step-label { color:var(--danger-fg); }
.agent-task-empty { margin:0; color:var(--fg-dim); font-size:var(--text-meta); }

.agent-task-files { display:flex; flex:none; flex-direction:column; gap:8px; }
.agent-task-files ul { display:flex; flex-direction:column; margin:0; padding:0; list-style:none; }
.agent-task-files li { display:flex; align-items:baseline; gap:6px; }
.agent-task-file { display:block; min-width:0; margin-right:auto; padding:1px 0; overflow:hidden;
  color:var(--accent-text); background:transparent; border:0; font-family:var(--font-mono);
  font-size:var(--text-meta); text-align:left; white-space:nowrap; text-overflow:ellipsis;
  cursor:pointer; }
.agent-task-file:hover { color:var(--accent-text-hover); text-decoration:underline; }
/* The transcript's own +N −M vocabulary (.conv-stat-add/.conv-stat-del), spelled
   again rather than borrowed: those rules ship with a MOUNTED Conversation, and
   this list is drawn with the conversation folded away. --diff-removed, not
   --danger-fg: a removed line is not a failure. */
.agent-task-file-add, .agent-task-file-del { flex:none; font-family:var(--font-mono);
  font-size:var(--text-meta); font-variant-numeric:tabular-nums; }
.agent-task-file-add { color:var(--diff-added); }
.agent-task-file-del { color:var(--diff-removed); }

`;
