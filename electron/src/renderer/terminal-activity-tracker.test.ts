import { afterEach, describe, expect, it, vi } from "vitest";
import { TerminalActivityTracker } from "./terminal-activity-tracker";
import { TERMINAL_ACTIVITY_QUIET_MS } from "./terminal-activity";
const bytes = (text: string) => new TextEncoder().encode(text);
const signal = (value: string) => `\x1b]2;interface-terminal-activity:v1:${value}\x07`;
afterEach(() => vi.useRealTimers());
describe("terminal activity lifetime", () => {
  it("publishes completion even when start and finish share one output batch", () => {
    const activity = vi.fn(), completed = vi.fn();
    const tracker = new TerminalActivityTracker({ profile: () => "opencode", activity, completed });
    tracker.output("pane:1", bytes(signal("working") + signal("complete")));
    expect(completed).toHaveBeenCalledExactlyOnceWith("pane:1", 1);
    tracker.dispose();
  });
  it("cancels retired generation timers and retains the new generation", () => {
    vi.useFakeTimers();
    const activity = vi.fn();
    const tracker = new TerminalActivityTracker({ profile: () => undefined, activity, completed: vi.fn() });
    tracker.output("pane:1", bytes("old output"));
    tracker.output("pane:2", bytes("new output"));
    tracker.prune(key => key === "pane:1");
    activity.mockClear();
    vi.advanceTimersByTime(TERMINAL_ACTIVITY_QUIET_MS);
    expect(activity).toHaveBeenCalledExactlyOnceWith("pane:2", "idle");
    tracker.output("pane:2", bytes("more"));
    activity.mockClear();
    tracker.dispose();
    vi.runAllTimers();
    expect(activity).not.toHaveBeenCalled();
  });
  it("input cancels quiet work and late profile identity governs future output", () => {
    vi.useFakeTimers();
    let profile: "codex" | undefined;
    const activity = vi.fn();
    const tracker = new TerminalActivityTracker({ profile: () => profile, activity, completed: vi.fn() });
    tracker.output("pane:1", bytes("work"));
    tracker.input("pane:1");
    activity.mockClear();
    vi.runAllTimers();
    expect(activity).not.toHaveBeenCalled();
    profile = "codex";
    tracker.output("pane:1", bytes("prompt redraw"));
    expect(activity).not.toHaveBeenCalledWith("pane:1", "working");
    tracker.dispose();
  });
});

describe("AGY completion publication", () => {
  it("publishes one native completion despite hooks and a coalesced output batch", () => {
    const activity = vi.fn(), completed = vi.fn();
    const tracker = new TerminalActivityTracker({ profile: () => "gemini", activity, completed });
    const native = (state: string) => `\x1b]7501;state=${state}:app=antigravity\x07`;
    tracker.output("agy:1", bytes(signal("working") + native("working")
      + signal("idle") + native("done") + signal("working") + native("done")));
    expect(completed).toHaveBeenCalledExactlyOnceWith("agy:1", 1);
    expect(activity).not.toHaveBeenCalledWith("agy:1", "working");
    // Retiring the generation discards its native status preference too.
    tracker.prune(key => key === "agy:1");
    tracker.output("agy:2", bytes(signal("working")));
    expect(activity).toHaveBeenLastCalledWith("agy:2", "working");
    tracker.dispose();
  });
});
