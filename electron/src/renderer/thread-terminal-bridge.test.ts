import { describe, expect, it } from "vitest";
import { continueAsThreadKey, continueAsThreadRefusal } from "./thread-terminal-bridge";

describe("Continue as thread (review C1)", () => {
  it("names a Claude or Codex pane's reported session by its registry key, and nothing else", () => {
    expect(continueAsThreadKey({
      launch: { version: 1, profileId: "claude" },
      launchSession: { version: 1, id: "5b0c1d2e", resume: true },
    })).toBe("claude#5b0c1d2e");
    expect(continueAsThreadKey({
      launch: { version: 1, profileId: "codex" },
      launchSession: { version: 1, id: "0199f2a4", resume: false },
    })).toBe("codex#0199f2a4");
    // Not reported yet, a shell, another CLI, or no pane at all.
    expect(continueAsThreadKey({ launch: { version: 1, profileId: "claude" } })).toBeNull();
    expect(continueAsThreadKey({
      launch: { version: 1, profileId: "claude" }, launchSession: { version: 1, id: " " },
    })).toBeNull();
    expect(continueAsThreadKey({
      launch: { version: 1, profileId: "grok" }, launchSession: { version: 1, id: "g1" },
    })).toBeNull();
    expect(continueAsThreadKey({ launchSession: { version: 1, id: "s1" } })).toBeNull();
    expect(continueAsThreadKey(undefined)).toBeNull();
  });

  it("is offered between turns of a running Claude or Codex pane, and says why not otherwise", () => {
    expect(continueAsThreadRefusal({ profileId: "claude", status: "running", activity: "idle" })).toBeNull();
    expect(continueAsThreadRefusal({ profileId: "codex", status: "running" })).toBeNull();
    expect(continueAsThreadRefusal({ profileId: "claude", status: "running", activity: "working" }))
      .toContain("Wait for the agent to finish");
    expect(continueAsThreadRefusal({ profileId: "claude", status: "running", activity: "waiting" }))
      .toContain("Wait for the agent to finish");
    expect(continueAsThreadRefusal({ profileId: "claude", status: "exited" })).toContain("no longer running");
    expect(continueAsThreadRefusal({ status: "running" })).toContain("Claude or Codex");
    expect(continueAsThreadRefusal({ profileId: "opencode", status: "running" })).toContain("Claude or Codex");
  });
});
