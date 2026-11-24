import type { EngineEvent, SessionKey } from "../shared/engine";
import { HISTORY_LIMIT, HISTORY_MAX_LIMIT } from "../shared/ipc";
import type { HistoryView } from "./conversation-types";
import { conversationItems } from "./conversation-reducer";

/* History paging and live-event batching: App's two per-session fences for
   what reaches a pane. Pure, so history-paging.test.ts drives them directly. */

// Shared identities. `history` must switch WITH the session on the same render
// (Conversation's prop contract §2), so these are constants rather than fresh
// objects that would re-render the pane on every keystroke elsewhere.
export const IDLE_HISTORY: HistoryView = { status: "idle" };
export const LOADING_HISTORY: HistoryView = { status: "loading" };
const HISTORY_FROZEN_MESSAGE =
  "Earlier messages cannot be loaded after this session became live.";

/** Settle the visible read state without discarding an already-safe history prefix.
 * `lateRead`: the session went live before its history was read, and its one
 * late read (`HistoryPagingController.beginLate`) is still to come. */
export function frozenHistoryView(current: HistoryView | undefined, lateRead = false): HistoryView {
  if (current === undefined || current.status === "loading") return lateRead ? LOADING_HISTORY : IDLE_HISTORY;
  if (current.status !== "ready") return current;
  return {
    ...current,
    loadingOlder: false,
    canLoadOlder: false,
    ...(current.loadingOlder === true
      ? { loadOlderError: HISTORY_FROZEN_MESSAGE }
      : {}),
  };
}

/**
 * The part of a history page that predates a live stream which began before
 * the page was read (`HistoryPagingController.beginLate`). Such a page also
 * holds the live turn, as prose `mergeTranscript` cannot dedupe, so it is cut
 * where the live stream begins:
 *  - at the history copy of the stream's first user turn, matched by text: the
 *    n-th from the end of the page's user turns with that text, n being how
 *    many the stream holds (a host-started send shows its prompt live);
 *  - when the stream holds no user turn but drew something (it joined a turn
 *    already running), right after the page's last user turn: that prompt
 *    predates the stream, its reply is the stream;
 *  - otherwise, or when the page does not hold the stream's prompt (it was
 *    read before the provider stored it), the whole page.
 * Only top-level user turns count: a subagent's prompt is not the owner's.
 */
export function historyBeforeLive(page: readonly EngineEvent[], live: readonly EngineEvent[]): EngineEvent[] {
  const userText = (event: EngineEvent): string | null =>
    event.kind === "user.message" && event.agentKey === undefined ? event.text.trim() : null;
  const liveTurns = live.map(userText).filter((text): text is string => text !== null);
  if (liveTurns.length > 0) {
    const first = liveTurns[0];
    let wanted = liveTurns.filter(text => text === first).length;
    for (let i = page.length - 1; i >= 0; i--) {
      if (userText(page[i]) !== first) continue;
      wanted -= 1;
      if (wanted === 0) return page.slice(0, i);
    }
    return page.slice();
  }
  if (conversationItems(live as EngineEvent[]).length === 0) return page.slice();
  for (let i = page.length - 1; i >= 0; i--) {
    if (userText(page[i]) !== null) return page.slice(0, i + 1);
  }
  return page.slice();
}

/**
 * Request identity and settled page size for one renderer's history reads.
 *
 * The global sequence makes a response permanently stale once a newer request
 * starts or the key is invalidated, including when the same provider key is
 * deleted and later re-imported. Settled limits advance only after an accepted
 * response, so retrying a failed older-page read asks for that same page again.
 */
export class HistoryPagingController {
  private sequence = 0;
  private readonly active = new Map<SessionKey, number>();
  private readonly settledLimits = new Map<SessionKey, number>();
  private readonly frozen = new Set<SessionKey>();
  private readonly started = new Set<SessionKey>();
  /** Frozen before any page was read: one late read is still allowed. */
  private readonly lateAllowed = new Set<SessionKey>();
  private readonly lateActive = new Map<SessionKey, number>();

  begin(key: SessionKey): number | null {
    if (this.frozen.has(key)) return null;
    const generation = this.sequence + 1;
    this.sequence = generation;
    this.active.set(key, generation);
    return generation;
  }

  isCurrent(key: SessionKey, generation: number): boolean {
    return (!this.frozen.has(key) && this.active.get(key) === generation)
      || this.lateActive.get(key) === generation;
  }

  settle(key: SessionKey, generation: number, limit: number): boolean {
    if (!this.isCurrent(key, generation)) return false;
    this.active.delete(key);
    this.lateActive.delete(key);
    this.settledLimits.set(key, Math.min(HISTORY_MAX_LIMIT, Math.max(HISTORY_LIMIT, limit)));
    return true;
  }

  fail(key: SessionKey, generation: number): boolean {
    if (!this.isCurrent(key, generation)) return false;
    this.active.delete(key);
    this.lateActive.delete(key);
    return true;
  }

  nextLimit(key: SessionKey): number | null {
    if (this.frozen.has(key) || this.active.has(key)) return null;
    const current = this.settledLimits.get(key);
    if (current === undefined || current >= HISTORY_MAX_LIMIT) return null;
    return Math.min(HISTORY_MAX_LIMIT, current + HISTORY_LIMIT);
  }

  invalidate(key: SessionKey): void {
    this.active.delete(key);
    this.settledLimits.delete(key);
  }

  /** Permanently stop history reads once this renderer has seen the live stream.
   * A session that had no page read yet (none asked, or one still in flight)
   * keeps one late read (`beginLate`): a send the HOST started (a scheduled or
   * watch prompt, a resume, a team notice, an outside agent) can be the first
   * thing this window hears of a thread, at launch before any read. */
  freeze(key: SessionKey): boolean {
    const changed = !this.frozen.has(key);
    if (changed && !this.settledLimits.has(key) && !this.started.has(key)) this.lateAllowed.add(key);
    this.frozen.add(key);
    this.active.delete(key);
    return changed;
  }

  /** Whether a frozen session's one late read is still to come. */
  canReadLate(key: SessionKey): boolean {
    return this.lateAllowed.has(key);
  }

  /** The one late read of a session frozen before any page was read; its
   * page must be cut to what predates the live stream (`historyBeforeLive`). */
  beginLate(key: SessionKey): number | null {
    if (!this.lateAllowed.delete(key)) return null;
    const generation = this.sequence + 1;
    this.sequence = generation;
    this.lateActive.set(key, generation);
    return generation;
  }

  isFrozen(key: SessionKey): boolean {
    return this.frozen.has(key);
  }

  /** Sessions created here already have their complete transcript in live events. */
  markStartedHere(key: SessionKey): void {
    this.started.add(key);
    this.lateAllowed.delete(key);
    this.lateActive.delete(key);
    this.freeze(key);
  }

  startedHere(key: SessionKey): boolean {
    return this.started.has(key);
  }

  /** Authoritative provider deletion is the sole boundary that permits key reuse. */
  drop(key: SessionKey): void {
    this.active.delete(key);
    this.lateAllowed.delete(key);
    this.lateActive.delete(key);
    this.settledLimits.delete(key);
    this.frozen.delete(key);
    this.started.delete(key);
  }
}

type PendingRendererEvent = { key: SessionKey; evt: EngineEvent; generation: number };

/** Animation-frame event coalescing with a per-key deletion fence. */
export class PendingEventBatch {
  private sequence = 0;
  private readonly generations = new Map<SessionKey, number>();
  private items: PendingRendererEvent[] = [];

  enqueue(key: SessionKey, evt: EngineEvent): void {
    let generation = this.generations.get(key);
    if (generation === undefined) {
      generation = this.sequence + 1;
      this.sequence = generation;
      this.generations.set(key, generation);
    }
    this.items.push({ key, evt, generation });
  }

  drop(key: SessionKey): void {
    this.generations.delete(key);
    this.items = this.items.filter(item => item.key !== key);
  }

  flush(): Array<{ key: SessionKey; evt: EngineEvent }> {
    const pending = this.items;
    this.items = [];
    // One pass: `generations` is only read here, so it can be cleared afterwards
    // instead of copied first.
    const out: Array<{ key: SessionKey; evt: EngineEvent }> = [];
    for (const item of pending) {
      if (item.generation === this.generations.get(item.key)) {
        out.push({ key: item.key, evt: item.evt });
      }
    }
    this.generations.clear();
    return out;
  }

  clear(): void {
    this.items = [];
    this.generations.clear();
  }
}
