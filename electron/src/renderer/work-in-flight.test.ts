import { describe, expect, it } from "vitest";
import { workInFlight } from "./work-in-flight";

/* Review A2 (2026-10-14): what the quit prompt counts. */
describe("work in flight", () => {
  const pane = (profileId: string, state: "starting" | "running" | "exited",
      activity?: "idle" | "working" | "waiting", sessionKey?: string) => ({
    state, ...(activity === undefined ? {} : { activity }),
    launch: { version: 1 as const, profileId: profileId as never },
    ...(sessionKey === undefined ? {} : { sessionKey }),
  });
  const session = (sessionId: string, status: "working" | "waiting" | "idle", origin?: "app" | "store") => ({
    engine: "claude" as const, sessionId, status, ...(origin === undefined ? {} : { origin }),
  });

  it("counts agent panes that are starting, working or waiting, never a shell", () => {
    expect(workInFlight([
      pane("claude", "running", "working"), pane("codex", "running", "waiting"),
      pane("grok", "starting"), pane("claude", "running", "idle"),
      pane("claude", "exited"), pane("shell", "running", "working"),
    ], [], [], 0)).toEqual({ version: 1, terminals: 3, threads: 0, away: 0, runs: 0, waiting: 1 });
  });

  it("counts threads Interface started, once, and Away loops by run", () => {
    expect(workInFlight(
      [pane("claude", "running", "working", "claude#pane")],
      [session("t1", "working", "app"), session("t2", "waiting", "app"), session("t3", "idle", "app"),
        session("cli", "working", "store"), session("unknown", "working"),
        session("pane", "working", "app"), session("away", "working", "app")],
      ["claude#away"], 2,
    )).toEqual({ version: 1, terminals: 1, threads: 2, away: 2, runs: 0, waiting: 1 });
  });

  // Review D3 (2026-10-14): teammate routine runs keep the Mac awake too.
  it("counts teammate runs in progress, once, and what only waits", () => {
    expect(workInFlight([], [session("run", "working", "app"), session("t1", "waiting", "app")], [], 0, [
      { status: "running", sessionKey: "claude#run" }, { status: "starting", sessionKey: null },
      { status: "waiting", sessionKey: null }, { status: "queued", sessionKey: null },
      { status: "succeeded", sessionKey: null }, { status: "failed", sessionKey: null },
    ])).toEqual({ version: 1, terminals: 0, threads: 1, away: 0, runs: 3, waiting: 2 });
  });
});
