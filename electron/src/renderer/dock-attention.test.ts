import { describe, expect, it } from "vitest";
import { dockAttention } from "./dock-attention";

const pane = (over: Record<string, unknown> = {}) => ({
  state: "running" as const, activity: "waiting" as const, ...over,
});

describe("terminal panes in the Dock badge (review D5)", () => {
  it("counts live panes waiting for input, the ones the attention tray lists", () => {
    expect(dockAttention([
      pane(), pane({ state: "starting" }), pane({ activity: "working" }), pane({ activity: "idle" }),
      pane({ state: "exited" }), pane({ state: "failed" }),
    ] as never, [])).toEqual({ version: 1, waitingPanes: 2 });
  });

  it("leaves out a pane whose session the registry already counts as waiting", () => {
    const sessions = [{ engine: "claude" as const, sessionId: "s1", status: "waiting" as const },
      { engine: "claude" as const, sessionId: "s2", status: "idle" as const }];
    expect(dockAttention([pane({ sessionKey: "claude#s1" }), pane({ sessionKey: "claude#s2" })] as never,
      sessions).waitingPanes).toBe(1);
  });
});
