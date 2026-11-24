/**
 * A read lane with one operation in flight and one replaceable next request.
 * Only the newest request may publish. Clearing the lane drops pending work
 * and late results; it does not pretend to cancel an IPC operation already sent.
 * Read-only work only: provider sends and mutations must never be coalesced.
 */
export class LatestRead<T> {
  private generation = 0;
  private running = false;
  private pending: { generation: number; read: () => Promise<T>;
    accept: (value: T) => void; reject: (error: unknown) => void } | null = null;

  request(read: () => Promise<T>, accept: (value: T) => void,
      reject: (error: unknown) => void): void {
    this.pending = { generation: ++this.generation, read, accept, reject };
    this.start();
  }

  clear(): void {
    this.generation += 1;
    this.pending = null;
  }

  private start(): void {
    if (this.running || this.pending === null) return;
    const job = this.pending;
    this.pending = null;
    this.running = true;
    void Promise.resolve().then(async () => {
      // Cleanup or a newer selection may have arrived before the first microtask.
      if (job.generation !== this.generation) return;
      try {
        const value = await job.read();
        if (job.generation === this.generation) job.accept(value);
      } catch (error: unknown) {
        if (job.generation === this.generation) job.reject(error);
      }
    }).finally(() => {
      this.running = false;
      this.start();
    });
  }
}
