// Helpers shared by the isolated renderer DOM fixtures. Test code only.

export function assert(value: unknown, message: string): asserts value {
  if (!value) throw new Error(message);
}

export const noop = (): void => {};

/** One animation frame. */
export const frame = (): Promise<void> => new Promise(resolve => requestAnimationFrame(() => resolve()));

/** `count` animation frames; two let React commit and the browser paint. */
export async function frames(count = 2): Promise<void> {
  for (let index = 0; index < count; index++) await frame();
}

export const wait = (ms: number): Promise<void> => new Promise(resolve => setTimeout(resolve, ms));

export interface UntilOptions { timeoutMs?: number; pollMs?: number }

/** Polls until `ready` holds and fails with `message` at the deadline. Pass a
 * function to report state: it runs at the deadline, so it shows what the
 * fixture last saw rather than what it saw when the wait began. */
export async function until(ready: () => boolean, message: string | (() => string),
  { timeoutMs = 3000, pollMs = 20 }: UntilOptions = {}): Promise<void> {
  const deadline = performance.now() + timeoutMs;
  while (!ready()) {
    if (performance.now() >= deadline) throw new Error(typeof message === "function" ? message() : message);
    await wait(pollMs);
  }
}

/** `until` with this fixture's own deadline and polling interval. */
export const untilWithin = (options: UntilOptions) =>
  (ready: () => boolean, message: string | (() => string)): Promise<void> => until(ready, message, options);
