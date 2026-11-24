import { readFileSync } from "node:fs";
import { describe, expect, it, vi } from "vitest";
import {
  TERMINAL_WEBGL_HIDDEN_RELEASE_MS, TERMINAL_WEBGL_IDLE_RELEASE_MS, TERMINAL_WEBGL_RETRY_MS,
  TERMINAL_WEBGL_STABLE_MS, TerminalWebglPool, TerminalWebglRecovery, type TerminalWebglClock,
} from "./terminal-webgl-pool";

/** A hand-driven clock: `advance` runs every timer that falls due, in order. */
function manualClock() {
  let now = 0;
  let id = 0;
  const timers = new Map<number, { at: number; task: () => void }>();
  const clock: TerminalWebglClock = {
    now: () => now,
    setTimeout: (task, ms) => { timers.set(++id, { at: now + ms, task }); return id; },
    clearTimeout: timer => { timers.delete(timer as number); },
  };
  return {
    clock,
    pending: () => timers.size,
    advance(ms: number): void {
      const until = now + ms;
      for (;;) {
        const due = [...timers].filter(([, timer]) => timer.at <= until)
          .sort(([, a], [, b]) => a.at - b.at)[0];
        if (due === undefined) break;
        timers.delete(due[0]);
        now = due[1].at;
        due[1].task();
      }
      now = until;
    },
  };
}

describe("terminal GPU renderer cache", () => {
  it("reuses renderers across workspace round trips without disposing them", () => {
    const pool = new TerminalWebglPool(2);
    const a = {}, b = {};
    const disposeA = vi.fn(), disposeB = vi.fn();
    expect(pool.acquire(a, disposeA)).toBe(true);
    pool.park(a);
    expect(pool.acquire(b, disposeB)).toBe(true);
    pool.park(b);
    pool.slot(a);
    pool.park(a);
    pool.slot(b);
    expect(disposeA).not.toHaveBeenCalled();
    expect(disposeB).not.toHaveBeenCalled();
    pool.release(a);
    pool.release(a);
    pool.release(b);
    expect(disposeA).toHaveBeenCalledOnce();
    expect(disposeB).toHaveBeenCalledOnce();
  });

  it("evicts the oldest hidden renderer before allocating its replacement", () => {
    const pool = new TerminalWebglPool(3);
    const a = {}, b = {}, c = {}, d = {};
    const disposed: string[] = [];
    pool.acquire(a, () => disposed.push("a"));
    pool.acquire(b, () => disposed.push("b"));
    pool.acquire(c, () => disposed.push("c"));
    pool.park(a);
    pool.park(b);
    pool.slot(a);
    pool.park(a);
    expect(pool.acquire(d, () => disposed.push("d"))).toBe(true);
    expect(disposed).toEqual(["b"]);
    pool.release(b);
    expect(disposed).toEqual(["b"]);
  });

  it("never evicts a visible pane or exceeds its budget", () => {
    const pool = new TerminalWebglPool(2);
    const a = {}, b = {}, c = {};
    const disposeA = vi.fn(), disposeB = vi.fn(), disposeC = vi.fn();
    pool.acquire(a, disposeA);
    pool.acquire(b, disposeB);
    expect(pool.acquire(c, disposeC)).toBe(false);
    expect(disposeA).not.toHaveBeenCalled();
    expect(disposeB).not.toHaveBeenCalled();
    pool.park(a);
    expect(pool.acquire(c, disposeC)).toBe(true);
    expect(disposeA).toHaveBeenCalledOnce();
    expect(disposeB).not.toHaveBeenCalled();
    pool.release(c);
    expect(disposeC).toHaveBeenCalledOnce();
  });

  it("protects every returning pane before allocating evicted siblings", () => {
    const pool = new TerminalWebglPool(3);
    const a = {}, b = {}, c = {}, d = {};
    const disposed: string[] = [];
    pool.acquire(a, () => disposed.push("a"));
    pool.acquire(b, () => disposed.push("b"));
    pool.park(a);
    pool.park(b);
    pool.acquire(c, () => disposed.push("c"));
    pool.acquire(d, () => disposed.push("d"));
    pool.park(c);
    pool.park(d);
    pool.slot(a);
    pool.slot(b);
    expect(pool.acquire(a, () => disposed.push("a again"))).toBe(true);
    expect(disposed).toEqual(["a", "c"]);
  });

  it("hands a freed slot to a visible pane left on the DOM renderer", () => {
    const pool = new TerminalWebglPool(2, task => { task(); });
    const a = {}, b = {}, c = {};
    const retries: string[] = [];
    const load = (): boolean => {
      if (pool.acquire(c, vi.fn())) return true;
      pool.wait(c, () => { retries.push("c"); load(); });
      return false;
    };
    pool.acquire(a, vi.fn());
    pool.acquire(b, vi.fn());
    expect(load()).toBe(false);
    expect(retries).toEqual([]);
    pool.release(a);
    expect(retries).toEqual(["c"]);
    // c now holds a's slot, so nothing is waiting for the next release.
    pool.release(b);
    expect(retries).toEqual(["c"]);
  });

  it("wakes a waiting pane when a visible renderer is parked", () => {
    const pool = new TerminalWebglPool(1, task => { task(); });
    const a = {}, b = {};
    const disposeA = vi.fn();
    const retryB = vi.fn(() => pool.acquire(b, vi.fn()));
    pool.acquire(a, disposeA);
    expect(pool.acquire(b, vi.fn())).toBe(false);
    pool.wait(b, retryB);
    pool.park(a);
    pool.park(a);
    expect(retryB).toHaveBeenCalledOnce();
    expect(disposeA).toHaveBeenCalledOnce();
  });

  it("stops waiting once the pane is hidden or gone, and wakes after the commit", () => {
    const tasks: (() => void)[] = [];
    const pool = new TerminalWebglPool(1, task => { tasks.push(task); });
    const a = {}, b = {}, c = {};
    const retryB = vi.fn(), retryC = vi.fn();
    pool.acquire(a, vi.fn());
    pool.wait(a, vi.fn());
    pool.wait(b, retryB);
    pool.wait(c, retryC);
    pool.park(b);
    pool.release(c);
    pool.release(a);
    expect(tasks).toHaveLength(0);
    pool.acquire(a, vi.fn());
    pool.wait(b, retryB);
    pool.release(a);
    pool.acquire(a, vi.fn());
    pool.release(a);
    // One wake per commit, run after it, whatever freed the capacity.
    expect(tasks).toHaveLength(1);
    expect(retryB).not.toHaveBeenCalled();
    tasks[0]();
    expect(retryB).toHaveBeenCalledOnce();
    expect(retryC).not.toHaveBeenCalled();
  });
});

describe("the default wake", () => {
  it("queues its microtask the way Chromium accepts, not as the pool's own method", async () => {
    const queue = globalThis.queueMicrotask;
    // Chromium's queueMicrotask throws "Illegal invocation" for any receiver
    // but the global object.
    vi.stubGlobal("queueMicrotask", function (this: unknown, task: () => void) {
      if (this !== undefined && this !== globalThis) throw new TypeError("Illegal invocation");
      queue(task);
    });
    try {
      const cache = new TerminalWebglPool(1);
      const a = {}, b = {};
      const retry = vi.fn();
      cache.acquire(a, vi.fn());
      expect(cache.acquire(b, vi.fn())).toBe(false);
      cache.wait(b, retry);
      expect(() => { cache.release(a); }).not.toThrow();
      await Promise.resolve();
      expect(retry).toHaveBeenCalledOnce();
    } finally {
      vi.unstubAllGlobals();
    }
  });
});

describe("terminal GPU renderers out of view", () => {
  const IDLE = TERMINAL_WEBGL_IDLE_RELEASE_MS;
  const pool = (time: ReturnType<typeof manualClock>) =>
    new TerminalWebglPool(12, task => { task(); }, time.clock);

  it("runs view work after the same idle window on its clock, unless cancelled", () => {
    const time = manualClock();
    const cache = pool(time);
    const ran = vi.fn();
    cache.later(ran);
    const cancel = cache.later(() => { throw new Error("cancelled work ran"); });
    cancel();
    time.advance(IDLE - 1);
    expect(ran).not.toHaveBeenCalled();
    time.advance(1);
    expect(ran).toHaveBeenCalledTimes(1);
    expect(time.pending()).toBe(0);
  });

  it("turns screen-reader mode off out of view and back on before the view shows", () => {
    const source = readFileSync(new URL("./components/TerminalView.tsx", import.meta.url), "utf8");
    expect(source).toContain("screenReaderMode: TERMINAL_SCREEN_READER_MODE && activeRef.current });");
    expect(source).toMatch(/if \(slotted && shown\) \{\s*cancelReaderIdle\?\.\(\);/u);
    expect(source).toContain(
      "if (terminal.options.screenReaderMode !== true) terminal.options.screenReaderMode = true;");
    // Never for a focused pane, nor one still painting under a dialog.
    expect(source).toContain("if (host.contains(document.activeElement) || webglHooks.onScreen()) {");
    for (const edge of ["slot()", "park()", "show(active)"]) {
      const start = source.indexOf(`        ${edge} {`);
      expect(source.slice(start, source.indexOf("},", start))).toContain("syncReader();");
    }
  });

  it("gives back a parked renderer once, after its idle window, and keeps it if slotted first", () => {
    const time = manualClock();
    const cache = pool(time);
    const a = {}, b = {};
    const disposeA = vi.fn(), disposeB = vi.fn();
    cache.acquire(a, disposeA);
    cache.acquire(b, disposeB);
    cache.park(a);
    cache.park(b);
    time.advance(IDLE - 1);
    cache.slot(b);
    expect(disposeA).not.toHaveBeenCalled();
    time.advance(1);
    expect(disposeA).toHaveBeenCalledOnce();
    expect(disposeB).not.toHaveBeenCalled();
    time.advance(10 * IDLE);
    expect(disposeA).toHaveBeenCalledOnce();
    expect(disposeB).not.toHaveBeenCalled();
    // Released means gone: the next visit acquires a fresh renderer.
    expect(cache.acquire(a, vi.fn())).toBe(true);
    expect(disposeA).toHaveBeenCalledOnce();
  });

  it("re-arms for the next oldest parked renderer and never sweeps a visible one", () => {
    const time = manualClock();
    const cache = pool(time);
    const disposed: string[] = [];
    const a = {}, b = {}, c = {};
    cache.acquire(a, () => disposed.push("a"));
    cache.acquire(b, () => disposed.push("b"));
    cache.acquire(c, () => disposed.push("c"));
    cache.park(a);
    time.advance(20_000);
    cache.park(b);
    time.advance(IDLE - 20_000);
    expect(disposed).toEqual(["a"]);
    expect(time.pending()).toBe(1);
    time.advance(20_000);
    expect(disposed).toEqual(["a", "b"]);
    expect(time.pending()).toBe(0);
    time.advance(10 * IDLE);
    expect(disposed).toEqual(["a", "b"]);
  });

  it("keeps a parked renderer that is still painting, and gives it back once it is not", () => {
    const time = manualClock();
    const cache = pool(time);
    let painting = true;
    const dispose = vi.fn();
    const a = {}, b = {};
    cache.acquire(a, dispose, { onScreen: () => painting });
    cache.park(a);
    time.advance(IDLE);
    expect(dispose).not.toHaveBeenCalled();
    // At the cache's limit a painting pane is no eviction candidate either.
    const full = new TerminalWebglPool(1, task => { task(); }, time.clock);
    full.acquire(b, vi.fn(), { onScreen: () => true });
    full.park(b);
    expect(full.acquire({}, vi.fn())).toBe(false);
    painting = false;
    time.advance(IDLE);
    expect(dispose).toHaveBeenCalledOnce();
  });

  it("gives back every renderer after the window stays hidden, and reloads the visible ones on show", () => {
    const time = manualClock();
    const tasks: (() => void)[] = [];
    // An idle window longer than the hidden one, so only hiding releases here.
    const cache = new TerminalWebglPool(12, task => { tasks.push(task); }, time.clock,
      10 * TERMINAL_WEBGL_HIDDEN_RELEASE_MS);
    const shown = {}, parked = {};
    const disposeShown = vi.fn(), disposeParked = vi.fn();
    const restoreShown = vi.fn(), restoreParked = vi.fn();
    cache.acquire(shown, disposeShown, { restore: restoreShown });
    cache.acquire(parked, disposeParked, { restore: restoreParked });
    cache.park(parked);
    tasks.length = 0;
    // Hidden briefly: nothing happens.
    cache.setDocumentHidden(true);
    time.advance(TERMINAL_WEBGL_HIDDEN_RELEASE_MS - 1);
    cache.setDocumentHidden(false);
    time.advance(TERMINAL_WEBGL_HIDDEN_RELEASE_MS);
    expect(disposeShown).not.toHaveBeenCalled();
    expect(disposeParked).not.toHaveBeenCalled();
    cache.setDocumentHidden(true);
    time.advance(TERMINAL_WEBGL_HIDDEN_RELEASE_MS);
    expect(disposeShown).toHaveBeenCalledOnce();
    expect(disposeParked).toHaveBeenCalledOnce();
    // While hidden no renderer is handed out; the pane waits for the show.
    const late = {};
    expect(cache.acquire(late, vi.fn())).toBe(false);
    const restoreLate = vi.fn();
    cache.wait(late, restoreLate);
    expect(tasks).toHaveLength(0);
    cache.setDocumentHidden(false);
    expect(tasks).toHaveLength(1);
    tasks[0]();
    expect(restoreShown).toHaveBeenCalledOnce();
    expect(restoreLate).toHaveBeenCalledOnce();
    expect(restoreParked).not.toHaveBeenCalled();
    expect(cache.acquire(shown, vi.fn())).toBe(true);
  });
});

describe("TerminalView GPU renderer recovery wiring", () => {
  // The DOM fallback draws the same font visibly lighter. A lost context
  // reloads with backoff while slotted (the workspace-switch fixture loses one
  // for real); a pane refused a full cache waits for a freed slot.
  const source = readFileSync(new URL("./components/TerminalView.tsx", import.meta.url), "utf8");

  it("brings a visible pane back to WebGL instead of leaving its text thin", () => {
    expect(source).toMatch(/terminalWebglPool\.release\(webglOwner\);\s*scheduleReload\(\);/u);
    expect(source).toContain("terminalWebglPool.wait(webglOwner, () => { ensureWebgl(); });");
    expect(source).toContain("if (recovering) scheduleReload();");
    expect(source).toMatch(/lostInSlot = false;\s*recovery\.reset\(\);/u);
    expect(source).toMatch(
      /clearTimeout\(reloadTimer\);\s*terminalWebglPool\.release\(webglOwner\);\s*terminal\.dispose\(\);/u);
  });
});

describe("terminal GPU renderer recovery", () => {
  it("backs off, gives up, and starts over once a renderer has stayed up", () => {
    const recovery = new TerminalWebglRecovery();
    let now = 0;
    const delays = TERMINAL_WEBGL_RETRY_MS.map(() => {
      const delay = recovery.lost(now);
      now += (delay ?? 0) + 3_000;
      return delay;
    });
    expect(delays).toEqual(TERMINAL_WEBGL_RETRY_MS);
    expect(recovery.lost(now)).toBeNull();
    expect(recovery.lost(now + TERMINAL_WEBGL_STABLE_MS)).toBe(TERMINAL_WEBGL_RETRY_MS[0]);
    expect(recovery.lost(now + TERMINAL_WEBGL_STABLE_MS + 1)).toBe(TERMINAL_WEBGL_RETRY_MS[1]);
    recovery.reset();
    expect(recovery.lost(now + TERMINAL_WEBGL_STABLE_MS + 2)).toBe(TERMINAL_WEBGL_RETRY_MS[0]);
  });
});
