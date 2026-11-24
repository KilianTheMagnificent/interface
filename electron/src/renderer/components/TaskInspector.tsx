import { useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import type { HarnessView, TerminalServerLink } from '../../shared/harness';
import { useModalFocus } from './ModalBehavior';
import { branchSessionLabel, inventoryGroups } from '../task-inspector-view';

/**
 * Task details: one dialog for a checkout's captured results and checks, its
 * context inventory and its workspace presets (the palette's one "Task
 * details…" row, the ··· menu's Context inventory, a terminal's inspect).
 */
export function TaskInspector({ view, busy, error, onClose, onCapture, onInspect, onContext, onAttach, onPreset, onRunChecks, servers, onFindServers, onOpenServer, onOpenSession, onShowContext, onShowPresets, sessionTitle }: {
  view: HarnessView | null; busy: boolean; error: string | null; onClose: () => void;
  onCapture: (objective: string) => void; onInspect: (id: string) => void;
  onContext: (query: string) => void; onAttach: (text: string) => void;
  onOpenSession: (key: string) => void;
  servers: TerminalServerLink[]; onFindServers: () => void; onOpenServer: (server: TerminalServerLink) => void;
  onPreset: (isolate: boolean) => void; onRunChecks: (id: string, selected: string[]) => void;
  /** Load the context inventory or the presets into this dialog. */
  onShowContext?: () => void; onShowPresets?: () => void;
  /** A session's title, the way the sidebar names it; undefined when it is not listed. */
  sessionTitle?: (key: string) => string | undefined;
}) {
  const [objective, setObjective] = useState('Inspect current task result');
  const [query, setQuery] = useState('');
  const ref = useRef<HTMLElement>(null);
  const onKeyDown = useModalFocus({ open: true, containerRef: ref, onEscape: onClose });
  const surface = <div className="task-inspector-backdrop" onMouseDown={event => { if (event.target === event.currentTarget) onClose(); }}>
    <style>{`
      .task-inspector-backdrop { position: fixed; inset: 0; z-index: 1200; display: flex; align-items: center; justify-content: center; padding: 16px; background: color-mix(in srgb, var(--chrome-bg) 68%, transparent); }
      .task-inspector { box-sizing: border-box; font-family: var(--font-ui); color: var(--fg); background: var(--widget-bg); border: var(--border-w) solid var(--border-strong); border-radius: var(--radius-overlay); box-shadow: var(--overlay-shadow); overflow-wrap: anywhere; }
      .task-inspector-head { display: flex; align-items: center; justify-content: space-between; gap: 12px; margin: 0 0 16px; }
      .task-inspector h2 { margin: 0; font-size: var(--text-display); }
      .task-inspector h3 { font-size: var(--text-nav); margin: 20px 0 10px; }
      .task-inspector h4.task-inspector-group { font-size: var(--text-body); margin: 12px 0 4px; }
      .task-inspector .task-inspector-scope { color: var(--fg-dim); }
      .task-inspector p, .task-inspector li { font-size: var(--text-body); line-height: var(--lh-prose); }
      .task-inspector ul { margin: 8px 0 12px; padding-left: 18px; }
      .task-inspector form { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; margin: 8px 0 12px; }
      .task-inspector form input { flex: 1 1 220px; }
      .task-inspector button { margin: 4px 6px 4px 0; padding: 6px 10px; border: 1px solid var(--border); border-radius: var(--radius-control); background: var(--row-hover); color: var(--fg); font: inherit; cursor: pointer; }
      .task-inspector button:hover:not(:disabled) { background: var(--row-selected); color: var(--fg-strong); }
      .task-inspector button:disabled { color: var(--fg-dim); cursor: default; }
      .task-inspector input { box-sizing: border-box; max-width: 100%; padding: 8px; background: var(--workbench-canvas); color: var(--fg); border: 1px solid var(--border); border-radius: var(--radius-control); }
      .task-inspector details { padding: 9px 0; border-top: 1px solid var(--border); }
      .task-inspector summary { cursor: pointer; font-size: var(--text-body); line-height: var(--lh-prose); }
      .task-inspector code, .task-inspector pre { font-size: var(--text-ui); white-space: pre-wrap; overflow-wrap: anywhere; }
      .task-inspector [role=alert] { color: var(--status-failed); }
      .task-inspector button:focus-visible, .task-inspector input:focus-visible, .task-inspector summary:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
    `}</style>
    <section ref={ref} onKeyDown={onKeyDown} role="dialog" aria-modal="true" aria-label="Task details" className="modal task-inspector is-floating" style={{ width: 720, maxWidth: 'calc(100vw - 32px)', maxHeight: '85vh', overflow: 'auto', padding: 20 }}>
      <div className="task-inspector-head">
        <h2>Task details</h2>
        <button type="button" onClick={onClose} aria-label="Close task details">Close</button>
      </div>
      {(onShowContext || onShowPresets) && (!view?.context || !view.presets) && <p className="task-inspector-sections">
        {onShowContext && !view?.context && <button type="button" disabled={busy} onClick={onShowContext}>Context inventory</button>}
        {onShowPresets && !view?.presets && <button type="button" disabled={busy} onClick={onShowPresets}>Workspace presets</button>}
      </p>}
      {busy && <p role="status">Reading confirmed host state…</p>}
      {error && <p role="alert">{error}</p>}
      {view?.result ? <>
        <h3>{view.result.objective}</h3>
        <p>{view.stale ? 'Files changed since this result was captured. Its evidence is stale.' : view.result.state}</p>
        <p>Checkout: <code>{view.result.root}</code><br />Baseline: <code>{view.result.baseline}</code><br />Candidate: <code>{view.result.candidate}</code></p>
        {view.result.checks.length === 0 && <p>No checks captured for this snapshot.</p>}
        <ul>{view.result.checks.map((check, index) => <li key={index}><details><summary>{check.name}: {check.status} · {check.durationMs} ms</summary><pre>{check.stdout}{check.stderr}</pre><button onClick={() => onAttach(`${check.name}: ${check.status}\n${check.stdout}\n${check.stderr}`)}>Attach check output to follow-up</button></details></li>)}</ul>
        {(view.checks?.checks ?? []).map(check => <button key={check} disabled={busy || view.stale} onClick={() => onRunChecks(view.result!.id, [check])}>Run {check}</button>)}
        <ul>{view.result.findings.map((finding, index) => <li key={index}>{finding}</li>)}</ul>
        <button type="button" onClick={() => onAttach([view.result!.objective, ...view.result!.checks.map(check => `${check.name}: ${check.status}`), ...view.result!.findings].join('\n'))}>Attach result summary to follow-up</button>
        {view.review?.files.map(file => <details key={file.path}><summary>{file.path}{file.omitted ? ` · ${file.omitted}` : ''}</summary><pre style={{ whiteSpace: 'pre-wrap' }}>{file.patch}</pre>{file.patchTruncated && <p>Patch truncated.</p>}</details>)}
      </> : null}
      {view?.context && <>
        <h3>Context inventory</h3><p>{view.context.root}</p>
        <form onSubmit={event => { event.preventDefault(); onContext(query); }}><input aria-label="Find relevant code" value={query} onChange={event => setQuery(event.target.value)} /><button disabled={busy}>Find routes</button></form>
        <p>{view.context.usage}</p>
        <ul>{view.context.suggestions.map(path => <li key={path}><code>{path}</code></li>)}</ul>
        <details><summary>Discovered instruction and customization sources</summary>
          {inventoryGroups(view.context.inventory).length === 0 && <p>No rules, skills, commands, hooks or MCP servers found.</p>}
          {inventoryGroups(view.context.inventory).map(group => <div key={group.kind}>
            <h4 className="task-inspector-group">{group.label} · {group.rows.length}</h4>
            <ul>{group.rows.map(row => <li key={row.key} title={row.sourcePath}>
              <strong>{row.name}</strong>{row.detail && <> — {row.detail}</>} <span className="task-inspector-scope">({row.scope})</span>
              {row.error && <><br /><span role="note">{row.error}</span></>}
            </li>)}</ul>
          </div>)}
          {view.context.inventory.truncated && <p>Long lists stop at 200 entries per kind; some are not shown.</p>}
        </details>
        <ul>{view.context.omissions.map(item => <li key={item}>{item}</li>)}</ul>
      </>}
      {view?.presets && <><h3>Workspace presets</h3>{view.presets.map(preset => <p key={preset.id}><button disabled={busy} onClick={() => onPreset(preset.isolate)}>{preset.label}</button><br />{preset.setup}<br /><code>{preset.root}</code></p>)}</>}
      <details><summary>Conversation branches</summary><p>Provider-current copies the latest stored conversation. It shares the checkout; it does not restore or isolate files.</p>
        {view?.state.branches.map(branch => <p key={branch.child}><button title={branch.parent} onClick={() => onOpenSession(branch.parent)}>Source: {branchSessionLabel(sessionTitle?.(branch.parent))}</button> → <button title={branch.child} onClick={() => onOpenSession(branch.child)}>Branch: {branchSessionLabel(sessionTitle?.(branch.child))}</button><br />{branch.root}</p>)}
      </details>
      <details><summary>Running app links</summary><p>Only listeners owned by the selected terminal process tree are listed. HTTP and app health are not verified.</p>
        <button disabled={busy} onClick={onFindServers}>Find servers in this terminal</button>
        {servers.map(server => <p key={`${server.processId}:${server.url}`}><button onClick={() => onOpenServer(server)}>{server.url}</button> · PID {server.processId}<br />{server.readiness}</p>)}
      </details>
      <details open={!view?.result && !view?.context && !view?.presets}><summary>Capture a result</summary>
        <p>Capture the current checkout and review its changes. Capturing does not run checks.</p>
        <input aria-label="Task objective" value={objective} onChange={event => setObjective(event.target.value)} maxLength={8000} />
        <button disabled={busy || !objective.trim()} onClick={() => onCapture(objective)}>Capture checkout result</button>
        {view?.state.tasks.slice().reverse().map(task => <p key={task.id}><button disabled={busy} onClick={() => onInspect(task.id)}>{task.objective}</button> · {task.root}</p>)}
      </details>
    </section>
  </div>;
  return typeof document === 'undefined' ? surface : createPortal(surface, document.body);
}
