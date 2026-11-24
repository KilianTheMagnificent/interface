import { afterEach, describe, expect, it, vi } from "vitest";
import {
  browserTerminalSplitterDragEnvironment, TerminalSplitterDragController,
  terminalSplitterRatio, type TerminalSplitterDragEnvironment,
  type TerminalSplitterPointer,
} from "./terminal-splitter-drag";

interface FakeDragEnvironment {
  readonly environment: TerminalSplitterDragEnvironment;
  readonly listeners: () => Parameters<TerminalSplitterDragEnvironment["listen"]>[0] | null;
  readonly flushFrames: () => void;
  readonly canceledFrames: readonly number[];
  readonly cleanups: () => number;
}

function fakeEnvironment(): FakeDragEnvironment {
  let activeListeners: Parameters<TerminalSplitterDragEnvironment["listen"]>[0] | null = null;
  let cleanupCount = 0;
  let nextFrame = 1;
  const frames = new Map<number, () => void>();
  const canceledFrames: number[] = [];
  return {
    environment: {
      listen: listeners => {
        activeListeners = listeners;
        let live = true;
        return () => {
          if (!live) return;
          live = false;
          cleanupCount += 1;
          if (activeListeners === listeners) activeListeners = null;
        };
      },
      requestFrame: callback => {
        const handle = nextFrame;
        nextFrame += 1;
        frames.set(handle, callback);
        return handle;
      },
      cancelFrame: handle => {
        canceledFrames.push(handle);
        frames.delete(handle);
      },
    },
    listeners: () => activeListeners,
    flushFrames: () => {
      const pending = [...frames.entries()];
      frames.clear();
      for (const [, callback] of pending) callback();
    },
    canceledFrames,
    cleanups: () => cleanupCount,
  };
}

function pointer(pointerId: number, clientX: number, clientY: number, buttons = 1): {
  readonly event: TerminalSplitterPointer;
  readonly prevented: () => number;
} {
  let preventions = 0;
  return {
    event: {
      pointerId, clientX, clientY, buttons,
      preventDefault: () => { preventions += 1; },
    },
    prevented: () => preventions,
  };
}

afterEach(() => { vi.unstubAllGlobals(); });

describe("terminal splitter drag", () => {
  it("measures the flexible track without jumping away from the grab point", () => {
    const container = { left: 100, top: 50, width: 1_000, height: 600 };
    expect(terminalSplitterRatio({
      orientation: "horizontal", container, splitterSize: 12, grabOffset: 6,
      clientX: 600, clientY: 0,
    })).toBeCloseTo(0.5);
    expect(terminalSplitterRatio({
      orientation: "vertical", container, splitterSize: 12, grabOffset: 3,
      clientX: 0, clientY: 347,
    })).toBeCloseTo(0.5);
    expect(terminalSplitterRatio({
      orientation: "horizontal", container, splitterSize: 12, grabOffset: 6,
      clientX: -10_000, clientY: 0,
    })).toBe(0.2);
    expect(terminalSplitterRatio({
      orientation: "vertical", container, splitterSize: 12, grabOffset: 3,
      clientX: 0, clientY: 10_000,
    })).toBe(0.8);
  });

  it("tracks the active pointer globally, coalesces moves, and flushes release", () => {
    const fake = fakeEnvironment();
    const ratios: number[] = [];
    let finishes = 0;
    const controller = new TerminalSplitterDragController(fake.environment);
    expect(controller.start({
      pointerId: 7, clientX: 500, clientY: 300, orientation: "horizontal",
      container: { left: 0, top: 0, width: 1_000, height: 600 },
      splitter: { left: 494, top: 0, width: 12, height: 600 },
      onRatioChange: ratio => { ratios.push(ratio); },
      onFinish: () => { finishes += 1; },
    })).toBe(true);

    const other = pointer(9, 900, 300);
    fake.listeners()?.move(other.event);
    fake.listeners()?.stop(other.event);
    expect(other.prevented()).toBe(0);
    expect(fake.cleanups()).toBe(0);

    const first = pointer(7, 620, 300);
    const latest = pointer(7, 700, 300);
    fake.listeners()?.move(first.event);
    fake.listeners()?.move(latest.event);
    expect(first.prevented()).toBe(1);
    expect(latest.prevented()).toBe(1);
    expect(ratios).toEqual([]);
    fake.flushFrames();
    expect(ratios).toHaveLength(1);
    expect(ratios[0]).toBeGreaterThan(0.6);

    const release = pointer(7, 10_000, 300, 0);
    fake.listeners()?.stop(release.event);
    expect(release.prevented()).toBe(1);
    expect(ratios).toEqual([ratios[0], 0.8]);
    expect(fake.cleanups()).toBe(1);
    expect(finishes).toBe(1);
    expect(fake.listeners()).toBeNull();
  });

  it("does not treat a canceled pointer's synthetic coordinates as a final move", () => {
    const fake = fakeEnvironment();
    const ratios: number[] = [];
    const controller = new TerminalSplitterDragController(fake.environment);
    controller.start({
      pointerId: 3, clientX: 500, clientY: 300, orientation: "horizontal",
      container: { left: 0, top: 0, width: 1_000, height: 600 },
      splitter: { left: 494, top: 0, width: 12, height: 600 },
      onRatioChange: ratio => { ratios.push(ratio); },
    });
    fake.listeners()?.move(pointer(3, 650, 300).event);
    fake.listeners()?.cancel(pointer(3, 0, 0).event);

    expect(ratios).toHaveLength(1);
    expect(ratios[0]).toBeGreaterThan(0.6);
    expect(fake.cleanups()).toBe(1);
  });

  it("recovers when release occurred outside the window and the pointer returns as hover", () => {
    const fake = fakeEnvironment();
    const ratios: number[] = [];
    const controller = new TerminalSplitterDragController(fake.environment);
    controller.start({
      pointerId: 8, clientX: 500, clientY: 300, orientation: "horizontal",
      container: { left: 0, top: 0, width: 1_000, height: 600 },
      splitter: { left: 494, top: 0, width: 12, height: 600 },
      onRatioChange: ratio => { ratios.push(ratio); },
    });
    fake.listeners()?.move(pointer(8, 650, 300).event);
    const hover = pointer(8, 50, 300, 0);
    fake.listeners()?.move(hover.event);

    expect(hover.prevented()).toBe(0);
    expect(ratios).toHaveLength(1);
    expect(ratios[0]).toBeGreaterThan(0.6);
    expect(fake.cleanups()).toBe(1);
    expect(fake.listeners()).toBeNull();
  });

  it("cleans up even when the ratio consumer throws during release", () => {
    const fake = fakeEnvironment();
    let finishes = 0;
    const controller = new TerminalSplitterDragController(fake.environment);
    controller.start({
      pointerId: 5, clientX: 500, clientY: 300, orientation: "horizontal",
      container: { left: 0, top: 0, width: 1_000, height: 600 },
      splitter: { left: 494, top: 0, width: 12, height: 600 },
      onRatioChange: () => { throw new Error("consumer failed"); },
      onFinish: () => { finishes += 1; },
    });

    expect(() => { fake.listeners()?.stop(pointer(5, 700, 300, 0).event); })
      .toThrow("consumer failed");
    expect(fake.cleanups()).toBe(1);
    expect(finishes).toBe(1);
    expect(fake.listeners()).toBeNull();
  });

  it("cancels queued work and global listeners when the workspace unmounts", () => {
    const fake = fakeEnvironment();
    const ratios: number[] = [];
    let finishes = 0;
    const controller = new TerminalSplitterDragController(fake.environment);
    expect(controller.start({
      pointerId: 4, clientX: 200, clientY: 300, orientation: "vertical",
      container: { left: 0, top: 0, width: 400, height: 600 },
      splitter: { left: 0, top: 294, width: 400, height: 12 },
      onRatioChange: ratio => { ratios.push(ratio); },
      onFinish: () => { finishes += 1; },
    })).toBe(true);
    fake.listeners()?.move(pointer(4, 200, 420).event);

    controller.dispose();
    fake.flushFrames();

    expect(ratios).toEqual([]);
    expect(fake.canceledFrames).toEqual([1]);
    expect(fake.cleanups()).toBe(1);
    expect(finishes).toBe(1);
  });

  it("registers and removes window capture listeners for move, release, cancel, and blur", () => {
    const addEventListener = vi.fn();
    const removeEventListener = vi.fn();
    vi.stubGlobal("window", {
      addEventListener,
      removeEventListener,
      requestAnimationFrame: vi.fn(() => 1),
      cancelAnimationFrame: vi.fn(),
    });
    const environment = browserTerminalSplitterDragEnvironment();
    const remove = environment.listen({
      move: () => undefined, stop: () => undefined, cancel: () => undefined,
    });

    expect(addEventListener.mock.calls.map(call => [call[0], call[2]])).toEqual([
      ["pointermove", true],
      ["pointerup", true],
      ["pointercancel", true],
      ["blur", undefined],
    ]);
    remove();
    expect(removeEventListener.mock.calls.map(call => [call[0], call[2]])).toEqual([
      ["pointermove", true],
      ["pointerup", true],
      ["pointercancel", true],
      ["blur", undefined],
    ]);
  });
});
