import { describe, expect, it } from "vitest";
import {
  RESTING_FLUSH_BYTES, RESTING_FLUSH_WRITES, RESTING_OUTPUT_MS, TerminalOutputPacer,
  type TerminalOutputTarget,
} from "./terminal-output-pacing";

function harness() {
  const state = { resting: false };
  const timers = new Map<number, { callback: () => void; ms: number }>();
  let next = 0;
  const pacer = new TerminalOutputPacer(() => state.resting, {
    setTimeout: (callback, ms) => { timers.set(++next, { callback, ms }); return next; },
    clearTimeout: id => { if (id !== undefined) timers.delete(id); },
  });
  const tick = (): void => {
    for (const [id, timer] of [...timers]) { timers.delete(id); timer.callback(); }
  };
  return { state, timers, pacer, tick };
}

function target(): TerminalOutputTarget & { written: string[] } {
  const written: string[] = [];
  return { written, write: (data, callback) => { written.push(String(data)); callback?.(); } };
}

describe("terminal output while Interface is not in front (2026-10-27, battery)", () => {
  it("applies every pane's output together about four times a second while resting", () => {
    const { state, timers, pacer, tick } = harness();
    const left = target();
    const right = target();
    const acknowledged: string[] = [];
    state.resting = true;
    pacer.write(left, "a", () => acknowledged.push("a"));
    pacer.write(right, "b", () => acknowledged.push("b"));
    pacer.write(left, "c", () => acknowledged.push("c"));
    expect(left.written).toEqual([]);
    expect(acknowledged).toEqual([]);
    // One shared timer, not one per pane or per write.
    expect([...timers.values()].map(timer => timer.ms)).toEqual([RESTING_OUTPUT_MS]);
    tick();
    expect(left.written).toEqual(["a", "c"]);
    expect(right.written).toEqual(["b"]);
    expect(acknowledged).toEqual(["a", "c", "b"]);
  });

  it("writes straight through in front, after anything still held", () => {
    const { state, timers, pacer } = harness();
    const pane = target();
    state.resting = true;
    pacer.write(pane, "held", () => {});
    state.resting = false;
    pacer.write(pane, "typed", () => {});
    expect(pane.written).toEqual(["held", "typed"]);
    expect(timers.size).toBe(0);
  });

  it("flushes a stream that piles up at once, inside the credit window", () => {
    const { state, pacer } = harness();
    const bulk = target();
    state.resting = true;
    const chunk = "x".repeat(RESTING_FLUSH_BYTES / 4);
    for (let index = 0; index < 4; index++) pacer.write(bulk, chunk, () => {});
    expect(bulk.written).toHaveLength(4);

    const chatty = target();
    for (let index = 0; index < RESTING_FLUSH_WRITES; index++) pacer.write(chatty, ".", () => {});
    expect(chatty.written).toHaveLength(RESTING_FLUSH_WRITES);
    // 64 frames and 256 KiB per stream (main/terminal-output-window.ts).
    expect(RESTING_FLUSH_WRITES).toBeLessThan(64);
    expect(RESTING_FLUSH_BYTES).toBeLessThan(256 * 1024);
  });

  it("acknowledges a disposed emulator's held writes without writing them", () => {
    const { state, pacer, tick } = harness();
    const pane = target();
    let acknowledged = 0;
    state.resting = true;
    pacer.write(pane, "late", () => { acknowledged++; });
    pacer.drop(pane);
    tick();
    expect(acknowledged).toBe(1);
    expect(pane.written).toEqual([]);
  });

  it("acknowledges a write xterm refused and still applies the rest", () => {
    const { state, pacer, tick } = harness();
    const acknowledged: string[] = [];
    const written: string[] = [];
    const failing: TerminalOutputTarget = {
      write: (data, callback) => {
        if (data === "bad") throw new Error("refused");
        written.push(String(data));
        callback?.();
      },
    };
    state.resting = true;
    pacer.write(failing, "bad", () => acknowledged.push("bad"));
    pacer.write(failing, "good", () => acknowledged.push("good"));
    expect(tick).toThrow("refused");
    expect(written).toEqual(["good"]);
    expect(acknowledged).toEqual(["bad", "good"]);
  });
});
