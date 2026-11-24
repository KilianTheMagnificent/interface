import { readFileSync } from "node:fs";
import { describe, expect, it, vi } from "vitest";
import type { TerminalLaunchProfileId } from "../shared/ipc";
import {
  createTerminalActivityMonitor, feedTerminalActivity, isTerminalProtocolReply, noteTerminalInput,
  setTerminalActivityProfile,
} from "./terminal-activity";
import { TerminalActivityTracker } from "./terminal-activity-tracker";

interface Step {
  readonly output?: string;
  readonly input?: string;
  readonly profile?: TerminalLaunchProfileId;
  readonly waiting: boolean;
}
interface Case {
  readonly name: string;
  readonly profile: TerminalLaunchProfileId;
  readonly steps: readonly Step[];
}
const fixture = JSON.parse(readFileSync(
  new URL("../fixtures/terminal-attention-v44.json", import.meta.url), "utf8")) as {
  version: number; cases: Case[];
};
const encoder = new TextEncoder();

// crates/interface-terminal/src/attention.rs replays the same cases: Rust's
// answer is the one this renderer used to compute, byte for byte.
describe("shared terminal attention rules v44", () => {
  it("is the fixture the Rust host reads", () => {
    expect(fixture.version).toBe(44);
    expect(fixture.cases.length).toBeGreaterThanOrEqual(12);
  });
  it.each(fixture.cases.map(entry => [entry.name, entry] as const))("%s", (_name, entry) => {
    const monitor = createTerminalActivityMonitor();
    setTerminalActivityProfile(monitor, entry.profile);
    entry.steps.forEach((step, index) => {
      if (step.output !== undefined) feedTerminalActivity(monitor, encoder.encode(step.output));
      else if (step.input !== undefined) {
        const bytes = encoder.encode(step.input);
        // As the terminal center does: protocol replies are not the user's.
        if (!isTerminalProtocolReply(bytes)) noteTerminalInput(monitor);
      } else if (step.profile !== undefined) setTerminalActivityProfile(monitor, step.profile);
      expect(monitor.activity === "waiting", `step ${index}`).toBe(step.waiting);
    });
  });
});

describe("terminal needs-you is Rust's answer", () => {
  const bytes = (text: string): Uint8Array => encoder.encode(text);
  it("publishes the host's waiting and never its own", () => {
    const activity = vi.fn();
    const tracker = new TerminalActivityTracker({ profile: () => "claude", activity, completed: vi.fn() });
    tracker.output("pane:1", bytes("\x1b]2;interface-claude-activity:working\x07"));
    expect(activity).toHaveBeenLastCalledWith("pane:1", "working");
    // The dialog frame alone keeps the turn working until Rust says so.
    tracker.output("pane:1", bytes("\x1b]2;interface-claude-activity:waiting\x07"));
    expect(activity).not.toHaveBeenCalledWith("pane:1", "waiting");
    tracker.hostAttention("pane:1", true);
    expect(activity).toHaveBeenLastCalledWith("pane:1", "waiting");
    // Typing answers the dialog on the Mac's own reading; the pane still
    // needs you until Rust's next summary says it does not.
    tracker.input("pane:1");
    expect(activity).toHaveBeenLastCalledWith("pane:1", "waiting");
    tracker.hostAttention("pane:1", false);
    expect(activity).toHaveBeenLastCalledWith("pane:1", "working");
    tracker.output("pane:1", bytes("\x1b]2;interface-claude-activity:idle\x07"));
    expect(activity).toHaveBeenLastCalledWith("pane:1", "idle");
    tracker.dispose();
  });
  it("lets a pane need you before this window saw a byte of it", () => {
    const activity = vi.fn();
    const tracker = new TerminalActivityTracker({ profile: () => "shell", activity, completed: vi.fn() });
    // A reload forgets every parser; Rust still knows the pane is waiting.
    tracker.hostAttention("pane:2", false);
    expect(activity).not.toHaveBeenCalled();
    tracker.hostAttention("pane:2", true);
    expect(activity).toHaveBeenCalledExactlyOnceWith("pane:2", "waiting");
    tracker.hostAttention("pane:2", true);
    expect(activity).toHaveBeenCalledTimes(1);
    tracker.hostAttention("pane:2", false);
    expect(activity).toHaveBeenLastCalledWith("pane:2", "idle");
    tracker.dispose();
  });
});
