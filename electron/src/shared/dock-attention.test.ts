import { describe, expect, it } from "vitest";
import { dockBadgeCount, parseDockAttention } from "./dock-attention";

describe("the Dock badge's pane report (review D5)", () => {
  it("adds waiting terminal panes to the waiting sessions", () => {
    expect(dockBadgeCount(2, { version: 1, waitingPanes: 3 })).toBe(5);
    expect(dockBadgeCount(2, null)).toBe(2);
  });

  it("accepts exactly a v1 report", () => {
    expect(parseDockAttention({ version: 1, waitingPanes: 4 })).toEqual({ version: 1, waitingPanes: 4 });
    for (const bad of [null, [], {}, { version: 2, waitingPanes: 1 }, { version: 1, waitingPanes: -1 },
      { version: 1, waitingPanes: 1.5 }, { version: 1, waitingPanes: "2" },
      { version: 1, waitingPanes: 10_001 }, { version: 1, waitingPanes: 1, extra: true }]) {
      expect(parseDockAttention(bad), JSON.stringify(bad)).toBeNull();
    }
  });
});
