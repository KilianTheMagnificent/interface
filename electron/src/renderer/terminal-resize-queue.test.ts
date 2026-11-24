import { afterEach, describe, expect, it, vi } from "vitest";
import { TerminalResizeQueue, type TerminalResizeSize } from "./terminal-resize-queue";

function deferred(): {
  readonly promise: Promise<void>;
  readonly resolve: () => void;
} {
  let resolve = (): void => undefined;
  const promise = new Promise<void>(accept => { resolve = accept; });
  return { promise, resolve };
}

async function settle(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

afterEach(() => { vi.useRealTimers(); });

describe("TerminalResizeQueue", () => {
  it("settles a synchronous sender failure and allows the next resize", async () => {
    vi.useFakeTimers();
    const errors: unknown[] = [];
    const error = new Error("transport unavailable");
    const queue = new TerminalResizeQueue(75, failure => { errors.push(failure); });
    queue.schedule("terminal:1", { columns: 80, rows: 24 }, () => { throw error; });
    await vi.advanceTimersByTimeAsync(75);
    await queue.cancelAndWait("terminal:1");
    const send = vi.fn(async (): Promise<void> => undefined);
    queue.schedule("terminal:1", { columns: 120, rows: 40 }, send);
    await vi.advanceTimersByTimeAsync(75);
    expect(errors).toEqual([error]);
    expect(send).toHaveBeenCalledWith({ columns: 120, rows: 40 });
    queue.dispose();
  });
  it("debounces unsent geometry and sends only the newest dimensions", async () => {
    vi.useFakeTimers();
    const sent: TerminalResizeSize[] = [];
    const queue = new TerminalResizeQueue(75, error => { throw error; });
    const send = async (size: TerminalResizeSize): Promise<void> => { sent.push(size); };

    queue.schedule("terminal:1", { columns: 188, rows: 52 }, send);
    queue.schedule("terminal:1", { columns: 58, rows: 24 }, send);
    await vi.advanceTimersByTimeAsync(75);

    expect(sent).toEqual([{ columns: 58, rows: 24 }]);
    queue.dispose();
  });

  it("serializes an in-flight resize and follows it with only the latest size", async () => {
    vi.useFakeTimers();
    const first = deferred();
    const sent: TerminalResizeSize[] = [];
    const queue = new TerminalResizeQueue(75, error => { throw error; });
    const send = (size: TerminalResizeSize): Promise<void> => {
      sent.push(size);
      return sent.length === 1 ? first.promise : Promise.resolve();
    };

    queue.schedule("terminal:1", { columns: 188, rows: 52 }, send);
    await vi.advanceTimersByTimeAsync(75);
    queue.schedule("terminal:1", { columns: 90, rows: 30 }, send);
    queue.schedule("terminal:1", { columns: 58, rows: 24 }, send);
    await vi.advanceTimersByTimeAsync(75);
    expect(sent).toEqual([{ columns: 188, rows: 52 }]);

    first.resolve();
    await settle();
    expect(sent).toEqual([
      { columns: 188, rows: 52 },
      { columns: 58, rows: 24 },
    ]);
    queue.dispose();
  });

  it("cancels a resize that has not started when disposed", async () => {
    vi.useFakeTimers();
    const send = vi.fn(async (): Promise<void> => undefined);
    const queue = new TerminalResizeQueue(75, error => { throw error; });
    queue.schedule("terminal:1", { columns: 58, rows: 24 }, send);

    queue.dispose();
    await vi.runAllTimersAsync();

    expect(send).not.toHaveBeenCalled();
  });

  it("cancels one terminal without disturbing another", async () => {
    vi.useFakeTimers();
    const sent: string[] = [];
    const queue = new TerminalResizeQueue(75, error => { throw error; });
    queue.schedule("terminal:1", { columns: 80, rows: 24 }, async () => {
      sent.push("terminal:1");
    });
    queue.schedule("terminal:2", { columns: 120, rows: 40 }, async () => {
      sent.push("terminal:2");
    });

    queue.cancel("terminal:1");
    await vi.advanceTimersByTimeAsync(75);

    expect(sent).toEqual(["terminal:2"]);
    queue.dispose();
  });

  it("fences an in-flight resize from sending a queued follow-up", async () => {
    vi.useFakeTimers();
    const first = deferred();
    const sent: TerminalResizeSize[] = [];
    const queue = new TerminalResizeQueue(75, error => { throw error; });
    queue.schedule("terminal:1", { columns: 120, rows: 40 }, async size => {
      sent.push(size);
      await first.promise;
    });
    await vi.advanceTimersByTimeAsync(75);
    queue.schedule("terminal:1", { columns: 80, rows: 24 }, async size => {
      sent.push(size);
    });

    queue.cancel("terminal:1");
    first.resolve();
    await settle();

    expect(sent).toEqual([{ columns: 120, rows: 40 }]);
    queue.dispose();
  });

  it("waits for an already-dispatched resize while canceling its follow-up", async () => {
    vi.useFakeTimers();
    const first = deferred();
    const queue = new TerminalResizeQueue(75, error => { throw error; });
    queue.schedule("terminal:1", { columns: 120, rows: 40 }, () => first.promise);
    await vi.advanceTimersByTimeAsync(75);
    queue.schedule("terminal:1", { columns: 80, rows: 24 }, async () => undefined);

    let idle = false;
    const waiting = queue.cancelAndWait("terminal:1").then(() => { idle = true; });
    await settle();
    expect(idle).toBe(false);

    first.resolve();
    await waiting;
    expect(idle).toBe(true);
    await vi.runAllTimersAsync();
    queue.dispose();
  });
});
