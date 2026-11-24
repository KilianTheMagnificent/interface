/** Metadata only: bounded completion tracking for one authenticated document. */
export type OutputIdentity = { terminalId: string; generation: bigint; sequence: bigint };
type Window = {
  identity: OutputIdentity;
  pending: Map<bigint, { bytes: number; complete: boolean }>;
  bytes: number;
};
export const TERMINAL_OUTPUT_WINDOW_BYTES = 256 * 1024;
export const TERMINAL_OUTPUT_WINDOW_FRAMES = 64;
/** One output stream: a terminal generation. */
export const outputKey = (frame: OutputIdentity): string => `${frame.terminalId}:${frame.generation}`;

export class TerminalOutputWindow {
  readonly #windows = new Map<string, Window>();
  #bytes = 0;

  get outstandingBytes(): number { return this.#bytes; }

  sent(frame: OutputIdentity, payloadBytes: number): void {
    const key = outputKey(frame);
    if (!this.#windows.has(key) && this.#windows.size >= 128) throw new Error("Too many terminal output windows");
    const window = this.#windows.get(key) ?? { identity: frame, pending: new Map(), bytes: 0 };
    const bytes = payloadBytes + 44;
    if (window.pending.size >= TERMINAL_OUTPUT_WINDOW_FRAMES
        || window.bytes + bytes > TERMINAL_OUTPUT_WINDOW_BYTES
        || window.pending.has(frame.sequence)) throw new Error("Terminal output exceeded its credit window");
    window.identity = { terminalId: frame.terminalId, generation: frame.generation, sequence: frame.sequence };
    window.pending.set(frame.sequence, { bytes, complete: false });
    window.bytes += bytes;
    this.#bytes += bytes;
    this.#windows.set(key, window);
  }

  complete(frame: OutputIdentity): bigint | undefined {
    const key = outputKey(frame);
    const window = this.#windows.get(key);
    const entry = window?.pending.get(frame.sequence);
    if (!window || !entry) return undefined; // stale, duplicate or unsent
    entry.complete = true;
    let watermark: bigint | undefined;
    for (const [sequence, pending] of window.pending) {
      if (!pending.complete) break;
      window.pending.delete(sequence);
      window.bytes -= pending.bytes;
      this.#bytes -= pending.bytes;
      watermark = sequence;
    }
    if (window.pending.size === 0) this.#windows.delete(key);
    return watermark;
  }

  /** Navigation discards this document's pending writes; never replay its acks. */
  abandon(): OutputIdentity[] {
    const watermarks = [...this.#windows.values()].map(window => window.identity);
    this.#windows.clear();
    this.#bytes = 0;
    return watermarks;
  }
}
