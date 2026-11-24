import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  TERMINAL_DRAG_FIT_INTERVAL_MS, TERMINAL_LAYOUT_DRAGGING, TERMINAL_LAYOUT_SLIDING, TERMINAL_PANE_LAYOUT_EVENT,
  TerminalFitThrottle, holdTerminals, throttleTerminalFits,
} from "./terminal-pane-motion";
import { TerminalResizeQueue, type TerminalResizeSize } from "./terminal-resize-queue";

const FRAME_MS = 16;

/** Just enough of a split container: its attributes and its emulator hosts. */
function container(hosts: number) {
  const attributes = new Set<string>();
  const events = Array.from({ length: hosts }, () => [] as string[]);
  const element = {
    setAttribute: (name: string) => { attributes.add(name); },
    removeAttribute: (name: string) => { attributes.delete(name); },
    querySelectorAll: (selector: string) => {
      expect(selector).toBe(".terminal-xterm");
      return events.map(log => ({ dispatchEvent: (event: Event) => { log.push(event.type); return true; } }));
    },
  };
  return { element: element as unknown as HTMLElement, attributes, events };
}

describe("terminal fits under a dragged divider", () => {
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { vi.useRealTimers(); });

  it("marks the split while dragging and refits every emulator once on release", () => {
    const split = container(2);
    const settle = throttleTerminalFits(split.element);
    expect([...split.attributes]).toEqual([TERMINAL_LAYOUT_DRAGGING]);
    expect(split.events).toEqual([[], []]);
    settle();
    expect(split.attributes.size).toBe(0);
    expect(split.events).toEqual([[TERMINAL_PANE_LAYOUT_EVENT], [TERMINAL_PANE_LAYOUT_EVENT]]);
    settle();
    expect(split.events).toEqual([[TERMINAL_PANE_LAYOUT_EVENT], [TERMINAL_PANE_LAYOUT_EVENT]]);
    // The slide hold is unchanged: its own attribute, the same single settle.
    const slide = container(1);
    holdTerminals(slide.element)();
    expect(slide.events).toEqual([[TERMINAL_PANE_LAYOUT_EVENT]]);
    expect(TERMINAL_LAYOUT_SLIDING).not.toBe(TERMINAL_LAYOUT_DRAGGING);
  });

  it("coalesces a frame-rate stream of resizes into trailing fits", () => {
    const fits: number[] = [];
    const throttle = new TerminalFitThrottle(() => { fits.push(Date.now()); });
    const start = Date.now();
    const drag = 1_000;
    for (let at = 0; at < drag; at += FRAME_MS) {
      throttle.request();
      vi.advanceTimersByTime(FRAME_MS);
    }
    const frames = Math.ceil(drag / FRAME_MS);
    expect(fits.length).toBeLessThanOrEqual(Math.ceil(drag / TERMINAL_DRAG_FIT_INTERVAL_MS) + 1);
    expect(fits.length).toBeLessThan(frames / 2);
    expect(fits[0] - start).toBe(TERMINAL_DRAG_FIT_INTERVAL_MS);
    for (let i = 1; i < fits.length; i++) {
      expect(fits[i] - fits[i - 1]).toBeGreaterThanOrEqual(TERMINAL_DRAG_FIT_INTERVAL_MS);
    }
    throttle.request();
    throttle.cancel();
    const settled = fits.length;
    vi.advanceTimersByTime(10 * TERMINAL_DRAG_FIT_INTERVAL_MS);
    expect(fits).toHaveLength(settled);
  });

  it("ends a drag at the exact final size, fitted and sent to the PTY once it settles", () => {
    // A model of TerminalView's wiring: the observer throttles while the split
    // is marked, and the settle's layout event cancels that and fits exactly.
    const split = container(1);
    let width = 400;
    const cols = (): number => Math.floor(width / 8);
    const emulator = { cols: cols(), fits: 0, reflows: 0 };
    const sent: TerminalResizeSize[] = [];
    const queue = new TerminalResizeQueue(75, error => { throw error; });
    const fit = (): void => {
      emulator.fits++;
      if (emulator.cols === cols()) return;
      emulator.cols = cols();
      emulator.reflows++;
      queue.schedule("pane", { columns: emulator.cols, rows: 24 }, async size => { sent.push(size); });
    };
    const throttle = new TerminalFitThrottle(fit);
    const observed = (): void => {
      if (split.attributes.has(TERMINAL_LAYOUT_DRAGGING)) throttle.request();
      else { throttle.cancel(); fit(); }
    };
    const layoutEvent = (): void => { throttle.cancel(); fit(); };

    const settle = throttleTerminalFits(split.element);
    const frames = 45;
    for (let frame = 0; frame < frames; frame++) {
      width += 7;
      observed();
      vi.advanceTimersByTime(FRAME_MS);
    }
    // The release lands mid-interval: a fit is pending and the last frames
    // have not been fitted yet.
    width += 3;
    observed();
    expect(emulator.cols).not.toBe(cols());
    settle();
    expect(split.events[0]).toEqual([TERMINAL_PANE_LAYOUT_EVENT]);
    layoutEvent();
    expect(emulator.cols).toBe(cols());
    const fitsAtRelease = emulator.fits;
    vi.advanceTimersByTime(1_000);
    // No stale throttled fit after the release, and the PTY got the final size.
    expect(emulator.fits).toBe(fitsAtRelease);
    expect(emulator.reflows).toBeLessThan(frames / 2);
    expect(sent.at(-1)).toEqual({ columns: cols(), rows: 24 });
    // A resize after the release fits at once again.
    width += 40;
    observed();
    expect(emulator.cols).toBe(cols());
    queue.dispose();
  });
});
