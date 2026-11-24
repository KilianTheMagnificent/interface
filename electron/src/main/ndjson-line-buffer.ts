/** Byte-bounded framing. Each fragment is searched once; growth copies amortize
 * to linear work. UTF-8 is decoded only after an entire line has arrived. */
export class NdjsonLineBuffer {
  #buffer: Buffer = Buffer.alloc(0);
  #length = 0;

  constructor(readonly maxBytes: number) {}

  clear(): void {
    this.#buffer = Buffer.alloc(0);
    this.#length = 0;
  }

  /** False means an oversized line. A false receiver result stops this chunk. */
  push(chunk: Buffer, accept: (line: string) => boolean): boolean {
    let start = 0;
    while (start < chunk.length) {
      const newline = chunk.indexOf(10, start);
      const end = newline < 0 ? chunk.length : newline;
      const length = this.#length + end - start;
      if (length > this.maxBytes) { this.clear(); return false; }
      let line: string | undefined;
      if (this.#length === 0 && newline >= 0) {
        line = chunk.toString("utf8", start, end);
      } else {
        if (length > this.#buffer.length) {
          const capacity = Math.min(this.maxBytes, Math.max(4096, length, this.#buffer.length * 2));
          const next = Buffer.allocUnsafe(capacity);
          this.#buffer.copy(next, 0, 0, this.#length);
          this.#buffer = next;
        }
        chunk.copy(this.#buffer, this.#length, start, end);
        this.#length = length;
        if (newline >= 0) line = this.#buffer.toString("utf8", 0, length);
      }
      if (newline < 0) return true;
      this.#length = 0;
      // A single large response must not pin its allocation for the host lifetime.
      if (this.#buffer.length > 64 * 1024) this.#buffer = Buffer.alloc(0);
      if (!accept(line!)) return true;
      start = newline + 1;
    }
    return true;
  }
}
