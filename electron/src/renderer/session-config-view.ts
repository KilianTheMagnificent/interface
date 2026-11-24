import type { EngineEvent } from "../shared/engine";
import type { EngineOp, EngineOpSupport } from "../shared/ipc";

/**
 * What each engine can be asked to do, what a session is running (config and
 * fast mode in force), and the mid-session permission picker.
 */

/* ---- what each engine can be asked to do ---------------------------------
   Every engine lifecycle operation is optional and the two engines
   genuinely differ (Codex cannot stamp an adoption at all — its metadata call
   carries three git fields and nothing else). The UI hides what is not listed
   rather than offering a button that rejects, and it must never decide from the
   engine's NAME: that is a hardcoded capability table by another name, and it
   goes stale the first time an engine gains a method. */

export type EngineOpMap = Record<string, readonly EngineOp[]>;

export function engineOpsMap(support: readonly EngineOpSupport[]): EngineOpMap {
  const out: Record<string, readonly EngineOp[]> = {};
  for (const entry of support) out[entry.engine] = entry.ops;
  return out;
}

/**
 * An engine nobody has reported on can do NOTHING, deliberately: the probe
 * answers before the user can click, and guessing the other way puts a button
 * in front of them that fails.
 */
export function engineCan(map: EngineOpMap, engine: string, op: EngineOp): boolean {
  return map[engine]?.includes(op) === true;
}

/**
 * "default" is the composer's word for "let the engine decide", and it must
 * never travel to an engine as a permission mode.
 *
 * It reached one. Measured against codex app-server 0.147.0 on this machine:
 *
 *   thread/start { approvalPolicy: "default" }
 *     -> -32600 Invalid request: unknown variant `default`,
 *        expected one of `untrusted`, `on-request`, `granular`, `never`
 *   thread/start { approvalPolicy: "on-request" }  -> OK
 *   thread/start { }                               -> OK
 *
 * The composer seeds every form with permissionMode "default", the launch
 * builder turns that word into an absent field for the model and the effort but
 * not for this one, and the Codex adapter forwards what it is given — so every
 * Codex session launched from the card failed on its first round-trip. Claude
 * accepts "default" as a real PermissionMode, so removing it is a no-op there:
 * absent and "default" mean the same thing to that engine.
 *
 * Kept here, pure and tested, rather than inline at the call: it is one line of
 * behaviour that no renderer test could otherwise reach, and it is the line
 * whose absence broke half the app's engines.
 */
export function withoutDefaultPermissionMode<T extends { permissionMode?: string }>(
    request: T): T {
  if (request.permissionMode !== "default") return request;
  const out = { ...request };
  delete out.permissionMode;
  return out;
}

/* ---- what the selected session is actually running ------------------------ */

/** The model and effort the event log PROVES are in force for the current run. */
export interface ConfigInForce {
  /**
   * A `config.changed` from THIS run set both values. It is the flag that keeps
   * `model: undefined` — the user picking the engine's own default — distinct
   * from "nothing has been said yet", which would otherwise resurrect the model
   * the session was launched on.
   */
  hasConfig: boolean;
  model: string | undefined;
  effort: string | null | undefined;
}

/**
 * Read backwards out of one session's log. The newest `config.changed` is the
 * whole answer when there is one — it carries both values, which is why it wins
 * over an older `session.meta` rather than falling through to it.
 *
 * THE RUN BOUNDARY, and the reason this is a function rather than four lines in
 * a component: the event log is never cleared, so it outlives the run that
 * produced it. `session.closed` is emitted exactly when the handle leaves the
 * bridge's live map (bridge.ts, the pump's `finally`). A reattach may restore
 * provider-confirmed model/effort from the durable SessionSummary, but an old
 * live event is still not evidence about the replacement handle (and access
 * overrides are intentionally not restored). The scan therefore stops there:
 * what the log below the boundary said is history, and the caller falls back to
 * the session record until the new run reports its own configuration.
 */
export function configInForce(events: readonly EngineEvent[]): ConfigInForce {
  let model: string | undefined;
  let effort: string | null | undefined;
  let hasConfig = false;
  for (let i = events.length - 1; i >= 0; i--) {
    const evt = events[i];
    if (evt.kind === "session.closed") break;
    if (evt.kind === "config.changed") {
      model = evt.model;
      effort = evt.effort ?? null;
      hasConfig = true;
      break;
    }
    if (evt.kind === "session.meta" && model === undefined && evt.model.length > 0) {
      model = evt.model;
    }
  }
  return { hasConfig, model, effort };
}

export interface FastModeInForce {
  state: import("../shared/engine").FastModeState;
  reason?: string;
}

/** Latest provider-reported fast-mode state, void once the session closed. */
export function fastModeInForce(
  events: readonly EngineEvent[],
): FastModeInForce | null {
  for (let i = events.length - 1; i >= 0; i--) {
    const evt = events[i];
    if (evt.kind === "session.closed") return null;
    if (evt.kind === "fastmode.changed") {
      return { state: evt.state,
        ...(evt.reason === undefined ? {} : { reason: evt.reason }) };
    }
  }
  return null;
}

/* ---- the mid-session permission picker ------------------------------------
   Which access modes a RUNNING session may be switched to. Pure, and here
   rather than in App.tsx, because every rule below is a real refusal the user
   would otherwise meet as an error message: the two engines have disjoint
   vocabularies, one of them ("default") is a valid mode for Claude and is
   rejected outright by Codex, and a mode that hands an agent unsupervised
   execution must not sit one click away in a status-bar dropdown. */

export interface PermissionPicker {
  /** Modes the user may switch this session TO, in the engine's own order. */
  options: string[];
  /**
   * True when the mode the session is running is not one of the engine's own —
   * Codex's "default" is the case that exists today: it is what the app shows
   * before anything has been chosen, it means "whatever the user's own
   * ~/.codex/config.toml says", and it is NOT an AskForApproval the server
   * will take (verified live: thread/start with approvalPolicy "default"
   * answers -32600 unknown variant `default`).
   *
   * The caller draws it as a DISABLED option rather than omitting it. Omitting
   * it would leave the <select> with no matching value, so the browser would
   * display the first option instead and the bar would state a mode the
   * session is not running; offering it as selectable would let the user pick
   * a value the engine rejects, in two clicks, having been shown it.
   */
  currentUnknown: boolean;
}

/**
 * `dangerous` modes are filtered OUT — with one exception: a session already
 * running one keeps it in the list, first, because that is the only way the UI
 * can both state what is armed and offer the click that disarms it.
 *
 * (The Claude adapter refuses `bypassPermissions` mid-session outright: the
 * `allowDangerouslySkipPermissions` flag it needs is a launch option that
 * cannot be added to a running query. So for that engine this filter is not
 * merely a safety preference, it is what stops the picker offering a
 * guaranteed rejection.)
 */
export function permissionPicker(modes: readonly string[],
    dangerous: readonly string[], current: string): PermissionPicker {
  const safe = modes.filter(m => !dangerous.includes(m));
  // `modes`, not `safe`: an armed dangerous mode is known to the engine, and it
  // is exactly the one that must stay visible.
  const currentUnknown = !modes.includes(current);
  const options = currentUnknown || safe.includes(current)
    ? safe
    : [current, ...safe];
  return { options, currentUnknown };
}
