import { sessionKey, type EngineEvent, type SessionKey, type SessionStatus, type SessionSummary,
  type ModelOption } from "../shared/engine";
import {
  configInForce, fastModeInForce, permissionPicker, type ConfigInForce, type FastModeInForce,
} from "./session-config-view";
import { turnProgress, type TurnProgress } from "./session-activity";
import { eventEndsTurn } from "./turn-events";
import { approvalVerdictKey, type ApprovalVerdict } from "./approval-keys";
import { equalValue } from "./stable-values";
import { latestModelPresetFor, modelPresetFor, type ModelPreset } from "./model-presets";
import { sessionHasPermissionControls, sessionPermissionMode } from "./session-presentation";
import { concreteEffortLevels, preferredEffort } from "./composer-model";
import type { SessionConfigView } from "./conversation-types";
import type { AgentCanvasSession } from "./components/AgentCanvas";
export type PendingConfig = { model?: string; effort?: string | null };
export type PendingMap = Record<SessionKey, PendingConfig>;

const EFFORT_LEVELS = ["low", "medium", "high", "xhigh", "max"];

/**
 * The fallback ladder, corrected per engine. This mirrors `withUltracode` in
 * engines/claude/adapter.ts: an xhigh-capable ladder gets "ultracode" appended,
 * and the list above contains xhigh, so Claude's fallback must carry the rung
 * too. It exists because the ladder is normally read off the session model's
 * catalogue row — and a pane restored at boot has no `models.available` event
 * yet, so when the probe catalogue is also absent the bare list above would
 * hide the ladder's top rung until the session went live. Codex is untouched
 * deliberately: its "ultra" is advertised per-model by app-server and is never
 * guessed here.
 */
export function fallbackEffortLevels(engine: string): string[] {
  return engine === "claude" ? [...EFFORT_LEVELS, "ultracode"] : [...EFFORT_LEVELS];
}

/**
 * Cache a derivation on log-array identity. appendEventBatch keeps untouched
 * session arrays stable, so a WeakMap keyed on the log hits for every session
 * that did not emit this frame.
 */
export function perLog<T>(
  cache: WeakMap<object, { value: T }>,
  log: readonly EngineEvent[],
  build: (log: readonly EngineEvent[]) => T,
): T {
  const hit = cache.get(log);
  if (hit !== undefined) return hit.value;
  const value = build(log);
  cache.set(log, { value });
  return value;
}

/** Return `previous` when every value is identically the same so memo() children can hit. */
export function reuseRecord<T>(
  previous: Record<string, T> | undefined,
  next: Record<string, T>,
): Record<string, T> {
  if (previous === undefined) return next;
  const nextKeys = Object.keys(next);
  if (nextKeys.length !== Object.keys(previous).length) return next;
  for (const key of nextKeys) {
    if (previous[key] !== next[key]) return next;
  }
  return previous;
}

export function reuseList<T>(previous: readonly T[] | undefined, next: readonly T[]): readonly T[] {
  if (previous === undefined || previous.length !== next.length) return next;
  for (let i = 0; i < next.length; i += 1) {
    if (previous[i] !== next[i]) return next;
  }
  return previous;
}

/** Canvas session rows. A row whose visible fields did not change keeps its
 * identity, and so does the list, so registry pushes that touch only status or
 * timestamps leave memo(AgentCanvas) alone. */
export function canvasSessionRows(previous: readonly AgentCanvasSession[] | undefined,
    sessions: readonly SessionSummary[]): readonly AgentCanvasSession[] {
  const prevByKey = new Map((previous ?? []).map(row => [row.key, row] as const));
  return reuseList(previous, sessions.map(summary => {
    const key = sessionKey(summary.engine, summary.sessionId);
    const cwd = summary.projectRoot ?? summary.cwd;
    const prev = prevByKey.get(key);
    if (prev !== undefined && prev.title === summary.title && prev.engine === summary.engine
        && prev.cwd === cwd && prev.model === summary.model && prev.effort === summary.effort) {
      return prev;
    }
    return {
      key,
      title: summary.title,
      engine: summary.engine,
      cwd,
      ...(summary.model === undefined ? {} : { model: summary.model }),
      ...(summary.effort === undefined ? {} : { effort: summary.effort }),
    };
  }));
}

export type OutputFeedbackTarget = { key: SessionKey; title: string };

/** Output preview feedback targets. A row whose title did not change keeps its
 * identity, and so does the list, so registry pushes that touch only status or
 * timestamps leave memo(IntegratedTerminalCenter) alone. */
export function outputFeedbackTargetRows(
  previous: readonly OutputFeedbackTarget[] | undefined,
  sessions: readonly SessionSummary[],
): readonly OutputFeedbackTarget[] {
  const prevByKey = new Map((previous ?? []).map(row => [row.key, row] as const));
  const byKey = new Map(sessions.map(summary => [
    sessionKey(summary.engine, summary.sessionId),
    summary,
  ] as const));
  return reuseList(previous, Array.from(byKey.entries(), ([key, summary]) => {
    const title = summary.title || key;
    const prev = prevByKey.get(key);
    return prev !== undefined && prev.title === title ? prev : { key, title };
  }));
}

/** turnProgress per log identity and registry status: a tile whose log did not change pays nothing. */
const TURN_PROGRESS_BY_LOG = new WeakMap<object, { value: Map<SessionStatus, TurnProgress> }>();
export function liveTurnProgress(log: readonly EngineEvent[], status: SessionStatus): TurnProgress {
  const byStatus = perLog(TURN_PROGRESS_BY_LOG, log,
    () => new Map<SessionStatus, TurnProgress>());
  let progress = byStatus.get(status);
  if (progress === undefined) {
    progress = turnProgress(log, status, true);
    byStatus.set(status, progress);
  }
  return progress;
}

/** The newest models.available per log: the one event-derived input choicesFor merges. */
const MODELS_EVENT_BY_LOG = new WeakMap<object, { value: EngineEvent | undefined }>();
export function newestModelsAvailable(log: readonly EngineEvent[]): EngineEvent | undefined {
  for (let i = log.length - 1; i >= 0; i--) {
    if (log[i].kind === "models.available") return log[i];
  }
  return undefined;
}

export interface CatalogueRevision {
  byKey: Map<SessionKey, EngineEvent | undefined>;
  revision: number;
}

/**
 * The catalogue revision after `events`: bumps when a session's newest
 * models.available changed, a session gained one, or a session that held one
 * left. A frame that merely streamed text keeps the revision. Exported so the
 * swap case is testable without mounting App.
 */
export function nextCatalogueRevision(
  previous: CatalogueRevision,
  events: Readonly<Record<SessionKey, readonly EngineEvent[]>>,
): CatalogueRevision {
  const byKey = new Map<SessionKey, EngineEvent | undefined>();
  let changed = false;
  for (const [key, log] of Object.entries(events)) {
    const newest = perLog(MODELS_EVENT_BY_LOG, log, newestModelsAvailable);
    byKey.set(key, newest);
    if (previous.byKey.get(key) !== newest) changed = true;
  }
  // Keys that left, and only those that took a catalogue with them. A size
  // check would miss a same-size swap: one log with a catalogue removed in the
  // frame another without one arrived. But a session that never reported one
  // leaving changes nothing choicesFor can read, and bumping there re-minted
  // the callback — and every composer memo keyed on it — for nothing.
  for (const [key, newest] of previous.byKey) {
    if (newest !== undefined && !byKey.has(key)) changed = true;
  }
  return { byKey, revision: changed ? previous.revision + 1 : previous.revision };
}

/**
 * The parts of sessionConfigFor that read the event log, cached per log
 * identity: the newest models.available list, the config in force and the
 * fast-mode state — three reverse walks per tile per frame otherwise.
 */
interface SessionConfigEvents {
  available: ModelOption[] | undefined;
  confirmed: ConfigInForce;
  fastMode: FastModeInForce | null;
}
const CONFIG_EVENTS_BY_LOG = new WeakMap<object, { value: SessionConfigEvents }>();
export function sessionConfigEvents(events: readonly EngineEvent[]): SessionConfigEvents {
  return perLog(CONFIG_EVENTS_BY_LOG, events, log => {
    let available: ModelOption[] | undefined;
    for (let i = log.length - 1; i >= 0; i--) {
      const evt = log[i];
      if (evt.kind === "models.available") {
        available = evt.models.filter(option => option.value !== "default");
        break;
      }
    }
    return { available, confirmed: configInForce(log), fastMode: fastModeInForce(log) };
  });
}

/**
 * The previous view per session, handed back when every input is identical,
 * and the source of the previous `models` / `effortLevels` / `permissionModes`
 * arrays when only something else changed — so ConfigPills', AppSelect's and
 * AgentTile's memos can hit. Keyed by session key rather than by summary so a
 * registry push (new summary identities) does not mint new arrays either;
 * bounded by the sessions this window has drawn, like APPROVALS_BY_LOG.
 */
const CONFIG_VIEWS = new Map<SessionKey, {
  inputs: readonly unknown[];
  view: SessionConfigView;
}>();

/**
 * The conversation pane is no longer singular, so the model controls cannot be
 * derived from `selectedKey`.  This is the same precedence the old selected
 * pane used, expressed as a pure lookup for any open agent tile.
 */
export function sessionConfigFor(
  key: SessionKey,
  summary: SessionSummary,
  events: readonly EngineEvent[],
  providerModels: readonly ModelOption[],
  engineLabel: string,
  launchEffort: Record<SessionKey, string | null>,
  pending: PendingMap,
  configErrors: Record<SessionKey, string>,
  modelPresets: readonly ModelPreset[],
  permissionModes: readonly string[],
  dangerousModes: readonly string[],
  currentPermission: string,
  pendingPermission: boolean,
  pendingFastMode: boolean,
  /** A mode picked in this window, Manual included (protocol 68). */
  chosenPermission?: string,
): SessionConfigView {
  const fromEvents = sessionConfigEvents(events);
  const inputs = [summary, fromEvents, providerModels, engineLabel, launchEffort[key],
    pending[key], configErrors[key], modelPresets, permissionModes, dangerousModes,
    currentPermission, pendingPermission, pendingFastMode, chosenPermission];
  const previous = CONFIG_VIEWS.get(key);
  if (previous !== undefined && previous.inputs.every((input, i) => input === inputs[i])) {
    return previous.view;
  }
  const prior = previous?.view;
  // A closed/restored session may not have emitted `models.available` during
  // this renderer run. Keep the provider probe catalogue as the baseline so
  // its model picker is still a real list instead of an arbitrary-id textbox;
  // a live session event remains authoritative when it arrives.
  const models: ModelOption[] = fromEvents.available ?? [...providerModels];
  const confirmed = fromEvents.confirmed;
  const liveModel = confirmed.hasConfig
    ? confirmed.model
    : confirmed.model ?? (summary.model || undefined);
  const liveEffort = confirmed.hasConfig
    ? confirmed.effort ?? null
    : launchEffort[key] ?? summary.effort ?? null;
  const inFlight = pending[key];
  const requested = inFlight !== undefined && "model" in inFlight
    ? inFlight.model : liveModel;
  const model = requested === undefined || requested === "" || requested === "default"
    ? latestModelPresetFor(modelPresets, summary.engine)?.model
      ?? models.find(option => option.value !== "" && option.value !== "default")?.value
    : requested;
  const row = models.find(option => option.value === model);
  const advertisedLevels = row !== undefined && row.effortLevels.length > 0
    ? row.effortLevels : fallbackEffortLevels(summary.engine);
  let effortLevels = concreteEffortLevels(advertisedLevels);
  const rawEffort = inFlight !== undefined && "effort" in inFlight
    ? inFlight.effort ?? null : liveEffort;
  if (rawEffort !== null && rawEffort !== "" && rawEffort !== "default"
      && !effortLevels.includes(rawEffort)) {
    effortLevels = [rawEffort, ...effortLevels];
  }
  const remembered = model !== undefined && model !== "" && model !== "default"
    ? modelPresetFor(modelPresets, summary.engine, model)?.effort : undefined;
  const effort = preferredEffort(effortLevels, rawEffort, remembered) || null;
  const canSetModel = summary.engine === "claude" || summary.engine === "codex";
  // Built-in Grok records a live effort choice and applies it on the next
  // session/prompt. A configured ACP profile can too, but only when its
  // catalogue advertises levels — that is the engine telling us it understands
  // the axis. Gate on the levels themselves so a profile that reports none
  // still shows no control.
  const canSetEffort = canSetModel || summary.engine === "acp:grok"
    || (summary.engine.startsWith("acp:") && effortLevels.length > 0);
  // Workspace sessions can change access before their lazy resume. Plain
  // Chat and Voice reject that operation even when their engine supports it.
  const canSetPermission = sessionHasPermissionControls(summary)
    && (canSetModel || summary.engine === "acp:grok");
  const pickerDangerous = summary.engine === "acp:grok" ? [] : dangerousModes;
  const rememberedPermission = model !== undefined && model !== "" && model !== "default"
    ? modelPresetFor(modelPresets, summary.engine, model)?.permissionMode : undefined;
  const permissionValue = sessionPermissionMode(summary, currentPermission, rememberedPermission,
    chosenPermission);
  const picker = permissionPicker(permissionModes, pickerDangerous, permissionValue ?? "default");
  const fastMode = fromEvents.fastMode;
  const view: SessionConfigView = {
    engine: summary.engine,
    engineLabel,
    model,
    effort,
    models: reuseList(prior?.models, models) as ModelOption[],
    canSetModel,
    effortLevels: reuseList(prior?.effortLevels, effortLevels) as string[],
    // Grok always has an effort axis. A value-only catalogue stub used to
    // claim supportsEffort: false and hide the pill on a live session.
    showEffort: summary.engine === "acp:grok" || row?.supportsEffort !== false,
    canSetEffort,
    permissionMode: permissionValue,
    permissionModes: reuseList(prior?.permissionModes, picker.options),
    permissionUnknown: picker.currentUnknown,
    canSetPermission,
    pendingPermission,
    supportsFastMode: row?.supportsFastMode === true,
    fastMode: fastMode?.state ?? null,
    ...(fastMode?.reason === undefined ? {} : { fastModeReason: fastMode.reason }),
    pendingFastMode,
    pendingModel: inFlight !== undefined && "model" in inFlight,
    pendingEffort: inFlight !== undefined && "effort" in inFlight,
    error: configErrors[key] ?? null,
  };
  // Registry pushes replace every summary object, so an unchanged session
  // misses the input check above; keeping the equal view's identity is what
  // lets the memoized pane skip that push.
  const kept = prior !== undefined && equalValue(prior, view) ? prior : view;
  CONFIG_VIEWS.delete(key);
  CONFIG_VIEWS.set(key, { inputs, view: kept });
  if (CONFIG_VIEWS.size > 256) CONFIG_VIEWS.delete(CONFIG_VIEWS.keys().next().value!);
  return kept;
}

/**
 * Outstanding approvals are session-local.  In particular, registry refreshes
 * must not make one disappear merely because a provider lists a live waiting
 * session as `working`; only the event log and the verdict sent for this exact
 * request can settle it.
 */
const NO_ACTIVE_APPROVALS: ReadonlySet<string> = new Set();
const APPROVALS_BY_LOG = new WeakMap<object, {
  live: boolean;
  verdicts: Record<string, ApprovalVerdict>;
  key: SessionKey;
  value: ReadonlySet<string>;
}>();

export function activeApprovalsFor(
  key: SessionKey,
  events: readonly EngineEvent[],
  live: boolean,
  verdicts: Record<string, ApprovalVerdict>,
): ReadonlySet<string> {
  if (!live) return NO_ACTIVE_APPROVALS;
  const hit = APPROVALS_BY_LOG.get(events);
  if (hit !== undefined && hit.live === live && hit.verdicts === verdicts && hit.key === key) {
    return hit.value;
  }
  const ids = new Set<string>();
  for (const evt of events) {
    if (evt.kind === "approval.requested") {
      const answer = verdicts[approvalVerdictKey(key, evt.approvalId)];
      if (answer === undefined || answer.error !== undefined) ids.add(evt.approvalId);
    }
    if (eventEndsTurn(evt)) ids.clear();
  }
  const value = ids.size === 0 ? NO_ACTIVE_APPROVALS : ids;
  APPROVALS_BY_LOG.set(events, { live, verdicts, key, value });
  return value;
}
