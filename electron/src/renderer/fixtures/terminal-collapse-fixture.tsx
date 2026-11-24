import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import { TerminalSplitWorkspace } from "../components/TerminalWorkspace";
import { TerminalView, type TerminalViewHandle } from "../components/TerminalView";
import {
  closeTerminalTab, EMPTY_TERMINAL_WORKSPACE_STATE, openTerminalTab,
  splitTerminalPane, tidyTerminalWorkspace, toggleTerminalPaneMaximized,
  type TerminalWorkspaceState,
} from "../terminal-workspace";
import { assert, noop, untilWithin } from "./fixture-support";

const until = untilWithin({ timeoutMs: 2000 });

/** Closing every sibling must leave the same usable area as maximizing. */
export async function terminalCollapseFixture(root: Root): Promise<{
  scenarios: number; closes: number; paneHeight: number; terminalRows: number;
  workspaceSwitches: number; openedPanes: number; preservedTerminals: number;
}> {
  const handles = new Map<string, TerminalViewHandle>();
  const terminals = ["a", "b", "c", "d", "e", "f", "g", "h", "i"].map(id => ({ id, title: `Shell ${id}`,
    cwd: "/fixture/collapse", status: "running" as const }));
  let state: TerminalWorkspaceState = EMPTY_TERMINAL_WORKSPACE_STATE;
  let closes = 0;
  let scenarios = 0;
  let paneHeight = 0;
  let terminalRows = 0;
  let workspaceSwitches = 0;
  let openedPanes = 0;
  let preservedTerminals = 0;
  const render = (): void => flushSync(() => root.render(
    <div className="terminal-center" style={{ height: 420 }}>
      <TerminalSplitWorkspace state={state} terminals={terminals}
        onFocusPane={noop} onSelectTab={noop}
        onMoveTerminal={noop} onRequestSplit={noop} onSplitRatioChange={noop}
        onCloseTab={id => { state = closeTerminalTab(state, id); closes++; render(); }}
        onToggleMaximize={id => { state = toggleTerminalPaneMaximized(state, id); render(); }}
        renderTerminal={({ terminalId, active }) => <TerminalView terminalId={terminalId}
          active={active} onData={noop} onReady={handle => {
            if (handle === null) handles.delete(terminalId);
            else {
              handles.set(terminalId, handle);
              handle.write(`Surviving shell ${terminalId}\r\n`, () => handle.completeSetup());
            }
          }} />}
      />
    </div>,
  ));
  const pane = (id: string): HTMLElement => {
    const element = document.querySelector<HTMLElement>(`[data-terminal-pane-id="pane-${id}"]`);
    assert(element, `Missing pane ${id}`);
    return element;
  };
  const click = (id: string, label: string): void => {
    const button = pane(id).querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`);
    assert(button, `Missing ${label} for ${id}`);
    button.click();
  };
  // Every pane the split layout still names must be the only occupant of its
  // slot and sit inside it. Closing a sibling collapses its split, so a survivor
  // can inherit the React position of the closed pane's slot; the closed pane's
  // mount must not stay there above it.
  const checkAllVisible = (phase: string): void => {
    if (state.arrangement === "equal-grid") return;
    for (const { id } of state.panes) {
      const element = document.querySelector<HTMLElement>(`[data-terminal-pane-id="${id}"]`);
      const slot = element?.closest<HTMLElement>(".terminal-pane-slot");
      assert(element && slot, `${phase}: ${id} left the layout`);
      const box = slot.getBoundingClientRect();
      const bounds = element.getBoundingClientRect();
      assert(slot.childElementCount === 1 && bounds.height > 0
        && bounds.top >= box.top - 1 && bounds.bottom <= box.bottom + 1,
        `${phase}: ${id} is hidden by a stale mount (${slot.childElementCount} mounts, `
          + `top ${Math.round(bounds.top - box.top)}px into a ${Math.round(box.height)}px slot)`);
    }
  };
  const checkFullPane = async (id: string, phase: string): Promise<void> => {
    const workspace = document.querySelector<HTMLElement>(".terminal-workspace")!;
    const box = workspace.getBoundingClientRect();
    const style = getComputedStyle(workspace);
    const left = box.left + parseFloat(style.paddingLeft);
    const top = box.top + parseFloat(style.paddingTop);
    const width = box.width - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
    const height = box.height - parseFloat(style.paddingTop) - parseFloat(style.paddingBottom);
    await until(() => {
      const bounds = pane(id).getBoundingClientRect();
      return Math.abs(bounds.left - left) < 1 && Math.abs(bounds.top - top) < 1
        && Math.abs(bounds.width - width) < 1 && Math.abs(bounds.height - height) < 1;
    }, `${phase}: remaining pane does not fill the workspace (${pane(id).getBoundingClientRect().height}px vs ${height}px)`);
    await until(() => {
      const body = pane(id).querySelector(".terminal-pane-body")!.getBoundingClientRect();
      const screen = pane(id).querySelector(".xterm-screen")?.getBoundingClientRect();
      const size = handles.get(id)?.size();
      return body.height > 300 && screen !== undefined && screen.height > body.height - 50
        && screen.height <= body.height && (size?.rows ?? 0) > 10
        && handles.get(id)!.readLines().some(line => line.includes(`Surviving shell ${id}`));
    }, `${phase}: remaining terminal is blank or did not fit its available height`);
    paneHeight = pane(id).getBoundingClientRect().height;
    terminalRows = handles.get(id)!.size()!.rows;
  };
  const checkVisiblePanes = async (phase: string): Promise<void> => {
    const workspace = document.querySelector<HTMLElement>(".terminal-workspace")!;
    const area = workspace.getBoundingClientRect();
    for (const item of state.panes) {
      const id = item.activeTerminalId!;
      await until(() => handles.get(id)?.readLines().some(line =>
        line.includes(`Surviving shell ${id}`)) === true, `${phase}: ${id} has no terminal output`);
      const element = pane(id);
      const box = element.getBoundingClientRect();
      assert(box.width > 100 && box.height > 80 && box.left >= area.left - 1
        && box.top >= area.top - 1 && box.right <= area.right + 1 && box.bottom <= area.bottom + 1,
      `${phase}: pane ${id} is outside its workspace (${box.y}px, workspace bottom ${area.bottom}px)`);
      const slot = element.closest(".terminal-pane-slot")!;
      assert(slot.children.length === 1, `${phase}: stale mounts occupy a pane slot`);
    }
    assert(workspace.querySelectorAll(".terminal-pane-mount").length === state.panes.length,
      `${phase}: workspace retained obsolete pane mounts`);
  };
  try {
    for (const arrangement of ["horizontal", "vertical", "equal-grid", "nested"] as const) {
      const ids = arrangement === "nested" ? ["a", "b", "c"] : ["a", "b"];
      for (const survivor of ids) {
        flushSync(() => root.render(null));
        state = openTerminalTab(EMPTY_TERMINAL_WORKSPACE_STATE, "pane-a", "a");
        state = splitTerminalPane(state, { paneId: "pane-a", newPaneId: "pane-b",
          splitId: "split-ab", terminalId: "b", ratio: 0.35,
          orientation: arrangement === "vertical" ? "vertical" : "horizontal" });
        if (arrangement === "nested") state = splitTerminalPane(state, {
          paneId: "pane-b", newPaneId: "pane-c", splitId: "split-bc",
          terminalId: "c", orientation: "vertical", ratio: 0.65,
        });
        if (arrangement === "equal-grid") state = tidyTerminalWorkspace(state, index => `tidy-${index}`);
        render();
        await until(() => ids.every(id => handles.has(id)), "Split terminals did not mount");
        for (const id of ids.filter(id => id !== survivor)) {
          click(id, "Close active terminal (ends it)");
          checkAllVisible(`${arrangement}, survivor ${survivor}, closed ${id}`);
        }
        assert(state.panes.length === 1 && state.maximizedPaneId === null,
          "Closing siblings should leave one ordinary pane");
        const phase = `${arrangement}, survivor ${survivor}`;
        await checkFullPane(survivor, phase);
        const rows = terminalRows;
        click(survivor, "Maximize terminal pane");
        await checkFullPane(survivor, `${phase}, maximized`);
        assert(terminalRows === rows, "Maximizing the last pane changed its fitted rows");
        click(survivor, "Restore terminal layout");
        await checkFullPane(survivor, `${phase}, restored`);
        assert(terminalRows === rows, "Restoring the last pane changed its fitted rows");
        scenarios++;
      }
    }
    // Reuse the mounted split renderer, as the real workspace switcher does.
    // Replacing it with root.render(null) would conceal abandoned portal mounts.
    for (const arrangement of ["horizontal", "vertical", "nested"] as const) {
      flushSync(() => root.render(null));
      for (let turn = 0; turn < 4; turn++) {
        const ids = turn % 2 === 0 ? ["a", "b", "c", "d"] : ["e", "f", "g", "h"];
        state = openTerminalTab(EMPTY_TERMINAL_WORKSPACE_STATE, `pane-${ids[0]}`, ids[0]);
        state = splitTerminalPane(state, { paneId: `pane-${ids[0]}`, newPaneId: `pane-${ids[1]}`,
          splitId: "workspace-columns", terminalId: ids[1],
          orientation: arrangement === "vertical" ? "vertical" : "horizontal" });
        if (arrangement === "nested") {
          for (let side = 0; side < 2; side++) state = splitTerminalPane(state, {
            paneId: `pane-${ids[side]}`, newPaneId: `pane-${ids[side + 2]}`,
            splitId: `workspace-rows-${side}`, terminalId: ids[side + 2], orientation: "vertical",
          });
        }
        render();
        const phase = `${arrangement} workspace ${turn}`;
        await checkVisiblePanes(phase);
        if (turn > 0) workspaceSwitches++;
        const originals = new Map(handles);
        state = splitTerminalPane(state, { paneId: `pane-${ids[0]}`, newPaneId: "pane-i",
          splitId: "new-pane", terminalId: "i", orientation: "horizontal" });
        render();
        await checkVisiblePanes(`${phase}, new pane`);
        openedPanes++;
        click("i", "Close active terminal (ends it)");
        await checkVisiblePanes(`${phase}, closed pane`);
        for (const [id, handle] of originals) {
          assert(handles.get(id) === handle, `${phase}: opening a pane remounted terminal ${id}`);
          preservedTerminals++;
        }
      }
    }
    return { scenarios, closes, paneHeight, terminalRows, workspaceSwitches, openedPanes, preservedTerminals };
  } finally {
    flushSync(() => root.render(null));
  }
}
