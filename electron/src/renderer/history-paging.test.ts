import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { HISTORY_LIMIT, HISTORY_MAX_LIMIT } from "../shared/ipc";
import { HistoryPagingController, PendingEventBatch } from "./history-paging";
import { appReducer, initialState } from "./store";

const app = readFileSync(new URL("./App.tsx", import.meta.url), "utf8");
const sessionTile = readFileSync(new URL("./components/SessionTile.tsx", import.meta.url), "utf8");
const pagingSource = readFileSync(new URL("./history-paging.ts", import.meta.url), "utf8");

function begin(paging: HistoryPagingController, key: string): number {
  const generation = paging.begin(key);
  expect(generation).not.toBeNull();
  if (generation === null) throw new Error(`history unexpectedly frozen for ${key}`);
  return generation;
}

describe("HistoryPagingController", () => {
  it("starts at the shared 500-message page and advances in bounded pages", () => {
    const paging = new HistoryPagingController();
    const key = "claude#long";
    const initial = begin(paging, key);
    expect(paging.nextLimit(key)).toBeNull();
    expect(paging.settle(key, initial, HISTORY_LIMIT)).toBe(true);
    expect(paging.nextLimit(key)).toBe(HISTORY_LIMIT * 2);

    let limit = HISTORY_LIMIT * 2;
    while (limit <= HISTORY_MAX_LIMIT) {
      const generation = begin(paging, key);
      expect(paging.settle(key, generation, limit)).toBe(true);
      const expected = limit === HISTORY_MAX_LIMIT ? null : limit + HISTORY_LIMIT;
      expect(paging.nextLimit(key)).toBe(expected);
      limit += HISTORY_LIMIT;
    }
  });

  it("rejects a stale page and lets only the newest generation advance the limit", () => {
    const paging = new HistoryPagingController();
    const key = "codex#race";
    const first = begin(paging, key);
    const second = begin(paging, key);

    expect(paging.isCurrent(key, first)).toBe(false);
    expect(paging.settle(key, first, HISTORY_LIMIT)).toBe(false);
    expect(paging.nextLimit(key)).toBeNull();
    expect(paging.settle(key, second, HISTORY_LIMIT)).toBe(true);
    expect(paging.nextLimit(key)).toBe(HISTORY_LIMIT * 2);
  });

  it("retries a failed older read at the same limit and invalidates deleted keys", () => {
    const paging = new HistoryPagingController();
    const key = "claude#retry";
    const initial = begin(paging, key);
    paging.settle(key, initial, HISTORY_LIMIT);

    const failed = begin(paging, key);
    expect(paging.fail(key, failed)).toBe(true);
    expect(paging.nextLimit(key)).toBe(HISTORY_LIMIT * 2);

    const stale = begin(paging, key);
    paging.invalidate(key);
    expect(paging.isCurrent(key, stale)).toBe(false);
    expect(paging.settle(key, stale, HISTORY_LIMIT * 2)).toBe(false);
    expect(paging.nextLimit(key)).toBeNull();
  });

  it("rejects a deferred read after live then closed and stays frozen across retry", async () => {
    const paging = new HistoryPagingController();
    const key = "claude#deferred";
    const generation = begin(paging, key);
    let resolveRead: (() => void) | undefined;
    const read = new Promise<void>(resolve => { resolveRead = resolve; });
    const accepted = read.then(() => paging.settle(key, generation, HISTORY_LIMIT));

    // Live invalidates the pending generation. Ordinary close has no thaw
    // operation, and retry invalidation deliberately preserves the freeze.
    expect(paging.freeze(key)).toBe(true);
    paging.invalidate(key);
    expect(paging.isFrozen(key)).toBe(true);
    expect(paging.begin(key)).toBeNull();
    resolveRead?.();
    expect(await accepted).toBe(false);
    expect(paging.nextLimit(key)).toBeNull();
  });

  it("clears born-here and frozen state only at authoritative drop for key reuse", () => {
    const paging = new HistoryPagingController();
    const key = "codex#reused";
    paging.markStartedHere(key);
    expect(paging.startedHere(key)).toBe(true);
    expect(paging.isFrozen(key)).toBe(true);
    expect(paging.begin(key)).toBeNull();

    paging.drop(key);
    expect(paging.startedHere(key)).toBe(false);
    expect(paging.isFrozen(key)).toBe(false);
    expect(paging.settle(key, begin(paging, key), HISTORY_LIMIT)).toBe(true);
  });
});

describe("PendingEventBatch", () => {
  it("purges an old lifecycle while preserving unrelated and reused-key events", () => {
    const batch = new PendingEventBatch();
    batch.enqueue("claude#reuse", { kind: "notice", tone: "info", message: "old" });
    batch.enqueue("codex#keep", { kind: "notice", tone: "info", message: "keep" });
    batch.drop("claude#reuse");
    batch.enqueue("claude#reuse", { kind: "notice", tone: "info", message: "new" });

    expect(batch.flush()).toEqual([
      { key: "codex#keep", evt: { kind: "notice", tone: "info", message: "keep" } },
      { key: "claude#reuse", evt: { kind: "notice", tone: "info", message: "new" } },
    ]);
  });

  it("cannot resurrect dropped reducer state or wake a stale user turn at RAF flush", () => {
    const key = "claude#deleted";
    const batch = new PendingEventBatch();
    batch.enqueue(key, { kind: "user.message", text: "stale" });
    batch.drop(key);
    const flushed = batch.flush();
    let state = appReducer({
      ...initialState,
      events: { [key]: [{ kind: "notice", tone: "info", message: "retained" }] },
    }, { type: "drop", key });
    if (flushed.length > 0) state = appReducer(state, { type: "events", items: flushed });

    expect(flushed).toEqual([]);
    expect(flushed.some(item => item.evt.kind === "user.message")).toBe(false);
    expect(state.events[key]).toBeUndefined();
  });
});

describe("App history paging wiring", () => {
  it("reads the requested limit and replaces the strict-superset page", () => {
    const loader = app.slice(app.indexOf("const loadHistory = useCallback"),
      app.indexOf("/* ---- session lifecycle"));
    expect(loader).toContain("limit = HISTORY_LIMIT");
    expect(loader).toContain("window.iface.readHistory(key, { limit })");
    expect(loader).toContain("historyPages.current[key] = { events, items };");
    expect(loader).not.toContain("HISTORY_MAX_LIMIT })");
    expect(loader).not.toMatch(/historyPages\.current\[key\][\s\S]*?\.concat\(/u);
  });

  it("fences success, failure, retry, and deletion through the same controller", () => {
    expect(app).toContain("historyPaging.current.isCurrent(key, generation)");
    expect(app).toContain("historyPaging.current.settle(key, generation, limit)");
    expect(app).toContain("historyPaging.current.fail(key, generation)");
    // Exactly two reviewed sites: retryHistoryFor and closeWorkspaceTile.
    expect(app.match(/historyPaging\.current\.invalidate\(key\)/gu)).toHaveLength(2);
    expect(app).toContain("historyPaging.current.drop(key)");
  });

  it("freezes both initial and expanded reads across the full live lifecycle", () => {
    const loader = app.slice(
      app.indexOf("const loadHistory = useCallback"),
      app.indexOf("/* ---- session lifecycle"),
    );
    const loadOlder = loader.slice(loader.indexOf("const loadOlderHistoryFor"));
    // Frozen means no read, except the one late read of a session that went
    // live before any page of it was read (history-late-read.test.ts).
    expect(loader).toContain("if (!late && historyPaging.current.isFrozen(key)) return;");
    expect(loader).toContain('const late = mode === "initial" && historyPaging.current.canReadLate(key);');
    expect(loader).toContain("if (generation === null) return;");
    expect(loadOlder).toContain("if (historyPaging.current.isFrozen(key)) return;");
    expect(app).toContain("historyFrozen={historyPaging.current.isFrozen(key)}");
    expect(sessionTile).toContain("onLoadOlderHistory={historyFrozen ? undefined");
    expect(sessionTile).toContain("onRetryHistory={historyFrozen ? undefined");
  });

  it("authoritatively drops renderer-only state after a successful delete", () => {
    const cleanup = app.slice(
      app.indexOf("const dropSessionClientState = useCallback"),
      app.indexOf("const workerSessionsOf"),
    );
    const deletion = app.slice(
      app.indexOf("const deleteSession = useCallback"),
      app.indexOf("const forkSession"),
    );
    expect(cleanup).toContain('dispatch({ type: "drop", key });');
    expect(cleanup).toContain("historyPaging.current.drop(key);");
    expect(cleanup).toContain("pendingEventBatch.current.drop(key);");
    // The notification edge record is stored, so a deleted key must leave it.
    expect(cleanup).toContain("lastNotified.current = withoutKey(lastNotified.current, key);");
    expect(cleanup).toContain("writeStored(LAST_NOTIFIED, JSON.stringify(lastNotified.current));");
    expect(deletion).toContain(".then(() => window.iface.deleteSession(key))");
    expect(deletion).toContain(".then(() => { dropSessionClientState(key); })");
  });
  it("frees a closed pane's stored page but never one the live stream froze", () => {
    const start = app.indexOf("const closeWorkspaceTile = useCallback");
    const close = app.slice(start, app.indexOf("/* ---- the subagent", start));
    // A frozen key can never re-read its history, so dropping its view would
    // lose the transcript for the rest of the run. Everything else reloads.
    expect(close).toContain("if (historyPaging.current.isFrozen(key)) return;");
    expect(close).toContain("delete historyPages.current[key];");
    expect(close).toContain("historyAsked.current.delete(key);");
    expect(close).toContain("setHistoryByKey(map => withoutKey(map, key));");
    // invalidate() clears the settled limit as well, so a reopen re-reads the
    // page the user had expanded rather than dropping back to HISTORY_LIMIT.
    expect(close).toContain("historyPaging.current.invalidate(key);");
    // The permanent fences stay put: closing a pane is not a deletion.
    expect(close).not.toContain("historyPaging.current.drop(key)");

    const paging = new HistoryPagingController();
    const key = "claude#reopened";
    expect(paging.settle(key, begin(paging, key), HISTORY_LIMIT)).toBe(true);
    expect(paging.nextLimit(key)).toBe(HISTORY_LIMIT * 2);
    expect(paging.settle(key, begin(paging, key), HISTORY_LIMIT * 2)).toBe(true);
    paging.invalidate(key);
    // Back to an unread key: the reopen's initial read asks for the base page
    // and the ladder starts again, instead of nextLimit() jumping past it.
    expect(paging.nextLimit(key)).toBeNull();
    expect(paging.isFrozen(key)).toBe(false);
    expect(paging.startedHere(key)).toBe(false);
  });

  it("settles the paging fence before reducing the page it accepted", () => {
    const loader = app.slice(app.indexOf("const loadHistory = useCallback"),
      app.indexOf("/* ---- session lifecycle"));
    const settle = loader.indexOf("historyPaging.current.settle(key, generation, limit)");
    const reduce = loader.indexOf("const items = historyPageItems(events");
    expect(settle).toBeGreaterThan(0);
    expect(reduce).toBeGreaterThan(settle);
  });
});

describe("App event batching", () => {
  it("uses a timer instead of an animation frame while the window is hidden", () => {
    const enqueue = app.slice(app.indexOf("const flushPendingEvents = useCallback"),
      app.indexOf("const lastNotified = useRef"));
    // A hidden window's frames are throttled or stopped outright, which would
    // strand a background session's transcript in the batch until it returned.
    expect(enqueue).toContain('if (document.visibilityState === "hidden") {');
    expect(enqueue).toContain("pendingEventTimer.current = window.setTimeout(flushPendingEvents, 0)");
    expect(enqueue).toContain("window.requestAnimationFrame(flushPendingEvents)");
    expect(enqueue).toContain(
      "if (pendingEventFrame.current !== null || pendingEventTimer.current !== null) return;");
    // The generation fence still decides what survives the flush.
    expect(enqueue).toContain("pendingEventBatch.current.flush()");
  });

  it("applies a registry push in the same flush, after the events it follows", () => {
    const flush = app.slice(app.indexOf("const flushPendingEvents = useCallback"),
      app.indexOf("const armEventFlush = useCallback"));
    const events = flush.indexOf('dispatch({ type: "events", items })');
    const registry = flush.indexOf('dispatch({ type: "registry", sessions: registry })');
    expect(events).toBeGreaterThan(0);
    expect(registry).toBeGreaterThan(events);
    const onRegistry = app.slice(app.indexOf("window.iface.onRegistry("),
      app.indexOf("window.iface.onEvent("));
    expect(onRegistry).toContain("pendingRegistry.current = sessions;");
    expect(onRegistry).toContain("armEventFlush();");
    expect(onRegistry).not.toContain('dispatch({ type: "registry"');
    // The listing answer supersedes a push still waiting for the frame.
    const listing = app.slice(app.indexOf("window.iface.listSessionSnapshot()"),
      app.indexOf("window.iface.probeEngines("));
    expect(listing.indexOf("pendingRegistry.current = null;"))
      .toBeLessThan(listing.indexOf('dispatch({ type: "registry", sessions: snapshot.sessions })'));
  });

  it("drains the batch on the way into the background, and cancels both schedulers", () => {
    expect(app).toMatch(
      /const onVisible = \(\): void => \{[\s\S]*?flushPendingEvents\(\);/u);
    const flush = app.slice(app.indexOf("const flushPendingEvents = useCallback"),
      app.indexOf("const enqueueEvent = useCallback"));
    expect(flush).toContain("window.cancelAnimationFrame(pendingEventFrame.current)");
    expect(flush).toContain("window.clearTimeout(pendingEventTimer.current)");
  });

  it("flushes in one pass and clears the fence without copying it first", () => {
    const batch = pagingSource.slice(pagingSource.indexOf("export class PendingEventBatch"));
    expect(batch).toContain("item.generation === this.generations.get(item.key)");
    expect(batch).not.toContain("new Map(this.generations)");

    // The fence itself: a dropped key's queued events never reach the reducer.
    const batchObject = new PendingEventBatch();
    batchObject.enqueue("claude#kept", { kind: "notice", tone: "info", message: "kept" });
    batchObject.enqueue("claude#gone", { kind: "user.message", text: "stale" });
    batchObject.drop("claude#gone");
    expect(batchObject.flush()).toEqual([
      { key: "claude#kept", evt: { kind: "notice", tone: "info", message: "kept" } },
    ]);
    // Generations are cleared by the flush, so a later batch starts fresh.
    batchObject.enqueue("claude#kept", { kind: "notice", tone: "info", message: "next" });
    expect(batchObject.flush()).toHaveLength(1);
  });
});
