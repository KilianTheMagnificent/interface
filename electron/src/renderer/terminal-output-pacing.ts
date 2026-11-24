/**
 * Terminal output while Interface is not the app in front (2026-10-27, battery).
 *
 * A shown window keeps painting behind other apps (main/window-visibility.ts),
 * so every chunk a working agent writes — a spinner frame, a token count —
 * made Chromium draw the whole window again: a 10 Hz spinner cost about twenty
 * frames a second, and each working pane added its own. While the root says
 * `windowActive` is "false" (window-activity.ts), writes wait and every pane
 * applies them together about four times a second. A stream that piles up
 * flushes at once, well inside the transport's credit window
 * (main/terminal-output-window.ts), so a background build keeps its pace. In
 * front, writes go straight to xterm as before; the first one flushes anything
 * still waiting, so a stream's order never changes.
 */
export const RESTING_OUTPUT_MS = 250;
export const RESTING_FLUSH_BYTES = 64 * 1024;
export const RESTING_FLUSH_WRITES = 32;

/** The part of an xterm the pacer writes into. */
export interface TerminalOutputTarget {
  write(data: string | Uint8Array, callback?: () => void): void;
}

interface PacingClock {
  setTimeout(callback: () => void, ms: number): number;
  clearTimeout(id: number | undefined): void;
}

interface Waiting {
  readonly writes: { readonly data: string | Uint8Array; readonly done: () => void }[];
  bytes: number;
}

export class TerminalOutputPacer {
  private readonly waiting = new Map<TerminalOutputTarget, Waiting>();
  private timer: number | undefined;

  constructor(private readonly resting: () => boolean, private readonly clock: PacingClock) {}

  /** Write now, or hold the write for the next shared flush while resting. */
  write(target: TerminalOutputTarget, data: string | Uint8Array, done: () => void): void {
    if (!this.resting()) {
      this.flush();
      target.write(data, done);
      return;
    }
    let queue = this.waiting.get(target);
    if (queue === undefined) {
      queue = { writes: [], bytes: 0 };
      this.waiting.set(target, queue);
    }
    queue.writes.push({ data, done });
    queue.bytes += data.length;
    if (queue.bytes >= RESTING_FLUSH_BYTES || queue.writes.length >= RESTING_FLUSH_WRITES) {
      this.release(target, queue);
      return;
    }
    this.timer ??= this.clock.setTimeout(() => {
      this.timer = undefined;
      this.flush();
    }, RESTING_OUTPUT_MS);
  }

  /** Apply every held write, in each target's order. */
  flush(): void {
    this.clock.clearTimeout(this.timer);
    this.timer = undefined;
    for (const [target, queue] of [...this.waiting]) this.release(target, queue);
  }

  /** A disposed emulator's held writes are acknowledged, never written. */
  drop(target: TerminalOutputTarget): void {
    const queue = this.waiting.get(target);
    if (queue === undefined) return;
    this.waiting.delete(target);
    for (const write of queue.writes) write.done();
  }

  private release(target: TerminalOutputTarget, queue: Waiting): void {
    this.waiting.delete(target);
    let failure: unknown;
    for (const write of queue.writes) {
      try { target.write(write.data, write.done); }
      catch (error) { write.done(); failure ??= error; }
    }
    if (failure !== undefined) throw failure;
  }
}

let shared: TerminalOutputPacer | undefined;

/**
 * The document's one pacer, so every pane flushes in the same frame. A window
 * coming back to the front flushes at once rather than on the next tick.
 */
export function terminalOutputPacer(win: Window = window): TerminalOutputPacer {
  if (shared !== undefined) return shared;
  const root = win.document.documentElement;
  const pacer = new TerminalOutputPacer(() => root.dataset.windowActive === "false", {
    setTimeout: (callback, ms) => win.setTimeout(callback, ms),
    clearTimeout: id => { win.clearTimeout(id); },
  });
  const flush = (): void => { pacer.flush(); };
  win.addEventListener("focus", flush);
  win.document.addEventListener("visibilitychange", flush);
  shared = pacer;
  return pacer;
}
