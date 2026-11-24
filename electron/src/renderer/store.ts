import { appendLiveEvents } from "./live-events";
export { MAX_LIVE_EVENTS_PER_SESSION } from "./live-events";
import { sessionKey } from "../shared/engine";
import { equalValue } from "./stable-values";
import type {
  EngineEvent, ProviderCommand, SessionKey, SessionSummary,
} from "../shared/engine";
import type { EngineProbe } from "../shared/ipc";

export interface AppState {
  sessions: SessionSummary[];
  selectedKey: SessionKey | null;
  events: Record<SessionKey, EngineEvent[]>;     // per-session event log
  /** Ephemeral live control state. Never persisted or mixed into transcripts. */
  providerCommands: Partial<Record<SessionKey, ProviderCommand[]>>;
  engineHealth: EngineProbe[];
  composerOpen: boolean;
}

export type AppAction =
  | { type: "registry"; sessions: SessionSummary[] }
  | { type: "event"; key: SessionKey; evt: EngineEvent }
  | { type: "events"; items: Array<{ key: SessionKey; evt: EngineEvent }> }
  | { type: "drop"; key: SessionKey }
  | { type: "select"; key: SessionKey | null }
  | { type: "composer"; open: boolean }
  | { type: "health"; engines: AppState["engineHealth"] };

function updateProviderCommands(
    current: AppState["providerCommands"],
    items: readonly { key: SessionKey; evt: EngineEvent }[]): AppState["providerCommands"] {
  let next = current;
  for (const { key, evt } of items) {
    if (evt.kind !== "commands.available" && evt.kind !== "commands.invalidated"
        && evt.kind !== "session.closed") continue;
    if (next === current) next = { ...current };
    if (evt.kind === "session.closed") delete next[key];
    // Invalidated is deliberately a known-empty catalogue, not `undefined`.
    // Conversation treats undefined as "provider cannot advertise" and lets
    // slash-shaped text fall through as an ordinary prompt. During a model
    // refresh that would let an old command reach the model before the new
    // catalogue arrives. An empty snapshot fails closed until replacement.
    else if (evt.kind === "commands.invalidated") next[key] = [];
    else next[key] = evt.commands;
  }
  return next;
}

export const initialState: AppState = {
  // Cursor's Agents window opens on a ready prompt rather than an empty editor.
  // A session click closes this surface through App's existing select path, so
  // returning users still reach their restored workspace in one action.
  sessions: [], selectedKey: null, events: {}, providerCommands: {},
  engineHealth: [], composerOpen: true,
};

/**
 * A registry push rebuilds every row. Keep each row that did not change as the
 * previous object, and the previous array when nothing changed at all (a
 * focus refresh, a timestamp-only round that repeats), so memoized rows and
 * the listings derived from them can hit.
 */
function reuseSessionRows(
  previous: SessionSummary[], next: SessionSummary[],
): SessionSummary[] {
  const byKey = new Map(previous.map(row => [sessionKey(row.engine, row.sessionId), row]));
  let unchanged = previous.length === next.length;
  const rows = next.map((row, index) => {
    const prior = byKey.get(sessionKey(row.engine, row.sessionId));
    const kept = prior !== undefined && equalValue(prior, row) ? prior : row;
    if (kept !== previous[index]) unchanged = false;
    return kept;
  });
  return unchanged ? previous : rows;
}

// Pure: a branch that changes nothing returns `s` itself; the others return a
// fresh state and share the untouched sub-objects.
export function appReducer(s: AppState, a: AppAction): AppState {
  switch (a.type) {
    case "registry": {
      const sessions = reuseSessionRows(s.sessions, a.sessions);
      // The registry pre-orders the listing; keep the selection only if it survived.
      const kept = sessions.some(x => sessionKey(x.engine, x.sessionId) === s.selectedKey);
      const selectedKey = kept ? s.selectedKey : null;
      return sessions === s.sessions && selectedKey === s.selectedKey
        ? s : { ...s, sessions, selectedKey };
    }
    case "event": {
      const items = [a];
      const events = appendLiveEvents(s.events, items);
      const providerCommands = updateProviderCommands(s.providerCommands, items);
      return events === s.events && providerCommands === s.providerCommands
        ? s : { ...s, events, providerCommands };
    }
    case "events": {
      const events = appendLiveEvents(s.events, a.items);
      const providerCommands = updateProviderCommands(s.providerCommands, a.items);
      return events === s.events && providerCommands === s.providerCommands
        ? s : { ...s, events, providerCommands };
    }
    case "drop": {
      const hasEvents = Object.prototype.hasOwnProperty.call(s.events, a.key);
      const hasCommands = Object.prototype.hasOwnProperty.call(s.providerCommands, a.key);
      if (!hasEvents && !hasCommands) return s;
      const events = { ...s.events };
      const providerCommands = { ...s.providerCommands };
      delete events[a.key];
      delete providerCommands[a.key];
      return { ...s, events, providerCommands };
    }
    // A click or focus inside a pane re-selects its session and closes an
    // already-closed composer; a focus refresh re-reports the same health.
    // Returning `s` for those lets React skip the render.
    case "select":
      return s.selectedKey === a.key ? s : { ...s, selectedKey: a.key };
    case "composer":
      return s.composerOpen === a.open ? s : { ...s, composerOpen: a.open };
    case "health":
      return equalValue(s.engineHealth, a.engines) ? s : { ...s, engineHealth: a.engines };
  }
}
