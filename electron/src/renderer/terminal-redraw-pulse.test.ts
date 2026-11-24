import { afterEach, describe, expect, it, vi } from "vitest";
import {
  TerminalRedrawPulseQueue, type TerminalRedrawPulseSize,
} from "./terminal-redraw-pulse";
import { advanceTerminalReplayBoundary, type TerminalReplayBoundary } from "./terminal-output-state";

function deferred(): {
  readonly promise: Promise<void>;
  readonly resolve: () => void;
  readonly reject: (error: unknown) => void;
} {
  let resolve = (): void => undefined;
  let reject = (_error: unknown): void => undefined;
  const promise = new Promise<void>((accept, decline) => {
    resolve = accept;
    reject = decline;
  });
  return { promise, resolve, reject };
}

async function settle(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

const KICK = { columns: 120, rows: 39 };
const TARGET = { columns: 120, rows: 40 };
const queueFor = (onError: (error: unknown) => void): TerminalRedrawPulseQueue =>
  new TerminalRedrawPulseQueue(75, 250, onError);

afterEach(() => { vi.useRealTimers(); });

describe("TerminalRedrawPulseQueue", () => {
  it("waits through animation and a busy OpenTUI until its split resize query arrives", async () => {
    vi.useFakeTimers();
    const sent: TerminalRedrawPulseSize[] = [];
    const queue = new TerminalRedrawPulseQueue(250, 500, error => { throw error; });
    let boundary: TerminalReplayBoundary = "ground";
    const output = (text: string): void => {
      const bytes = new TextEncoder().encode(text);
      queue.noteOutput("opencode:1", bytes, boundary);
      boundary = advanceTerminalReplayBoundary(boundary, bytes);
    };
    queue.start("opencode:1", KICK, TARGET, async size => { sent.push(size); }, "opentui-resize");
    for (let frame = 0; frame < 8; frame++) {
      output("\x1b[?2026h\x1b[9;1H.\x1b[?2026l");
      await vi.advanceTimersByTimeAsync(100);
    }
    // Titles, control-string payloads, cancelled and other window queries
    // must not be mistaken for OpenTUI processing SIGWINCH.
    for (const text of ["\x1b]0;title\x1b[14t\x07", "\x1bP\x1b[14t\x1b\\",
      "\x1b[14\x18t", "\x1b[114t", "\x1b[14;t", "\x1b[18t"]) output(text);
    expect(sent).toEqual([KICK]);
    output("\x1b[1");
    output("4");
    expect(sent).toEqual([KICK]);
    output("t");
    await settle();
    expect(sent).toEqual([KICK, TARGET]);
    queue.dispose();
  });

  it("bounds OpenTUI recovery when a resize report never arrives", async () => {
    vi.useFakeTimers();
    const sent: TerminalRedrawPulseSize[] = [];
    const queue = queueFor(error => { throw error; });
    queue.start("opencode:1", KICK, TARGET, async size => { sent.push(size); }, "opentui-resize");
    await vi.advanceTimersByTimeAsync(2999);
    expect(sent).toEqual([KICK]);
    await vi.advanceTimersByTimeAsync(1);
    expect(sent).toEqual([KICK, TARGET]);
    queue.dispose();
  });

  it("never restores before the kick request resolves", async () => {
    vi.useFakeTimers();
    const kick = deferred();
    const sent: TerminalRedrawPulseSize[] = [];
    const queue = queueFor(error => { throw error; });
    const send = (size: TerminalRedrawPulseSize): Promise<void> => {
      sent.push(size);
      return sent.length === 1 ? kick.promise : Promise.resolve();
    };

    queue.start("terminal:1", KICK, TARGET, send);
    queue.noteOutput("terminal:1");
    await vi.advanceTimersByTimeAsync(250);
    expect(sent).toEqual([KICK]);

    kick.resolve();
    await settle();
    expect(sent).toEqual([KICK]);
    await vi.advanceTimersByTimeAsync(75);
    expect(sent).toEqual([KICK, TARGET]);
    queue.dispose();
  });

  it("requires both observed output and the 75ms minimum delay", async () => {
    vi.useFakeTimers();
    const sent: TerminalRedrawPulseSize[] = [];
    const queue = queueFor(error => { throw error; });
    const send = async (size: TerminalRedrawPulseSize): Promise<void> => { sent.push(size); };

    queue.start("terminal:1", KICK, TARGET, send);
    queue.noteOutput("terminal:1");
    await settle();
    await vi.advanceTimersByTimeAsync(74);
    expect(sent).toEqual([KICK]);

    await vi.advanceTimersByTimeAsync(1);
    expect(sent).toEqual([KICK, TARGET]);
    queue.dispose();
  });

  it("restores after the bounded 250ms fallback when no output arrives", async () => {
    vi.useFakeTimers();
    const sent: TerminalRedrawPulseSize[] = [];
    const queue = queueFor(error => { throw error; });
    const send = async (size: TerminalRedrawPulseSize): Promise<void> => { sent.push(size); };

    queue.start("terminal:1", KICK, TARGET, send);
    await settle();
    await vi.advanceTimersByTimeAsync(249);
    expect(sent).toEqual([KICK]);

    await vi.advanceTimersByTimeAsync(1);
    expect(sent).toEqual([KICK, TARGET]);
    queue.dispose();
  });

  it("does not accept output from another terminal generation", async () => {
    vi.useFakeTimers();
    const sent: TerminalRedrawPulseSize[] = [];
    const queue = queueFor(error => { throw error; });
    const send = async (size: TerminalRedrawPulseSize): Promise<void> => { sent.push(size); };

    queue.start("terminal:2", KICK, TARGET, send);
    await settle();
    queue.noteOutput("terminal:1");
    await vi.advanceTimersByTimeAsync(249);
    expect(sent).toEqual([KICK]);

    await vi.advanceTimersByTimeAsync(1);
    expect(sent).toEqual([KICK, TARGET]);
    queue.dispose();
  });

  it("restores the latest target and serializes changes during an in-flight restore", async () => {
    vi.useFakeTimers();
    const firstRestore = deferred();
    const sent: TerminalRedrawPulseSize[] = [];
    const queue = queueFor(error => { throw error; });
    const send = (size: TerminalRedrawPulseSize): Promise<void> => {
      sent.push(size);
      return sent.length === 2 ? firstRestore.promise : Promise.resolve();
    };
    const firstTarget = { columns: 100, rows: 30 };
    const secondTarget = { columns: 90, rows: 28 };
    const latestTarget = { columns: 80, rows: 24 };

    queue.start("terminal:1", KICK, TARGET, send);
    expect(queue.updateTarget("terminal:1", firstTarget)).toBe(true);
    queue.noteOutput("terminal:1");
    await vi.advanceTimersByTimeAsync(75);
    expect(sent).toEqual([KICK, firstTarget]);

    expect(queue.updateTarget("terminal:1", secondTarget)).toBe(true);
    expect(queue.updateTarget("terminal:1", latestTarget)).toBe(true);
    expect(sent).toEqual([KICK, firstTarget]);

    firstRestore.resolve();
    await settle();
    expect(sent).toEqual([KICK, firstTarget, latestTarget]);
    await settle();
    expect(queue.updateTarget("terminal:1", TARGET)).toBe(false);
    queue.dispose();
  });

  it("cancels pending restores and fences a replaced start", async () => {
    vi.useFakeTimers();
    const cancelledKick = deferred();
    const staleKick = deferred();
    const currentKick = deferred();
    const sent: TerminalRedrawPulseSize[] = [];
    const queue = queueFor(error => { throw error; });

    queue.start("cancelled", KICK, TARGET, size => {
      sent.push(size);
      return cancelledKick.promise;
    });
    queue.noteOutput("cancelled");
    queue.cancel("cancelled");
    cancelledKick.resolve();
    await vi.advanceTimersByTimeAsync(250);
    expect(sent).toEqual([KICK]);

    const staleTarget = { columns: 90, rows: 30 };
    const currentKickSize = { columns: 70, rows: 19 };
    const currentTarget = { columns: 70, rows: 20 };
    queue.start("replaced", KICK, staleTarget, size => {
      sent.push(size);
      return staleKick.promise;
    });
    queue.start("replaced", currentKickSize, currentTarget, size => {
      sent.push(size);
      return sent.length === 3 ? currentKick.promise : Promise.resolve();
    });
    staleKick.resolve();
    await settle();
    queue.noteOutput("replaced");
    await vi.advanceTimersByTimeAsync(75);
    expect(sent).toEqual([KICK, KICK, currentKickSize]);

    currentKick.resolve();
    await settle();
    await vi.advanceTimersByTimeAsync(75);
    expect(sent).toEqual([KICK, KICK, currentKickSize, currentTarget]);
    expect(sent).not.toContainEqual(staleTarget);
    queue.dispose();
  });

  it("reports a failed kick and never sends a restore", async () => {
    vi.useFakeTimers();
    const failure = new Error("kick failed");
    const errors: unknown[] = [];
    const sent: TerminalRedrawPulseSize[] = [];
    const queue = queueFor(error => { errors.push(error); });

    queue.start("terminal:1", KICK, TARGET, async size => {
      sent.push(size);
      throw failure;
    });
    await settle();
    await vi.runAllTimersAsync();

    expect(errors).toEqual([failure]);
    expect(sent).toEqual([KICK]);
    expect(queue.updateTarget("terminal:1", TARGET)).toBe(false);
    queue.dispose();
  });
});
