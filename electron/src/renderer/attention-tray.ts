/**
 * The attention tray's state: what needs the user, and where.
 *
 * Two kinds of row share one list. LIVE rows are the agents and panes blocked
 * on the user right now — derived on every render from the same listing the
 * dock badge and the sidebar's pinned section use, so the tray can never
 * disagree with them, and they leave by themselves the moment the need does.
 * EVENT rows are turns that finished or failed while the user was not
 * watching; they are recorded at the edge that already chimes and stay until
 * the user opens them or clears the list.
 *
 * Pure: every function returns a new state, or the SAME object when nothing
 * changed, so an effect can call it on every render without re-rendering.
 */
import type { SessionKey } from "../shared/engine";
import type { HarnessState, HarnessView } from "../shared/harness";
import { statusWord } from "./components/status-words";
import { sessionAgeLabel } from "./sidebar-view";
import { lastSegment } from "./display-paths";

export type AttentionKind = "needs-input" | "failed" | "finished";

export type AttentionTarget =
  | { readonly kind: "session"; readonly key: SessionKey }
  | { readonly kind: "terminal"; readonly terminalId: string; readonly generation?: number };

/** One thing the user is needed for. */
export interface AttentionRow {
  /** Stable per need: `session:<key>` / `terminal:<id>` for live rows, unique per event. */
  readonly id: string;
  readonly kind: AttentionKind;
  readonly target: AttentionTarget;
  readonly title: string;
  /** Where it is: the project, and for a pane its workspace. "" when unknown. */
  readonly where: string;
  /** When the need arose, epoch ms. */
  readonly at: number;
  /** Live rows clear themselves; events stay until dismissed. */
  readonly live: boolean;
}

/** A need that is true right now, before the tray knows since when. */
export type LiveNeed = Pick<AttentionRow, "id" | "target" | "title" | "where">;

export interface AttentionState {
  /** Finished/failed turns, newest first, until dismissed or cleared. */
  readonly events: readonly AttentionRow[];
  /** First time each live need was seen, by row id. */
  readonly since: Readonly<Record<string, number>>;
  /** When the tray was last opened; rows newer than this are unread. */
  readonly openedAt: number;
}

export const EMPTY_ATTENTION: AttentionState = { events: [], since: {}, openedAt: 0 };

/** Events kept. Older ones fall off the end; a tray is not a log. */
export const ATTENTION_EVENT_CAP = 50;

/** Remember when each live need began; forget the ones that ended. */
export function trackNeeds(state: AttentionState, needs: readonly LiveNeed[],
    now: number): AttentionState {
  const since: Record<string, number> = {};
  let changed = false;
  for (const need of needs) {
    const seen = state.since[need.id];
    since[need.id] = seen ?? now;
    if (seen === undefined) changed = true;
  }
  if (!changed && Object.keys(state.since).length === needs.length) return state;
  return { ...state, since };
}

/** Prepend an event. The same id twice replaces rather than repeats. */
export function recordEvent(state: AttentionState, event: Omit<AttentionRow, "live">): AttentionState {
  const rest = state.events.filter(row => row.id !== event.id);
  return { ...state, events: [{ ...event, live: false }, ...rest].slice(0, ATTENTION_EVENT_CAP) };
}

export function dismissEvent(state: AttentionState, id: string): AttentionState {
  if (!state.events.some(row => row.id === id)) return state;
  return { ...state, events: state.events.filter(row => row.id !== id) };
}

export function clearEvents(state: AttentionState): AttentionState {
  return state.events.length === 0 ? state : { ...state, events: [] };
}

export function markOpened(state: AttentionState, now: number): AttentionState {
  return state.openedAt === now ? state : { ...state, openedAt: now };
}

/** Every row, newest first: the live needs, then the events between them. */
export function attentionRows(state: AttentionState, needs: readonly LiveNeed[]): AttentionRow[] {
  const live: AttentionRow[] = needs.map(need => ({
    ...need, kind: "needs-input", at: state.since[need.id] ?? 0, live: true,
  }));
  return [...live, ...state.events].sort((a, b) => b.at - a.at);
}

/** Rows that arrived since the tray was last opened. */
export function unreadCount(rows: readonly AttentionRow[], openedAt: number): number {
  return rows.filter(row => row.at > openedAt).length;
}

/**
 * The harness's attention history as tray event rows: its unresolved finished,
 * failed and interrupted items, newest first and capped. Needs-input items are
 * live rows, derived elsewhere; an interrupted turn reads as failed.
 */
export function harnessAttentionEvents(state: Pick<HarnessState, "attention">): AttentionRow[] {
  return state.attention.filter(item => !item.resolved && item.kind !== "needs-input")
    .slice(-ATTENTION_EVENT_CAP).reverse().map(item => ({
      id: item.id, kind: item.kind === "interrupted" ? "failed" as const : item.kind as "finished" | "failed",
      target: item.target, title: item.title, where: item.place, at: item.at, live: false,
    }));
}

/**
 * The bell's count. With a harness read it is the harness's unseen, unresolved
 * items plus the live rows that arrived since the tray was last opened;
 * without one, every row that arrived since then.
 */
export function attentionUnreadCount(view: Pick<HarnessView, "state"> | null,
    rows: readonly AttentionRow[], openedAt: number): number {
  return view === null ? unreadCount(rows, openedAt)
    : view.state.attention.filter(item => !item.seen && !item.resolved).length
      + unreadCount(rows.filter(row => row.live), openedAt);
}

/** The row's one word, from the table every other surface prints. */
export function attentionWord(kind: AttentionKind): string {
  if (kind === "needs-input") return statusWord("waiting");
  if (kind === "failed") return statusWord("failed");
  return "Finished";
}

/** "Needs attention · Interface · 4m": the parts that are known, and no dash for the rest. */
export function attentionRowMeta(row: AttentionRow, now: number): string {
  const age = Number.isFinite(row.at) && row.at > 0
    ? sessionAgeLabel(new Date(row.at).toISOString(), now) : "";
  return [attentionWord(row.kind), row.where, age].filter(part => part !== "").join(" · ");
}

/** The bell's accessible name: the count, and how many of them are new. */
export function attentionTrayLabel(count: number, unread: number): string {
  if (count === 0) return "Attention: nothing needs you";
  const items = count === 1 ? "1 item" : `${count.toString()} items`;
  return unread > 0 ? `Attention: ${items}, ${unread.toString()} new` : `Attention: ${items}`;
}

/** The last path segment: the folder the row happened in. */
export function attentionPlace(path: string | null | undefined): string {
  return path === null || path === undefined ? "" : lastSegment(path);
}
