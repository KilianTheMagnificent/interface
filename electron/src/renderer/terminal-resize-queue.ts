export interface TerminalResizeSize {
  readonly columns: number;
  readonly rows: number;
}

type ResizeSender = (size: TerminalResizeSize) => Promise<void>;

interface PendingResize {
  size: TerminalResizeSize;
  send: ResizeSender;
  timer: ReturnType<typeof globalThis.setTimeout> | null;
  inFlight: boolean;
  idle: Promise<void> | null;
  resolveIdle: (() => void) | null;
}

function sameSize(left: TerminalResizeSize, right: TerminalResizeSize): boolean {
  return left.columns === right.columns && left.rows === right.rows;
}

/**
 * Debounces resize noise while keeping at most one host request in flight for
 * each terminal generation. If geometry changes during a request, only the
 * newest dimensions are sent after that request settles.
 */
export class TerminalResizeQueue {
  private readonly entries = new Map<string, PendingResize>();

  constructor(
    private readonly delayMs: number,
    private readonly onError: (error: unknown) => void,
  ) {}

  schedule(key: string, size: TerminalResizeSize, send: ResizeSender): void {
    let entry = this.entries.get(key);
    if (entry === undefined) {
      entry = {
        size, send, timer: null, inFlight: false, idle: null, resolveIdle: null,
      };
      this.entries.set(key, entry);
    } else {
      entry.size = size;
      entry.send = send;
      if (entry.timer !== null) globalThis.clearTimeout(entry.timer);
    }
    const scheduled = entry;
    scheduled.timer = globalThis.setTimeout(() => {
      scheduled.timer = null;
      this.flush(key, scheduled);
    }, this.delayMs);
  }

  /**
   * Drop an unsent resize and fence any in-flight request from scheduling a
   * follow-up. The host request itself cannot be revoked once dispatched.
   */
  cancel(key: string): void {
    const entry = this.entries.get(key);
    if (entry === undefined) return;
    if (entry.timer !== null) globalThis.clearTimeout(entry.timer);
    this.entries.delete(key);
  }

  /** Cancel queued work and wait only for a request already inside the host. */
  cancelAndWait(key: string): Promise<void> {
    const entry = this.entries.get(key);
    if (entry === undefined) return Promise.resolve();
    const idle = entry.idle ?? Promise.resolve();
    this.cancel(key);
    return idle;
  }

  dispose(): void {
    for (const entry of this.entries.values()) {
      if (entry.timer !== null) globalThis.clearTimeout(entry.timer);
    }
    this.entries.clear();
  }

  private flush(key: string, entry: PendingResize): void {
    if (entry.inFlight || this.entries.get(key) !== entry) return;
    entry.inFlight = true;
    entry.idle = new Promise<void>(resolve => { entry.resolveIdle = resolve; });
    const sentSize = entry.size;
    const send = entry.send;
    let request: Promise<void>;
    try {
      request = send(sentSize);
    } catch (error) {
      request = Promise.reject(error);
    }
    void request.catch(this.onError).finally(() => {
      entry.inFlight = false;
      entry.resolveIdle?.();
      entry.resolveIdle = null;
      entry.idle = null;
      if (this.entries.get(key) !== entry) return;
      if (sameSize(entry.size, sentSize)) {
        this.entries.delete(key);
        return;
      }
      this.flush(key, entry);
    });
  }
}
