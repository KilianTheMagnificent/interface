// Chromium permits 16 WebGL contexts per renderer. Leave room for transient
// contexts and other surfaces; terminal buffers live independently of this cache.
export const TERMINAL_WEBGL_LIMIT = 12;

// xterm's addon has already waited 3 s for the browser to restore a lost
// context before it reports the loss, so the first reload can follow soon.
export const TERMINAL_WEBGL_RETRY_MS: readonly number[] = [1_000, 5_000, 15_000];
// A renderer that stays up this long has recovered: a later loss (the next
// sleep, say) starts a fresh retry budget instead of spending the old one.
export const TERMINAL_WEBGL_STABLE_MS = 60_000;

// Each renderer holds a pane-sized GL drawing buffer and swap chain, a second
// pane-sized link canvas and its own glyph atlas textures. A pane out of view
// (another workspace, a background tab, a hidden surface) keeps them this long,
// so a quick switch back stays warm, and then gives them back. Coming back
// later costs a cold renderer, about 10-14 ms per pane.
export const TERMINAL_WEBGL_IDLE_RELEASE_MS = 60_000;
// A hidden window (closed to the Dock and still running) gives back every
// renderer, visible panes included, after this long; they reload on show.
export const TERMINAL_WEBGL_HIDDEN_RELEASE_MS = 120_000;

/** The pool's time source; tests drive it by hand. */
export interface TerminalWebglClock {
  now(): number;
  setTimeout(task: () => void, ms: number): unknown;
  clearTimeout(timer: unknown): void;
}

const SYSTEM_CLOCK: TerminalWebglClock = {
  now: () => performance.now(),
  setTimeout: (task, ms) => globalThis.setTimeout(task, ms),
  clearTimeout: timer => { globalThis.clearTimeout(timer as ReturnType<typeof setTimeout>); },
};

export interface TerminalWebglOwnerHooks {
  /** Reloads a renderer released while its pane was on screen. */
  readonly restore?: () => void;
  /** Whether the pane still paints, e.g. under a dialog that made it inactive. */
  readonly onScreen?: () => boolean;
}

interface Entry {
  parked: boolean;
  parkedAt: number;
  readonly dispose: () => void;
  readonly hooks: TerminalWebglOwnerHooks;
}

/** Keep recent renderers warm, evicting only the least recently used hidden one. */
export class TerminalWebglPool {
  private readonly entries = new Map<object, Entry>();
  // Visible panes painting with the DOM renderer because every slot is held by
  // another visible pane. That renderer draws text visibly thinner, so a freed
  // slot goes to them without waiting for their next attach or resize.
  private readonly waiting = new Map<object, () => void>();
  private waking = false;
  private idleTimer: unknown = null;
  private hiddenTimer: unknown = null;
  // Every renderer was given back while the window stayed hidden. Panes that
  // were on screen wait in `waiting` and reload when it is shown again.
  private asleep = false;

  // Not `queueMicrotask` itself: called as this pool's method, Chromium
  // refuses it ("Illegal invocation"), and every wake threw.
  constructor(private readonly limit = TERMINAL_WEBGL_LIMIT,
    private readonly schedule: (task: () => void) => void = task => { queueMicrotask(task); },
    private clock: TerminalWebglClock = SYSTEM_CLOCK,
    private readonly idleMs = TERMINAL_WEBGL_IDLE_RELEASE_MS,
    private readonly hiddenMs = TERMINAL_WEBGL_HIDDEN_RELEASE_MS) {}

  acquire(owner: object, dispose: () => void, hooks: TerminalWebglOwnerHooks = {}): boolean {
    if (this.entries.has(owner)) { this.slot(owner); return true; }
    // A hidden window takes no new renderer; `wait` reloads the pane on show.
    if (this.asleep) return false;
    if (this.entries.size >= this.limit) {
      const oldest = [...this.entries].find(([, entry]) => entry.parked && entry.hooks.onScreen?.() !== true);
      // Visible panes keep their renderer. Extra panes use xterm's DOM renderer;
      // `wait` hands them the next slot released or parked.
      if (oldest === undefined) return false;
      this.release(oldest[0]);
    }
    this.waiting.delete(owner);
    this.entries.set(owner, { parked: false, parkedAt: 0, dispose, hooks });
    return true;
  }

  /** A visible pane refused by `acquire` retries once capacity frees up. */
  wait(owner: object, retry: () => void): void {
    if (!this.entries.has(owner)) this.waiting.set(owner, retry);
  }

  slot(owner: object): void {
    const entry = this.entries.get(owner);
    if (entry === undefined) return;
    entry.parked = false;
    this.entries.delete(owner);
    this.entries.set(owner, entry);
  }

  park(owner: object): void {
    this.waiting.delete(owner);
    const entry = this.entries.get(owner);
    if (entry === undefined || entry.parked) return;
    entry.parked = true;
    entry.parkedAt = this.clock.now();
    this.armIdle(entry.parkedAt + this.idleMs);
    this.wake();
  }

  release(owner: object): void {
    this.waiting.delete(owner);
    const entry = this.entries.get(owner);
    if (entry === undefined) return;
    this.entries.delete(owner);
    entry.dispose();
    this.wake();
  }

  /** `document.visibilityState`'s changes; see TERMINAL_WEBGL_HIDDEN_RELEASE_MS. */
  setDocumentHidden(hidden: boolean): void {
    if (hidden) {
      if (this.hiddenTimer === null && !this.asleep) {
        this.hiddenTimer = this.clock.setTimeout(() => { this.sleep(); }, this.hiddenMs);
      }
      return;
    }
    if (this.hiddenTimer !== null) this.clock.clearTimeout(this.hiddenTimer);
    this.hiddenTimer = null;
    if (!this.asleep) return;
    this.asleep = false;
    // A microtask after the visibility change, so before the first frame shown.
    this.wake();
  }

  /**
   * Runs `task` once, `ms` (the idle window by default) from now on the pool's
   * clock, which fixtures drive by hand; answers its cancel. For view state
   * that follows the renderer's idle window, such as screen-reader mode.
   */
  later(task: () => void, ms = this.idleMs): () => void {
    const clock = this.clock;
    const timer = clock.setTimeout(task, ms);
    return () => { clock.clearTimeout(timer); };
  }

  /** Fixtures drive the shared pool's windows by hand; pending ones restart on `clock`. */
  useClock(clock: TerminalWebglClock = SYSTEM_CLOCK): void {
    const hidden = this.hiddenTimer !== null;
    if (this.idleTimer !== null) this.clock.clearTimeout(this.idleTimer);
    if (this.hiddenTimer !== null) this.clock.clearTimeout(this.hiddenTimer);
    this.idleTimer = null;
    this.hiddenTimer = null;
    this.clock = clock;
    const now = clock.now();
    for (const entry of this.entries.values()) {
      if (!entry.parked) continue;
      entry.parkedAt = now;
      this.armIdle(now + this.idleMs);
    }
    if (hidden) this.setDocumentHidden(true);
  }

  /** One timer for every parked renderer: it fires for the oldest. */
  private armIdle(due: number): void {
    if (this.idleTimer !== null) return;
    this.idleTimer = this.clock.setTimeout(() => {
      this.idleTimer = null;
      this.sweep();
    }, Math.max(0, due - this.clock.now()));
  }

  private sweep(): void {
    const now = this.clock.now();
    let next = Number.POSITIVE_INFINITY;
    for (const [owner, entry] of [...this.entries]) {
      if (!entry.parked) continue;
      if (now - entry.parkedAt >= this.idleMs) {
        // Inactive yet still painting (under a dialog, say): keep it for now.
        if (entry.hooks.onScreen?.() === true) entry.parkedAt = now;
        else { this.release(owner); continue; }
      }
      next = Math.min(next, entry.parkedAt + this.idleMs);
    }
    if (next !== Number.POSITIVE_INFINITY) this.armIdle(next);
  }

  private sleep(): void {
    this.hiddenTimer = null;
    this.asleep = true;
    for (const [owner, entry] of [...this.entries]) {
      this.release(owner);
      if (!entry.parked && entry.hooks.restore !== undefined) this.waiting.set(owner, entry.hooks.restore);
    }
  }

  /** After the current commit, let every waiting pane try the freed capacity. */
  private wake(): void {
    if (this.asleep || this.waking || this.waiting.size === 0) return;
    this.waking = true;
    this.schedule(() => {
      this.waking = false;
      const retries = [...this.waiting.values()];
      this.waiting.clear();
      for (const retry of retries) retry();
    });
  }
}

/** Bounded reloads of one pane's lost GPU renderer. */
export class TerminalWebglRecovery {
  private attempts = 0;
  private lastLoss = Number.NEGATIVE_INFINITY;

  /** The delay before the next reload, or null once this budget is spent. */
  lost(now: number): number | null {
    if (now - this.lastLoss >= TERMINAL_WEBGL_STABLE_MS) this.attempts = 0;
    this.lastLoss = now;
    return TERMINAL_WEBGL_RETRY_MS[this.attempts++] ?? null;
  }

  reset(): void {
    this.attempts = 0;
    this.lastLoss = Number.NEGATIVE_INFINITY;
  }
}

// One budget per renderer document, including independent native worker panes.
export const terminalWebglPool = new TerminalWebglPool();
if (typeof document !== "undefined") {
  const hidden = (): boolean => document.visibilityState === "hidden";
  document.addEventListener("visibilitychange", () => { terminalWebglPool.setDocumentHidden(hidden()); });
  terminalWebglPool.setDocumentHidden(hidden());
}
