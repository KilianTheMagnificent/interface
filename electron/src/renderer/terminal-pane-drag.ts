import {
  moveTerminalTab, swapTerminalPanes, terminalPaneForTerminal,
  type TerminalId, type TerminalPaneId, type TerminalWorkspaceState,
} from "./terminal-workspace";

/** A header drag exchanges whole panes; only a drop in the same pane reorders tabs. */
export function dropTerminalPane(state: TerminalWorkspaceState, terminalId: TerminalId,
    targetPaneId: TerminalPaneId, targetIndex?: number): TerminalWorkspaceState {
  const source = terminalPaneForTerminal(state, terminalId);
  if (source === undefined) return state;
  return source.id === targetPaneId
    ? moveTerminalTab(state, terminalId, targetPaneId, targetIndex)
    : swapTerminalPanes(state, source.id, targetPaneId);
}
