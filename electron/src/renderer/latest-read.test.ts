import { describe, expect, it, vi } from "vitest";
import { LatestRead } from "./latest-read";

function pending<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const flush = async (): Promise<void> => {
  for (let i = 0; i < 10; i++) await Promise.resolve();
};

describe("latest read lane", () => {
  it("a hundred selections run only the in-flight read and the newest pending read", async () => {
    const lane = new LatestRead<number>();
    const first = pending<number>();
    const accept = vi.fn();
    const reject = vi.fn();
    const read = vi.fn((value: number) => Promise.resolve(value));
    lane.request(() => first.promise, accept, reject);
    await flush();
    for (let i = 1; i <= 100; i++) lane.request(() => read(i), accept, reject);
    expect(read).not.toHaveBeenCalled();
    first.resolve(0);
    await flush();
    expect(read).toHaveBeenCalledExactlyOnceWith(100);
    expect(accept).toHaveBeenCalledExactlyOnceWith(100);
    expect(reject).not.toHaveBeenCalled();
  });

  it("closing drops pending work and stale results, and reopening waits for the existing read", async () => {
    const lane = new LatestRead<string>();
    const first = pending<string>();
    const accept = vi.fn();
    const reject = vi.fn();
    const skipped = vi.fn(() => Promise.resolve("skipped"));
    const latest = vi.fn(() => Promise.resolve("reopened"));
    lane.request(() => first.promise, accept, reject);
    await flush();
    lane.request(skipped, accept, reject);
    lane.clear();
    lane.request(latest, accept, reject);
    expect(latest).not.toHaveBeenCalled();
    first.reject(new Error("late"));
    await flush();
    expect(skipped).not.toHaveBeenCalled();
    expect(reject).not.toHaveBeenCalled();
    expect(accept).toHaveBeenCalledExactlyOnceWith("reopened");
  });

  it("cleanup before the first microtask prevents the read and cleanup during it prevents publication", async () => {
    const lane = new LatestRead<string>();
    const read = vi.fn(() => Promise.resolve("unused"));
    const accept = vi.fn();
    const reject = vi.fn();
    lane.request(read, accept, reject);
    lane.clear();
    await flush();
    expect(read).not.toHaveBeenCalled();
    const running = pending<string>();
    lane.request(() => running.promise, accept, reject);
    await flush();
    lane.clear();
    running.resolve("late");
    await flush();
    expect(accept).not.toHaveBeenCalled();
  });

  it("reports current failures and accepts later reads after synchronous throws", async () => {
    const lane = new LatestRead<number>();
    const failure = new Error("offline");
    const accept = vi.fn();
    const reject = vi.fn();
    lane.request(() => { throw failure; }, accept, reject);
    await flush();
    expect(reject).toHaveBeenCalledExactlyOnceWith(failure);
    lane.request(() => Promise.resolve(2), accept, reject);
    await flush();
    expect(accept).toHaveBeenCalledExactlyOnceWith(2);
  });
});
