import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type { AwayCommand, AwayOptions, AwayRequest, AwayResponse } from "../../shared/away";
import { useModalFocus } from "./ModalBehavior";
import {
  AWAY_STATE_LABELS, EMPTY_AWAY_VIEW, awayButtonAction, awayFormOptions, awayHiddenSessionKeys,
  awayLoopRows, awayModelFromValue, awayModelRows, awayModelValue, awayNeedsSnapshot,
  awayReceiptObserved, awayRefreshDelay, awayReviewer, awaySessionKeys, awayStartedThread,
  awayWithReviewer, mergeAwayView, withAwayReviewer, AWAY_PROVIDERS,
  type AwayLoopRow, type AwayProvider,
} from "../away-view";
import type { EngineChoices, EngineOption } from "../composer-model";
import { projectHueClass } from "../project-hue";
import { providerLabel } from "../provider-labels";
import { startVisiblePolling, type VisiblePolling } from "../visible-polling";
import { withoutIpcErrorShape } from "../ipc-message";
import { AppSelect, type AppSelectOption } from "./AppSelect";
import { ProjectTile } from "./ProjectHueParts";
import { ProviderLogo } from "./ProviderLogo";
import { Icon } from "./glyphs";
import "./AwayControl.css";

const MARKER = "interface.away.pending.v1";
type Start = Extract<AwayCommand, { kind: "start" }>;
const ANY_PROVIDER = (): boolean => true;
const NO_ENGINES: readonly EngineOption[] = [];
const NO_THREADS: ReadonlyMap<string, string> = new Map();
const NO_CHOICES = (): EngineChoices => ({ models: [], efforts: [], permissionModes: [] });
const folderName = (path: string): string => path.split("/").filter(Boolean).at(-1) ?? path;
const clock = (at: number): string =>
  new Date(at).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });

/**
 * AWAY (review B3, protocol 45). The coffee button and its panel. Each loop
 * runs as a Threads thread in its own worktree; the panel is the short list
 * of running and finished loops, each opening its thread, and the start
 * form, drawn like the New thread card: the project the loop works on, the
 * objective in a card whose foot carries the shared model picker (Away's
 * providers only), the time and round limits and Start, and a line under the
 * card saying where the work happens. Rust owns every run, receipt and
 * preset; this renders them and sends the owner's commands.
 */
export function AwayControl({ currentProject, providerReady = ANY_PROVIDER, request, pickDirectory,
  onInspectSession, onOpenThread, onSessionKeys, onRunningChange, onOpenChange, openRequest = 0,
  inert = false, engines = NO_ENGINES, choicesFor = NO_CHOICES, threadKeys = NO_THREADS }: {
  /** The project in front of the owner, or "" when nothing is focused. */
  currentProject: () => string;
  /** Whether a provider can run now; the form defaults to one that can. */
  providerReady?: (provider: AwayProvider) => boolean;
  request: (request: AwayRequest) => Promise<AwayResponse>;
  pickDirectory: (preferred?: string) => Promise<string | null>;
  /** Opens a loop from before protocol 45, which has sessions but no thread. */
  onInspectSession: (key: string) => void;
  /** Opens a loop's thread on the Threads surface. */
  onOpenThread?: (key: string) => void;
  /**
   * The sessions Away's runs created (`all`, which the quit guard counts as
   * the loops) and the ones the session lists leave out (`hidden`: reviews,
   * and every session of a loop with no thread), whenever the snapshot changes.
   */
  onSessionKeys?: (keys: { all: string[]; hidden: string[] }) => void;
  /** How many loops are running, reported whenever that changes (the quit guard counts them). */
  onRunningChange?: (count: number) => void;
  onOpenChange?: (open: boolean) => void;
  /** Each new value opens the panel (the palette's "Away loop…"); it never starts a loop. */
  openRequest?: number;
  inert?: boolean;
  /** The shared model catalog; the picker offers Away's providers only. */
  engines?: readonly EngineOption[];
  choicesFor?: (engine: string) => EngineChoices;
  /** Each loop's thread by run, from the Threads view Rust pushes. */
  threadKeys?: ReadonlyMap<string, string>;
}) {
  const [view, setView] = useState(EMPTY_AWAY_VIEW);
  const { snapshot, status } = view;
  const viewRef = useRef(view); viewRef.current = view;
  const [open, setOpen] = useState(false);
  useEffect(() => () => { onOpenChange?.(false); }, [onOpenChange]);
  const [cwd, setCwd] = useState("");
  const [options, setOptions] = useState<AwayOptions | null>(null);
  const [error, setError] = useState("");
  const [pending, setPending] = useState<Start | null>(null);
  // The start whose thread opens once Rust has filed it.
  const [opening, setOpening] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const root = useRef<HTMLElement>(null);
  const startButton = useRef<HTMLButtonElement>(null);
  const api = useRef(request); api.current = request;
  const ready = useRef(providerReady); ready.current = providerReady;
  const openRef = useRef(open); openRef.current = open;
  const polling = useRef<VisiblePolling | null>(null);
  const pendingRef = useRef(pending); pendingRef.current = pending;
  const openingRef = useRef(opening); openingRef.current = opening;
  const openThread = useRef(onOpenThread); openThread.current = onOpenThread;
  const alive = useRef(true);
  const changeOpen = (next: boolean): void => { openRef.current = next; setOpen(next); onOpenChange?.(next); polling.current?.refresh(); };
  const close = (): void => { changeOpen(false); };
  const onKeyDown = useModalFocus({ open, containerRef: root, initialFocus: () => root.current, onEscape: close, returnFocusRef: startButton });
  const apply = (next: AwayResponse): void => {
    if (!alive.current) return;
    if (viewRef.current.status && viewRef.current.status.revision > next.revision) return;
    const merged = mergeAwayView(viewRef.current, next);
    viewRef.current = merged; setView(merged);
    const receipt = pendingRef.current;
    if (receipt && awayReceiptObserved(next, receipt.runId)) {
      // The host has the run, so the receipt is resolved either way. A marker
      // that blocked storage keeps names that same run; a later load observes
      // it again and clears it. Nothing re-sends it on its own.
      try { sessionStorage.removeItem(MARKER); } catch { /* see above */ }
      pendingRef.current = null; setPending(null);
    }
  };
  useEffect(() => {
    alive.current = true;
    try {
      const raw = sessionStorage.getItem(MARKER);
      if (raw) { const saved = JSON.parse(raw) as Start; pendingRef.current = saved; setPending(saved); setCwd(saved.cwd); }
    } catch { setError("Away cannot retain its delivery receipt in this window. Reopen the window before starting."); }
    let cancelled = false;
    const read = async (): Promise<void> => {
      try {
        const full = awayNeedsSnapshot(viewRef.current, openRef.current);
        const next = await api.current(full ? { version: 1, command: { kind: "inspect" } }
          : { version: 2, command: { kind: "status" } });
        if (!cancelled) apply(next);
      } catch (failure) { if (!cancelled) setError(failure instanceof Error ? withoutIpcErrorShape(failure.message) : String(failure)); }
    };
    // A snapshot behind the status (a loop just ended, say) is fetched soon:
    // the sessions it names leave the lists only once it is read.
    const stop = startVisiblePolling(read, () => awayRefreshDelay(viewRef.current.status,
      openRef.current, pendingRef.current !== null || openingRef.current !== null
        || awayNeedsSnapshot(viewRef.current, false)));
    polling.current = stop;
    return () => { cancelled = true; alive.current = false; polling.current = null; stop(); };
  }, []);
  useEffect(() => {
    onSessionKeys?.({ all: awaySessionKeys(snapshot), hidden: awayHiddenSessionKeys(snapshot) });
  }, [snapshot, onSessionKeys]);
  // A start opens its thread as soon as Rust has filed it; one that ended
  // before it had a thread (no git repository, say) leaves the panel saying why.
  useEffect(() => {
    if (opening === null) return;
    // The Threads push names the thread first; an open panel's snapshot too.
    const key = threadKeys.get(opening) ?? awayStartedThread(snapshot, opening);
    const run = snapshot?.runs.find(item => item.id === opening);
    if (key !== null) {
      openingRef.current = null; setOpening(null);
      if (openRef.current) changeOpen(false);
      openThread.current?.(key);
    } else if (run !== undefined && run.state !== "running") {
      openingRef.current = null; setOpening(null);
    }
  });
  const send = async (command: AwayCommand): Promise<void> => {
    const urgent = command.kind === "stop";
    if (!urgent && busyRef.current) return;
    if (!urgent) { busyRef.current = true; setBusy(true); }
    setError("");
    try {
      if (command.kind === "start") {
        sessionStorage.setItem(MARKER, JSON.stringify(command));
        pendingRef.current = command; setPending(command);
        openingRef.current = command.runId; setOpening(command.runId);
      }
      apply(await api.current({ version: 1, command }));
    } catch (failure) {
      setError(`${failure instanceof Error ? withoutIpcErrorShape(failure.message) : String(failure)}${command.kind === "start" ? " The request is retained. Retry uses the same identity and cannot start a duplicate loop." : ""}`);
    } finally { if (!urgent) { busyRef.current = false; setBusy(false); } }
  };
  const selectProject = (path: string): void => {
    setCwd(path);
    setOptions(awayFormOptions(viewRef.current.snapshot, path, ready.current));
  };
  const start = (path: string, settings: AwayOptions | null): void => {
    if (!path || !viewRef.current.snapshot || pendingRef.current || busyRef.current) return;
    if (viewRef.current.status?.activeRuns.some(run => run.cwd === path)) return;
    void send({ kind: "start", runId: crypto.randomUUID(), cwd: path, options: settings });
  };
  const quickStart = (): void => {
    const action = awayButtonAction(viewRef.current.snapshot, viewRef.current.status, currentProject());
    selectProject(action.cwd); changeOpen(true);
    if (action.kind === "start") start(action.cwd, null);
  };
  const configure = (): void => { selectProject(currentProject()); changeOpen(true); };
  // Opens on what the button would show — the loops and the project's form —
  // but never starts one: the palette asked to see Away, not to run it.
  const openPanel = (): void => {
    const action = awayButtonAction(viewRef.current.snapshot, viewRef.current.status, currentProject());
    selectProject(action.cwd); changeOpen(true);
  };
  const lastOpenRequest = useRef(openRequest);
  useEffect(() => {
    if (openRequest === lastOpenRequest.current) return;
    lastOpenRequest.current = openRequest;
    openPanel();
  });
  const running = status?.activeRuns ?? [];
  useEffect(() => { onRunningChange?.(running.length); }, [running.length, onRunningChange]);
  useEffect(() => {
    if (open && options === null && snapshot) setOptions(awayFormOptions(snapshot, cwd, ready.current));
  }, [open, options, snapshot, cwd]);
  useLayoutEffect(() => {
    if (open && root.current && !root.current.contains(document.activeElement)) root.current.focus();
  }, [open]);
  const patch = (change: Partial<AwayOptions>): void => { if (options) setOptions({ ...options, ...change }); };
  const loops = awayLoopRows(snapshot);
  const openLoop = (loop: AwayLoopRow): void => {
    if (loop.threadKey !== null) { close(); onOpenThread?.(loop.threadKey); }
    else if (loop.sessionKey !== null) { close(); onInspectSession(loop.sessionKey); }
  };
  const runningHere = cwd !== "" && running.some(run => run.cwd === cwd);
  const modelRows = options === null ? [] : awayModelRows(engines, choicesFor, options);
  // The reviewer: Codex or Claude, each saying when it cannot run now.
  const reviewerRows: AppSelectOption[] = AWAY_PROVIDERS.map(provider => ({
    value: provider, label: providerLabel(provider),
    triggerLabel: `Reviewed by ${providerLabel(provider)}`,
    description: options !== null && provider === options.provider
      ? "The worker's own provider, in a fresh session."
      : providerReady(provider) ? "The other vendor, in a fresh session." : "Unavailable now.",
  }));
  const groupLogo = (option: AppSelectOption) => {
    const provider = awayModelFromValue(option.value)?.provider;
    return provider === undefined ? null : <ProviderLogo engine={provider} label={providerLabel(provider)} />;
  };
  const chooseFolder = (): void => {
    void pickDirectory(cwd || undefined).then(path => { if (path) selectProject(path); })
      .catch(failure => { setError(withoutIpcErrorShape(String(failure))); });
  };
  const locked = busy || pending !== null;
  const panel = open ? <div className="away-backdrop">
    <section ref={root} className="away-panel is-floating" data-voice-guard role="dialog" aria-modal="true" aria-label="Away loop" tabIndex={-1} onKeyDown={onKeyDown}>
      <header className="away-head">
        <div><h2>Away</h2><p>Unattended work in its own worktree, reviewed by a fresh session every round.</p></div>
        <button type="button" className="away-close" aria-label="Close Away loop" onClick={close}><Icon name="close" /></button>
      </header>
      <div className="away-body">
        {loops.length > 0 && <section className="away-loops" aria-label="Away loops">
          <h3 className="away-label">Loops</h3>
          <ul>{loops.map(loop => <li key={loop.id} className={loop.active ? "away-loop is-active" : "away-loop"}>
            <button type="button" className="away-loop-open" disabled={loop.threadKey === null && loop.sessionKey === null}
              title={loop.threadKey !== null ? "Open its thread" : "Open its last session"}
              onClick={() => { openLoop(loop); }}>
              <ProjectTile root={loop.cwd} name={loop.project} />
              <span className="away-loop-text">
                <span className="away-loop-title">{loop.objective}</span>
                <span className="away-loop-meta">{[loop.project, AWAY_STATE_LABELS[loop.state],
                  loop.active ? `${loop.round} · ends ${clock(loop.deadlineAtMs)}` : loop.detail].join(" · ")}</span>
              </span>
              {loop.threadKey !== null && <Icon name="chevron-right" sm className="away-loop-go" />}
            </button>
            {loop.active && <button type="button" className="away-stop" disabled={loop.state === "stopping"}
              onClick={() => { void send({ kind: "stop", runId: loop.id }); }}>
              {loop.state === "stopping" ? "Stopping…" : "Stop loop"}</button>}
          </li>)}</ul>
        </section>}
        {options && <form className={`away-start ${projectHueClass(cwd || null)}`}
          onSubmit={event => { event.preventDefault(); start(cwd, options); }}>
          <h3 className="away-label">New loop</h3>
          <div className="away-project-row">
            <button type="button" className="away-project" disabled={locked}
              title={cwd || "Choose a project folder"} onClick={chooseFolder}>
              <ProjectTile root={cwd || null} name={cwd ? folderName(cwd) : null} />
              <span className="away-project-name">{cwd ? folderName(cwd) : "Choose a project"}</span>
            </button>
            {cwd && <span className="away-path" title={cwd}>{cwd}</span>}
            <button type="button" className="away-link" disabled={locked} onClick={chooseFolder}>Choose folder…</button>
          </div>
          <div className="away-card">
            <label className="away-sr" htmlFor="away-objective">What should it work on?</label>
            <textarea id="away-objective" rows={3} maxLength={8192} value={options.objective}
              placeholder="What should Away work on?"
              onChange={event => { patch({ objective: event.target.value }); }} />
            <div className="away-card-foot">
              <AppSelect label="Model" value={awayModelValue(options)} options={modelRows}
                placeholder="Choose model" compact
                leading={<ProviderLogo engine={options.provider} label={providerLabel(options.provider)} />}
                renderGroupLeading={groupLogo}
                onChange={value => {
                  const choice = awayModelFromValue(value);
                  // A new worker takes its default reviewer: the other vendor when it can run.
                  if (choice === null) return;
                  setOptions(choice.provider === options.provider ? { ...options, ...choice }
                    : withAwayReviewer({ ...options, ...choice }, ready.current));
                }}
                onCustom={(model, option) => {
                  const provider = (option === undefined ? undefined
                    : awayModelFromValue(option.value)?.provider) ?? options.provider;
                  if (model.trim() !== "") patch({ provider, model: model.trim() });
                }}
                popoverClassName="composer-model-popover away-model-popover" className="away-chip away-model" />
              <AppSelect label="Reviewer" value={awayReviewer(options)} options={reviewerRows}
                compact leading={<ProviderLogo engine={awayReviewer(options)} label={providerLabel(awayReviewer(options))} />}
                onChange={value => {
                  const reviewer = AWAY_PROVIDERS.find(provider => provider === value);
                  if (reviewer !== undefined) setOptions(awayWithReviewer(options, reviewer));
                }}
                popoverClassName="away-model-popover" className="away-chip away-reviewer" />
              <span className="away-sep" aria-hidden="true" />
              <label className="away-chip away-number" title="How long the loop may run">
                <input type="number" min={1} max={1440} aria-label="Minutes" value={options.durationSeconds / 60}
                  onChange={event => { patch({ durationSeconds: Number(event.target.value) * 60 }); }} />
                <span>min</span>
              </label>
              <label className="away-chip away-number" title="How many work and review rounds">
                <input type="number" min={1} max={20} aria-label="Rounds" value={options.maxIterations}
                  onChange={event => { patch({ maxIterations: Number(event.target.value) }); }} />
                <span>rounds</span>
              </label>
              <button type="submit" className="away-submit" aria-label="Start loop"
                title={runningHere ? "A loop already runs in this project" : "Start loop"}
                disabled={!cwd || locked || runningHere || status?.storageUncertain === true || !options.objective.trim()}>
                <Icon name="send" />
              </button>
            </div>
          </div>
          <p className="away-drawer">
            {providerReady(options.provider) ? null : <strong>{providerLabel(options.provider)} is unavailable. </strong>}
            {providerReady(awayReviewer(options)) ? null : <strong>{providerLabel(awayReviewer(options))} is unavailable to review. </strong>}
            {`Works in a new worktree of ${cwd ? folderName(cwd) : "the project"}, so your checkout stays as it is; a fresh ${providerLabel(awayReviewer(options))} session reviews every round. Commits, publishing and installs stay yours.`}
          </p>
          <div className="away-actions"><button type="button" className="away-link" disabled={!cwd || locked}
            onClick={() => { void send({ kind: "save", cwd, options }); }}>Save settings</button></div>
        </form>}
        {pending && <div role="status" className="away-pending"><p>Reconciling start for {pending.cwd}…</p><button type="button" disabled={busy} onClick={() => { void send(pending); }}>Retry same start</button><button type="button" onClick={() => { void send({ kind: "stop", runId: pending.runId }); }}>Stop pending loop</button></div>}
        {status?.storageUncertain && <p role="alert">Away storage is uncertain. Use Stop for any active loop, then restart Interface and inspect the retained sessions.</p>}
        {error && <p role="alert">{error}</p>}
        <p className="away-hint">You can close this while work continues. Interface keeps the Mac from idle sleep while a loop runs; closing the lid still sleeps it. Quitting asks first, then stops its loops. A stopped or interrupted loop never restarts on its own.</p>
      </div>
    </section>
  </div> : null;
  return <><div className="away-toolbar" data-voice-guard>
    <button ref={startButton} type="button" className="workspace-icon-btn" aria-label={running.length ? `Away loops (${running.length} running)` : "Start Away loop"} title={running.length ? "View Away loops" : "Start Away now for the current project"} disabled={inert || snapshot === null || status?.storageUncertain || busy} onClick={quickStart}><span className="codicon codicon-coffee" aria-hidden="true" />{running.length > 0 && <span className="away-count">{running.length}</span>}</button>
    <button type="button" className="workspace-icon-btn away-options" aria-label="Away loop options" title="Choose a project and configure Away" disabled={inert} onClick={configure}><span className="codicon codicon-sm codicon-chevron-down" aria-hidden="true" /></button>
  </div>{panel && createPortal(panel, document.body)}</>;
}
