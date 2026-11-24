import { expect, it, vi } from "vitest";
import { focusTerminalViewIfCurrent } from "./terminal-view-focus";
import { EMPTY_TERMINAL_WORKSPACE_STATE, openTerminalTab, splitTerminalPane,
  terminalPaneById } from "./terminal-workspace";
it("a delayed replay focuses only the current split, never its remounted neighbour", () => {
  const source = { focus: vi.fn() }, split = { focus: vi.fn() };
  const layout = splitTerminalPane(openTerminalTab(
    EMPTY_TERMINAL_WORKSPACE_STATE, "pane-a", "terminal-a"), {
    paneId: "pane-a", newPaneId: "pane-b", splitId: "split", terminalId: "terminal-b",
    orientation: "horizontal",
  });
  const focused = terminalPaneById(layout, layout.focusedPaneId!)?.activeTerminalId;
  focusTerminalViewIfCurrent(source, source, true, focused === "terminal-a");
  focusTerminalViewIfCurrent(split, split, true, focused === "terminal-b");
  expect(source.focus).not.toHaveBeenCalled();
  expect(split.focus).toHaveBeenCalledOnce();
});
it("a disposed generation or hidden workspace cannot steal the caret", () => {
  const old = { focus: vi.fn() }, replacement = { focus: vi.fn() };
  focusTerminalViewIfCurrent(old, replacement, true, true);
  focusTerminalViewIfCurrent(old, undefined, true, true);
  focusTerminalViewIfCurrent(replacement, replacement, false, true);
  expect(old.focus).not.toHaveBeenCalled();
  expect(replacement.focus).not.toHaveBeenCalled();
});
