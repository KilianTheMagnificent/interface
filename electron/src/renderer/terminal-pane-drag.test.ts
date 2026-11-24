import { describe, expect, it } from "vitest";
import { dropTerminalPane } from "./terminal-pane-drag";
import {
  EMPTY_TERMINAL_WORKSPACE_STATE, openTerminalTab, splitTerminalPane,
  parseTerminalWorkspaceState, serializeTerminalWorkspaceState,
  swapTerminalPanes, terminalWorkspacePaneOrder, toggleTerminalPaneMaximized,
} from "./terminal-workspace";

function workspace() {
  let state = openTerminalTab(EMPTY_TERMINAL_WORKSPACE_STATE, "left", "a");
  state = openTerminalTab(state, "left", "a2");
  return splitTerminalPane(state, { paneId: "left", newPaneId: "right",
    splitId: "columns", terminalId: "b", orientation: "horizontal", ratio: 0.35 });
}

describe("pane header drops", () => {
  it("moves the whole pane from an inactive tab, keeping both panes and their active tabs", () => {
    const state = workspace();
    const next = dropTerminalPane(state, "a", "right", 0);
    expect(terminalWorkspacePaneOrder(next)).toEqual(["right", "left"]);
    expect(next.panes).toBe(state.panes);
    expect(next.root).toEqual({ kind: "split", id: "columns", ratio: 0.35,
      orientation: "horizontal", first: { kind: "pane", paneId: "right" },
      second: { kind: "pane", paneId: "left" } });
    expect(next.focusedPaneId).toBe(state.focusedPaneId);
    expect(next.arrangement).toBe(state.arrangement);
  });

  it("still reorders tabs within their own pane", () => {
    const state = workspace();
    const next = dropTerminalPane(state, "a", "left", 2);
    expect(next.panes[0].tabs.map(tab => tab.terminalId)).toEqual(["a2", "a"]);
    expect(terminalWorkspacePaneOrder(next)).toEqual(["left", "right"]);
    expect(next.panes[1]).toEqual(state.panes[1]);
  });

  it("ignores stale sources, stale targets and unchanged self-drops", () => {
    const state = workspace();
    expect(dropTerminalPane(state, "missing", "right")).toBe(state);
    expect(dropTerminalPane(state, "a", "missing")).toBe(state);
    expect(dropTerminalPane(state, "a2", "left", 2)).toBe(state);
  });

  it("preserves an unaffected branch, nested split ratios and maximized identity", () => {
    let state = splitTerminalPane(workspace(), { paneId: "right", newPaneId: "bottom",
      splitId: "rows", terminalId: "c", orientation: "vertical", ratio: 0.65 });
    state = toggleTerminalPaneMaximized(state, "right");
    const next = swapTerminalPanes(state, "right", "bottom");
    if (state.root?.kind !== "split" || next.root?.kind !== "split") {
      throw new Error("Expected the custom split tree");
    }
    expect(next.root.first).toBe(state.root.first);
    expect(next.root.second).toEqual({ kind: "split", id: "rows", ratio: 0.65,
      orientation: "vertical", first: { kind: "pane", paneId: "bottom" },
      second: { kind: "pane", paneId: "right" } });
    expect(next.panes).toBe(state.panes);
    expect(next.focusedPaneId).toBe("right");
    expect(next.maximizedPaneId).toBe("right");
    expect(parseTerminalWorkspaceState(serializeTerminalWorkspaceState(next))).toEqual(next);
    expect(swapTerminalPanes(next, "right", "bottom")).toEqual(state);
  });
});
