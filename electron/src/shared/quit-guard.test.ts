import { describe, expect, it } from "vitest";
import { NO_WORK_IN_FLIGHT, parseWorkInFlight, quitPrompt, workingNow } from "./quit-guard";

/* Review A2 (2026-10-14): Cmd+Q ended in-flight turns, Away loops and agent
   panes without a word. Main asks first, from the renderer's last count. */
describe("the quit prompt", () => {
  it("asks how many agents are still working and names what stops", () => {
    expect(quitPrompt({ version: 1, terminals: 2, threads: 1, away: 1, runs: 0, waiting: 0 })).toEqual({
      message: "4 agents are still working. Quit anyway?",
      detail: "Quitting stops 2 agent panes, 1 thread turn and 1 Away loop."
        + " A stopped Away loop does not restart on its own.",
    });
    expect(quitPrompt({ version: 1, terminals: 1, threads: 0, away: 0, runs: 0, waiting: 1 })).toEqual({
      message: "1 agent is still working. Quit anyway?",
      detail: "Quitting stops 1 agent pane.",
    });
    expect(quitPrompt({ version: 1, terminals: 0, threads: 3, away: 0, runs: 0, waiting: 0 })?.detail)
      .toBe("Quitting stops 3 thread turns.");
  });

  it("names teammate runs, and asks about work that is only waiting", () => {
    expect(quitPrompt({ version: 1, terminals: 0, threads: 0, away: 0, runs: 2, waiting: 2 })).toEqual({
      message: "2 agents are still working. Quit anyway?",
      detail: "Quitting stops 2 teammate runs.",
    });
  });

  it("does not ask when nothing would be lost or nothing was reported", () => {
    expect(quitPrompt(NO_WORK_IN_FLIGHT)).toBeNull();
    expect(quitPrompt(null)).toBeNull();
  });

  it("admits only a well-formed renderer report", () => {
    expect(parseWorkInFlight({ version: 1, terminals: 1, threads: 0, away: 2, runs: 1, waiting: 1 }))
      .toEqual({ version: 1, terminals: 1, threads: 0, away: 2, runs: 1, waiting: 1 });
    for (const bad of [null, [], "1", { version: 2, terminals: 0, threads: 0, away: 0 },
      { version: 1, terminals: -1, threads: 0, away: 0 }, { version: 1, terminals: 1.5, threads: 0, away: 0 },
      { version: 1, terminals: 0, threads: 0 }, { version: 1, terminals: 1e9, threads: 0, away: 0 },
      { version: 1, terminals: 0, threads: 0, away: 0, runs: 0 },
      { version: 1, terminals: 1, threads: 0, away: 1, runs: 0, waiting: 2 }]) {
      expect(parseWorkInFlight(bad)).toBeNull();
    }
  });
});

/* Review D3 (2026-10-14): the same count keeps the Mac awake. */
describe("working now", () => {
  it("is working while anything progresses, not while everything only waits", () => {
    expect(workingNow({ ...NO_WORK_IN_FLIGHT, away: 1 })).toBe(true);
    expect(workingNow({ ...NO_WORK_IN_FLIGHT, runs: 1 })).toBe(true);
    expect(workingNow({ ...NO_WORK_IN_FLIGHT, terminals: 2, waiting: 1 })).toBe(true);
    expect(workingNow({ ...NO_WORK_IN_FLIGHT, terminals: 1, threads: 1, waiting: 2 })).toBe(false);
    expect(workingNow(NO_WORK_IN_FLIGHT)).toBe(false);
    expect(workingNow(null)).toBe(false);
  });
});
