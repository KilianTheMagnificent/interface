import type { AwayOptions, AwayResponse, AwayRun, AwaySnapshot, AwayState, AwayStatus } from '../shared/away';
import type { SessionKind } from '../shared/engine';
import type { AppSelectOption } from './components/AppSelect';
import {
  composerModelOptions, composerProviderForModelValue, decodeComposerModelChoice,
  encodeComposerModelChoice, type EngineChoices, type EngineOption,
} from './composer-model';
import { providerLabel } from './provider-labels';
import { equalValue } from './stable-values';

export interface AwayView { snapshot: AwaySnapshot | null; status: AwayStatus | null }
export const EMPTY_AWAY_VIEW: AwayView = { snapshot: null, status: null };

/** Display projection only: Rust revisions and receipts remain authoritative. */
export function mergeAwayView(current: AwayView, next: AwayResponse): AwayView {
  if (current.status && current.status.revision > next.revision) return current;
  if (current.status?.revision === next.revision && current.status.storageUncertain && !next.storageUncertain) {
    next = { ...next, storageUncertain: true };
  }
  let snapshot = current.snapshot;
  if (next.version === 1 && (!snapshot || snapshot.revision !== next.revision || snapshot.storageUncertain !== next.storageUncertain)) {
    snapshot = next;
  }
  const status: AwayStatus = next.version === 2 ? next : {
    version: 2, revision: next.revision, storageUncertain: next.storageUncertain,
    activeRuns: next.runs.filter(run => run.state === 'running' || run.state === 'stopping').map(({ id, cwd }) => ({ id, cwd })),
    runIds: next.runs.map(run => run.id), cancelledStarts: next.cancelledStarts,
  };
  const stableStatus = equalValue(current.status, status) ? current.status : status;
  return snapshot === current.snapshot && stableStatus === current.status ? current : { snapshot, status: stableStatus };
}
export function awayReceiptObserved(next: AwayResponse, runId: string): boolean {
  return (next.version === 2 ? next.runIds.includes(runId) : next.runs.some(run => run.id === runId))
    || next.cancelledStarts.includes(runId);
}

/** A compact read that moved past the cached reports leaves new turns' sessions
 *  unknown, so the next read fetches the snapshot once, not on every poll. */
export function awayNeedsSnapshot(view: AwayView, open: boolean): boolean {
  return open || view.snapshot === null
    || (view.status !== null && view.status.revision !== view.snapshot.revision);
}

/** Every session an Away run created. Those rows belong to the run's report,
 *  which opens each one, not to the session list of what the owner started. */
export function awaySessionKeys(snapshot: AwaySnapshot | null): string[] {
  return (snapshot?.runs ?? []).flatMap(run => run.turns
    .flatMap(turn => turn.sessionKey === null ? [] : [turn.sessionKey]));
}

/** Unknown initial state must not be mistaken for a confirmed idle project. */
export function awayRefreshDelay(status: AwayStatus | null, open: boolean, pending: boolean): number {
  return open || status === null || status.activeRuns.length > 0 || pending ? 2000 : 15_000;
}

export type AwayProvider = AwayOptions['provider'];

/**
 * What the coffee button does. A running loop is shown, never joined by a
 * second one: the button reads "View Away loops" then, so it opens them. A
 * project Away has never been started in (no saved settings) opens the form
 * so the folder and the agent are confirmed once before anything edits it;
 * after that the button starts at once with the saved settings. No project
 * ("") opens the form to choose one.
 */
export type AwayButtonAction =
  | { kind: 'view'; cwd: string; runId: string }
  | { kind: 'confirm'; cwd: string }
  | { kind: 'start'; cwd: string };
export function awayButtonAction(snapshot: AwaySnapshot | null, status: AwayStatus | null,
    project: string): AwayButtonAction {
  const running = status?.activeRuns ?? [];
  if (running.length > 0) {
    const shown = running.find(run => run.cwd === project) ?? running[0];
    return { kind: 'view', cwd: shown.cwd, runId: shown.id };
  }
  if (project === '' || !snapshot?.presets.some(preset => preset.cwd === project)) {
    return { kind: 'confirm', cwd: project };
  }
  return { kind: 'start', cwd: project };
}

/**
 * The settings a project's form opens on: its saved ones, else the host
 * defaults with the agent moved to one that can run now. The defaults name
 * Codex, and offering it while Codex is down meant a start that could only
 * fail. A saved choice is the owner's and is left alone.
 */
export function awayFormOptions(snapshot: AwaySnapshot | null, cwd: string,
    ready: (provider: AwayProvider) => boolean): AwayOptions | null {
  if (snapshot === null) return null;
  const saved = snapshot.presets.find(preset => preset.cwd === cwd)?.options;
  if (saved !== undefined) return saved;
  const defaults = snapshot.defaults;
  const other: AwayProvider = defaults.provider === 'codex' ? 'claude' : 'codex';
  const worker = ready(defaults.provider) || !ready(other) ? defaults
    : { ...defaults, provider: other, model: null };
  return withAwayReviewer(worker, ready);
}

/**
 * The reviewer a loop starts with (owner, 2026-10-14: "Other vendor by
 * default"): the other provider when it can run now, else none, which means
 * the worker's own provider reviews.
 */
export function awayDefaultReviewer(provider: AwayProvider,
    ready: (provider: AwayProvider) => boolean): AwayProvider | undefined {
  const other: AwayProvider = provider === 'codex' ? 'claude' : 'codex';
  return ready(other) ? other : undefined;
}

/** `options` with the default reviewer for its worker. */
export function withAwayReviewer(options: AwayOptions,
    ready: (provider: AwayProvider) => boolean): AwayOptions {
  const { reviewer: _previous, ...rest } = options;
  const reviewer = awayDefaultReviewer(options.provider, ready);
  return reviewer === undefined ? rest : { ...rest, reviewer };
}

/** The provider that reviews: the one named, else the worker's own. */
export function awayReviewer(options: Pick<AwayOptions, "provider" | "reviewer">): AwayProvider {
  return options.reviewer ?? options.provider;
}

/** `options` reviewed by `reviewer`; the worker's own provider is stored as no reviewer. */
export function awayWithReviewer(options: AwayOptions, reviewer: AwayProvider): AwayOptions {
  const { reviewer: _previous, ...rest } = options;
  return reviewer === options.provider ? rest : { ...rest, reviewer };
}

/**
 * The project the toolbar's Away works on: the one in front of the owner.
 * Threads and Agent pass their focused session's project (a thread's own
 * record first, since a worktree thread runs inside the worktree); Code reads
 * its active terminal. Code's answer is only asked for on Code: Threads used
 * to get the Code tab's terminal folder, often home. Nothing focused is "",
 * and the form then asks for a folder instead of guessing one.
 */
export function awayProject(surface: SessionKind, focusedProject: string | null,
    codeProject: () => string): string {
  return surface === 'code' ? codeProject() : focusedProject ?? '';
}

/* ---- Away loops as Threads (review B3, protocol 45) ------------------------ */

/**
 * The sessions Threads and the session lists leave out: each loop's reviews,
 * and every session of a loop from before protocol 45. A loop's work session
 * is its thread, so it stays listed.
 */
export function awayHiddenSessionKeys(snapshot: AwaySnapshot | null): string[] {
  return (snapshot?.runs ?? []).flatMap(run => run.turns
    .flatMap(turn => turn.sessionKey === null || turn.sessionKey === run.thread?.key
      ? [] : [turn.sessionKey]));
}

/** The thread a start opened, once Rust has filed it. */
export function awayStartedThread(snapshot: AwaySnapshot | null, runId: string): string | null {
  return snapshot?.runs.find(run => run.id === runId)?.thread?.key ?? null;
}

/** One loop in the Away panel's list. */
export interface AwayLoopRow {
  id: string;
  state: AwayState;
  active: boolean;
  cwd: string;
  /** The project folder's name. */
  project: string;
  /** The objective's first line. */
  objective: string;
  detail: string;
  /** "Round 2 of 12": the newest round the loop reached. */
  round: string;
  /** The loop's thread; null before it exists and for loops from before protocol 45. */
  threadKey: string | null;
  /** A loop with no thread opens its newest session instead. */
  sessionKey: string | null;
  startedAtMs: number;
  deadlineAtMs: number;
}

/** What a loop's state reads as in the panel and on the thread's cards. */
export const AWAY_STATE_LABELS: Readonly<Record<AwayState, string>> = {
  running: "Working", stopping: "Stopping", completed: "Finished", stopped: "Stopped",
  expired: "Time limit reached", blocked: "Stopped for you", unknown: "Interrupted",
};

const firstLine = (text: string): string =>
  text.split("\n").map(line => line.trim()).find(line => line !== "") ?? "";

/** The panel's list: running loops first, then the newest finished ones, `limit` in all. */
export function awayLoopRows(snapshot: AwaySnapshot | null, limit = 8): AwayLoopRow[] {
  const runs = [...(snapshot?.runs ?? [])].reverse();
  const live = (run: AwayRun): boolean => run.state === "running" || run.state === "stopping";
  return [...runs.filter(live), ...runs.filter(run => !live(run))].slice(0, limit).map(run => {
    const reached = run.turns.reduce((most, turn) => Math.max(most, turn.iteration), 0);
    const last = [...run.turns].reverse().find(turn => turn.sessionKey !== null);
    return {
      id: run.id, state: run.state, active: live(run), cwd: run.cwd,
      project: run.cwd.split("/").filter(Boolean).at(-1) ?? run.cwd,
      objective: firstLine(run.options.objective), detail: run.detail,
      round: `Round ${Math.max(reached, 1)} of ${run.options.maxIterations}`,
      threadKey: run.thread?.key ?? null, sessionKey: last?.sessionKey ?? null,
      startedAtMs: run.startedAtMs, deadlineAtMs: run.deadlineAtMs,
    };
  });
}

/** The providers Away runs. */
export const AWAY_PROVIDERS: readonly AwayProvider[] = ["codex", "claude"];
const DEFAULT_MODEL_PREFIX = "away-default-model:";

/**
 * The model picker's rows for Away: the shared catalog (ready providers
 * first) restricted to Away's providers, each provider's group led by its
 * own default model, which is what a saved `model: null` means.
 */
export function awayModelRows(engines: readonly EngineOption[],
    choicesFor: (engine: string) => EngineChoices, options: AwayOptions): AppSelectOption[] {
  const away = engines.filter(engine => (AWAY_PROVIDERS as readonly string[]).includes(engine.id));
  const rows = composerModelOptions(away.length > 0 ? away : AWAY_PROVIDERS.map(id => ({ id })),
    choicesFor, options.provider, options.model ?? "");
  const out: AppSelectOption[] = [];
  let group: string | undefined;
  for (const row of rows) {
    if (row.group !== group) {
      group = row.group;
      const engine = composerProviderForModelValue(row.value);
      if (engine !== null) {
        const label = providerLabel(engine);
        out.push({ value: `${DEFAULT_MODEL_PREFIX}${engine}`, label: "Default model", group,
          triggerLabel: `${label} · Default model`,
          description: `${label}'s own default model.` });
      }
    }
    out.push(row);
  }
  return out;
}

/** The picker row the form's provider and model are. */
export function awayModelValue(options: Pick<AwayOptions, "provider" | "model">): string {
  return options.model === null || options.model.trim() === ""
    ? `${DEFAULT_MODEL_PREFIX}${options.provider}`
    : encodeComposerModelChoice(options.provider, options.model);
}

/** The provider and model a picker row names; null for a row Away cannot run. */
export function awayModelFromValue(value: string): Pick<AwayOptions, "provider" | "model"> | null {
  const provider = value.startsWith(DEFAULT_MODEL_PREFIX) ? value.slice(DEFAULT_MODEL_PREFIX.length)
    : decodeComposerModelChoice(value)?.engine;
  if (provider !== "codex" && provider !== "claude") return null;
  const model = value.startsWith(DEFAULT_MODEL_PREFIX) ? null : decodeComposerModelChoice(value)?.model ?? null;
  return { provider, model };
}
