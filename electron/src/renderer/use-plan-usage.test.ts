import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { EngineUsage } from "../shared/engine";
import {
  PlanUsageReads, USAGE_TURN_FLOOR_MS, usageCatchUp, usageClockActive,
} from "./use-plan-usage";

/** A scripted readUsage: every call is recorded and answered by the test. */
function harness(window = { hidden: false }) {
  const calls: { at: number; resolve: (rows: EngineUsage[]) => void; reject: (error: unknown) => void }[] = [];
  const landed: number[] = [];
  const failed: unknown[] = [];
  const reads = new PlanUsageReads({
    hidden: () => window.hidden,
    read: () => new Promise<EngineUsage[]>((resolve, reject) => { calls.push({ at: Date.now(), resolve, reject }); }),
    loading: () => {},
    landed: (_rows, readAt) => { landed.push(readAt); },
    failed: error => { failed.push(error); },
  });
  return { reads, calls, landed, failed };
}

/** Settle the latest read and let its promise chain run. */
async function answer(calls: ReturnType<typeof harness>["calls"], outcome: "ok" | "fail" = "ok"): Promise<void> {
  const call = calls.at(-1)!;
  if (outcome === "ok") call.resolve([]); else call.reject(new Error("usage unavailable"));
  await vi.advanceTimersByTimeAsync(0);
}

describe("plan usage reads", () => {
  beforeEach(() => { vi.useFakeTimers(); vi.setSystemTime(1_000_000); });
  afterEach(() => { vi.useRealTimers(); });

  it("reads once, shortly after a burst of finished turns, when nothing was read before", async () => {
    const { reads, calls } = harness();
    reads.turnDone();
    await vi.advanceTimersByTimeAsync(300);
    reads.turnDone();
    reads.turnDone();
    await vi.advanceTimersByTimeAsync(799);
    expect(calls).toHaveLength(0);
    await vi.advanceTimersByTimeAsync(1);
    expect(calls).toHaveLength(1);
  });

  it("holds a finished turn's read until a minute after the previous read began, then still reads it", async () => {
    const { reads, calls } = harness();
    reads.refresh();
    const first = Date.now();
    await answer(calls);
    await vi.advanceTimersByTimeAsync(10_000);
    reads.turnDone();
    reads.turnDone();
    await vi.advanceTimersByTimeAsync(first + USAGE_TURN_FLOOR_MS - Date.now() - 1);
    expect(calls).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(calls).toHaveLength(2);
    expect(calls[1].at - calls[0].at).toBe(USAGE_TURN_FLOOR_MS);
    await answer(calls);
    // A turn long after the floor reads at the ordinary short pause.
    await vi.advanceTimersByTimeAsync(2 * USAGE_TURN_FLOOR_MS);
    reads.turnDone();
    await vi.advanceTimersByTimeAsync(800);
    expect(calls).toHaveLength(3);
  });

  it("reads at once on Refresh inside the floor, and that read pays an owed turn", async () => {
    const { reads, calls } = harness();
    reads.refresh();
    await answer(calls);
    await vi.advanceTimersByTimeAsync(5_000);
    reads.turnDone();
    await vi.advanceTimersByTimeAsync(5_000);
    reads.refresh();
    expect(calls).toHaveLength(2);
    expect(calls[1].at - calls[0].at).toBe(10_000);
    await answer(calls);
    // The turn ended before the Refresh began, so its answer already counts it.
    await vi.advanceTimersByTimeAsync(3 * USAGE_TURN_FLOOR_MS);
    expect(calls).toHaveLength(2);
    // A second Refresh straight after is still immediate.
    reads.refresh();
    expect(calls).toHaveLength(3);
  });

  it("floors turns after a failed read too", async () => {
    const { reads, calls, failed } = harness();
    reads.refresh();
    await answer(calls, "fail");
    expect(failed).toHaveLength(1);
    reads.turnDone();
    await vi.advanceTimersByTimeAsync(USAGE_TURN_FLOOR_MS - 1);
    expect(calls).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(calls).toHaveLength(2);
  });

  it("re-reads a turn that ended while an earlier read was still in flight", async () => {
    const { reads, calls, landed } = harness();
    reads.refresh();
    await vi.advanceTimersByTimeAsync(500);
    reads.turnDone();
    await vi.advanceTimersByTimeAsync(1_500);
    await answer(calls);
    expect(landed).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(USAGE_TURN_FLOOR_MS);
    expect(calls).toHaveLength(2);
    expect(calls[1].at - calls[0].at).toBe(USAGE_TURN_FLOOR_MS);
  });

  it("queues a request behind the read in flight instead of spawning on top", async () => {
    const { reads, calls } = harness();
    reads.refresh();
    reads.refresh();
    reads.refresh();
    expect(calls).toHaveLength(1);
    await answer(calls);
    expect(calls).toHaveLength(2);
    await answer(calls);
    expect(calls).toHaveLength(2);
  });

  it("forgets an owed read on dispose", async () => {
    const { reads, calls } = harness();
    reads.turnDone();
    reads.dispose();
    await vi.advanceTimersByTimeAsync(2 * USAGE_TURN_FLOOR_MS);
    expect(calls).toHaveLength(0);
  });

  it("disposal ignores a late answer and prevents its queued CLI read", async () => {
    const { reads, calls, landed, failed } = harness();
    reads.refresh();
    reads.refresh();
    reads.dispose();
    await answer(calls);
    expect(calls).toHaveLength(1);
    expect(landed).toHaveLength(0);
    expect(failed).toHaveLength(0);
  });

  it("reactivation waits for the old read and ignores its stale failure", async () => {
    const { reads, calls, landed, failed } = harness();
    reads.refresh();
    reads.dispose();
    reads.refresh();
    reads.turnDone();
    reads.shown();
    expect(vi.getTimerCount()).toBe(0);
    reads.activate();
    reads.refresh();
    expect(calls).toHaveLength(1);
    await answer(calls, "fail");
    expect(failed).toHaveLength(0);
    expect(calls).toHaveLength(2);
    await answer(calls);
    expect(landed).toHaveLength(1);
  });

  it("a synchronous bridge failure releases the read lane", async () => {
    const failure = new Error("bridge unavailable");
    const read = vi.fn<() => Promise<EngineUsage[]>>()
      .mockImplementationOnce(() => { throw failure; }).mockResolvedValue([]);
    const failed = vi.fn();
    const landed = vi.fn();
    const reads = new PlanUsageReads({ read, failed, landed, loading: () => {} });
    reads.refresh();
    await vi.advanceTimersByTimeAsync(0);
    expect(failed).toHaveBeenCalledWith(failure);
    reads.refresh();
    await vi.advanceTimersByTimeAsync(0);
    expect(landed).toHaveBeenCalledOnce();
  });

  it("holds a turn's read while the window is hidden and reads it once shown", async () => {
    const window = { hidden: true };
    const { reads, calls } = harness(window);
    reads.turnDone();
    reads.turnDone();
    await vi.advanceTimersByTimeAsync(3 * USAGE_TURN_FLOOR_MS);
    expect(calls).toHaveLength(0);
    window.hidden = false;
    reads.shown();
    expect(calls).toHaveLength(1);
    await answer(calls);
    // Paid: showing again owes nothing.
    reads.shown();
    expect(calls).toHaveLength(1);
  });

  it("leaves a turn still inside its floor to its own timer when shown", async () => {
    const window = { hidden: false };
    const { reads, calls } = harness(window);
    reads.refresh();
    await answer(calls);
    reads.turnDone();
    window.hidden = true;
    await vi.advanceTimersByTimeAsync(10_000);
    window.hidden = false;
    reads.shown();
    expect(calls).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(USAGE_TURN_FLOOR_MS);
    expect(calls).toHaveLength(2);
  });
});

describe("the usage clock and catch-up while hidden", () => {
  it("runs the minute clock only for a snapshot in a visible window", () => {
    expect(usageClockActive(false, 1_000)).toBe(true);
    expect(usageClockActive(true, 1_000)).toBe(false);
    expect(usageClockActive(false, null)).toBe(false);
  });

  it("reads nothing hidden, and the first read or the stale catch-up once visible", () => {
    const at = (status: "idle" | "loading" | "ready" | "error", stale: boolean, open: boolean, hidden: boolean) =>
      usageCatchUp({ status, stale, open, hidden });
    expect(at("ready", true, false, true)).toBe("none");
    expect(at("idle", false, false, true)).toBe("none");
    expect(at("ready", true, false, false)).toBe("now");
    expect(at("ready", false, true, false)).toBe("none");
    expect(at("idle", false, false, false)).toBe("soon");
    expect(at("idle", false, true, false)).toBe("now");
    expect(at("loading", true, true, false)).toBe("none");
    expect(at("error", true, true, false)).toBe("none");
  });
});
