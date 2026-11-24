import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ImportProgress } from "../main/bridge";
import { IMPORT_PROGRESS_INTERVAL_MS, ImportProgressThrottle } from "./use-session-import";

const at = (done: number, total = 1_100): ImportProgress =>
  ({ adopt: true, done, total, ok: done, failed: 0 });

describe("import progress", () => {
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { vi.useRealTimers(); });

  it("renders the total at once, then the newest at most once per interval, and the end at once", () => {
    const shown: number[] = [];
    const throttle = new ImportProgressThrottle(progress => { shown.push(progress.done); });
    throttle.push(at(0));
    expect(shown).toEqual([0]);
    for (let done = 1; done <= 40; done++) throttle.push(at(done));
    expect(shown).toEqual([0]);
    vi.advanceTimersByTime(IMPORT_PROGRESS_INTERVAL_MS);
    expect(shown).toEqual([0, 40]);
    for (let done = 41; done < 1_100; done++) throttle.push(at(done));
    throttle.push(at(1_100));
    expect(shown).toEqual([0, 40, 1_100]);
    vi.advanceTimersByTime(10 * IMPORT_PROGRESS_INTERVAL_MS);
    expect(shown).toEqual([0, 40, 1_100]);
  });

  it("drops a pending update once the import has ended and cleared its row", () => {
    const shown: number[] = [];
    const throttle = new ImportProgressThrottle(progress => { shown.push(progress.done); });
    throttle.push(at(0));
    throttle.push(at(5));
    throttle.cancel();
    vi.advanceTimersByTime(IMPORT_PROGRESS_INTERVAL_MS);
    expect(shown).toEqual([0]);
  });
});
