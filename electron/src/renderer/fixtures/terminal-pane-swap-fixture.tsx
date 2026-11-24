import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import { TerminalSplitWorkspace } from "../components/TerminalWorkspace";
import { TerminalView, type TerminalViewHandle } from "../components/TerminalView";
import { dropTerminalPane } from "../terminal-pane-drag";
import {
  EMPTY_TERMINAL_WORKSPACE_STATE, focusTerminalPane, openTerminalTab,
  parseTerminalWorkspaceState, serializeTerminalWorkspaceState, setTerminalSplitRatio,
  splitTerminalPane, terminalWorkspacePaneOrder, tidyTerminalWorkspace,
  toggleTerminalPaneMaximized, type TerminalWorkspaceState,
} from "../terminal-workspace";
import { assert, frames as settled, noop } from "./fixture-support";

/** Real header events must exchange occupied positions without merging or disposing xterm. */
export async function terminalPaneSwapFixture(root: Root, holdPreview = false): Promise<{
  scenarios: number; swaps: number; preservedTerminals: number;
}> {
  const handles = new Map<string, TerminalViewHandle>();
  const ids = ["a", "a2", "b", "b2", "c", "d", "e", "f"];
  const terminals = ids.map(id => ({ id, title: holdPreview
    ? (id.startsWith("a") ? "Review terminal workspace changes and release checks" : "Check deployment status")
    : `Shell ${id}`, profileId: id.startsWith("a") ? "claude" : "codex",
    activity: "working" as const, cwd: "/fixture/swap", status: "running" as const }));
  let state: TerminalWorkspaceState = EMPTY_TERMINAL_WORKSPACE_STATE;
  let swaps = 0;
  let scenarios = 0;
  let preservedTerminals = 0;
  const render = (): void => flushSync(() => root.render(
    <div className="terminal-center" style={{ height: 600 }}>
      <TerminalSplitWorkspace state={state} terminals={terminals}
        onSelectTab={noop} onCloseTab={noop} onRequestSplit={noop}
        onFocusPane={id => { state = focusTerminalPane(state, id); }}
        onSplitRatioChange={(id, ratio) => {
          state = setTerminalSplitRatio(state, id, ratio); render();
        }}
        onToggleMaximize={id => { state = toggleTerminalPaneMaximized(state, id); render(); }}
        onMoveTerminal={(id, target, index) => {
          state = dropTerminalPane(state, id, target, index); render();
        }}
        renderTerminal={({ terminalId, active }) => <TerminalView terminalId={terminalId}
          active={active} onData={noop} onReady={handle => {
            if (handle === null) handles.delete(terminalId);
            else {
              handles.set(terminalId, handle);
              handle.write(holdPreview
                ? "\r\n  Reviewing workspace changes\r\n\r\n  Workspace and release checks completed.\r\n  All terminal sessions remain connected.\r\n\r\n  > Ready for your next task\r\n"
                : `Buffer for ${terminalId}\r\n`, () => handle.completeSetup());
            }
          }} />}
      />
    </div>,
  ));
  const pane = (id: string): HTMLElement => {
    const node = document.querySelector<HTMLElement>(`[data-terminal-pane-id="pane-${id}"]`);
    assert(node, `Missing pane ${id}`);
    return node;
  };
  const beginDrag = async (source: Element, target: Element): Promise<DataTransfer> => {
    const dataTransfer = new DataTransfer();
    assert(source.dispatchEvent(new DragEvent("dragstart", {
      bubbles: true, cancelable: true, dataTransfer,
    })), "Header drag was unexpectedly cancelled");
    await settled();
    const box = target.closest(".terminal-pane")!.getBoundingClientRect();
    // Chromium protects the payload during hover: only types are readable.
    const protectedTransfer = new DataTransfer();
    protectedTransfer.setData("application/x-interface-terminal-tab", "");
    target.dispatchEvent(new DragEvent("dragover", { bubbles: true, cancelable: true,
      dataTransfer: protectedTransfer, clientX: box.x + box.width / 2, clientY: box.y + box.height / 2 }));
    await settled();
    // Hidden Chromium can park reduced-motion's 0.01ms transitions at time 0.
    for (const animation of document.getAnimations()) if (animation instanceof CSSTransition) animation.finish();
    await settled();
    const badge = document.querySelector<HTMLElement>(".terminal-pane-drag-preview");
    assert(badge && getComputedStyle(badge).pointerEvents === "none", "Drag badge missing or intercepting drops");
    const samePane = source.closest(".terminal-pane") === target.closest(".terminal-pane");
    assert(badge.classList.contains("is-swap") !== samePane, "Swap label shown for the wrong target");
    assert(target.closest(".terminal-pane")?.classList.contains("is-drop-target") !== samePane,
      "Swap highlight shown for the wrong target");
    if (!samePane) {
      const overlay = getComputedStyle(target.closest(".terminal-pane")!, "::after");
      assert(overlay.borderWidth === "2px" && overlay.backgroundColor !== "rgba(0, 0, 0, 0)",
        `Swap overlay not painted: ${overlay.borderWidth}, ${overlay.backgroundColor}`);
    }
    const badgeBox = badge.getBoundingClientRect();
    assert(badgeBox.left >= 0 && badgeBox.right <= innerWidth && badgeBox.bottom <= innerHeight,
      "Drag badge escaped the viewport");
    return dataTransfer;
  };
  const drag = async (source: Element, target: Element): Promise<void> => {
    const dataTransfer = await beginDrag(source, target);
    target.dispatchEvent(new DragEvent("drop", { bubbles: true, cancelable: true, dataTransfer }));
    source.dispatchEvent(new DragEvent("dragend", { bubbles: true, dataTransfer }));
    await settled();
    assert(!document.querySelector(".terminal-pane-drag-preview, .terminal-pane.is-drop-target"),
      "Drag feedback survived a drop");
  };
  const bounds = (id: string): number[] => {
    const box = pane(id).getBoundingClientRect();
    return [box.x, box.y, box.width, box.height];
  };
  const near = (actual: number[], expected: number[]): boolean =>
    actual.every((value, index) => Math.abs(value - expected[index]) < 1);
  try {
    for (const arrangement of ["grid-2", "grid-3", "grid-6", "nested"] as const) {
      flushSync(() => root.render(null));
      state = openTerminalTab(EMPTY_TERMINAL_WORKSPACE_STATE, "pane-a", "a");
      state = splitTerminalPane(state, { paneId: "pane-a", newPaneId: "pane-b",
        splitId: "split-ab", terminalId: "b", orientation: "horizontal", ratio: 0.35 });
      const paneIds = arrangement === "grid-2" ? ["a", "b"]
        : arrangement === "grid-6" ? ["a", "b", "c", "d", "e", "f"] : ["a", "b", "c"];
      for (const id of paneIds.slice(2)) state = splitTerminalPane(state, {
        paneId: "pane-b", newPaneId: `pane-${id}`, splitId: `split-b${id}`,
        terminalId: id, orientation: "vertical", ratio: 0.65,
      });
      state = openTerminalTab(state, "pane-a", "a2");
      state = openTerminalTab(state, "pane-b", "b2");
      if (arrangement !== "nested") state = tidyTerminalWorkspace(state, index => `tidy-${index}`);
      state = focusTerminalPane(state, "pane-a");
      render();
      await settled();
      const originals = new Map(handles);
      const originalPanes = state.panes;
      const sourceBox = bounds("a");
      const destination = paneIds.at(-1)!;
      const targetBox = bounds(destination);
      const order = terminalWorkspacePaneOrder(state);
      // Drag the title of an inactive tab; its whole pane (including a2) must move.
      const title = pane("a").querySelector('[data-terminal-id="a"] .terminal-tab-title')!;
      if (holdPreview) {
        await beginDrag(title, pane(destination).querySelector(".xterm-screen")!);
        return { scenarios: 1, swaps: 0, preservedTerminals: handles.size };
      }
      // Leaving, Escape and outside drops all clear the feedback without swapping.
      for (const ending of ["leave", "escape", "outside-drop", "blur"]) {
        const transfer = await beginDrag(title, pane(destination));
        if (ending === "leave") {
          pane(destination).dispatchEvent(new DragEvent("dragleave", { bubbles: true, dataTransfer: transfer }));
        } else if (ending === "escape") window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
        else window.dispatchEvent(new Event(ending === "blur" ? "blur" : "drop"));
        await settled();
        assert(!document.querySelector(".terminal-pane.is-drop-target"), `${ending}: stale target highlight`);
        if (ending !== "leave") assert(!document.querySelector(".terminal-pane-drag-preview"), `${ending}: stale badge`);
        title.dispatchEvent(new DragEvent("dragend", { bubbles: true, dataTransfer: transfer }));
        assert(state.panes === originalPanes, `${ending}: changed the layout`);
      }
      const foreign = new DataTransfer();
      foreign.setData("text/plain", "unrelated drag");
      pane(destination).dispatchEvent(new DragEvent("dragover", { bubbles: true, cancelable: true, dataTransfer: foreign }));
      await settled();
      assert(!document.querySelector(".terminal-pane.is-drop-target"), "Unrelated drag showed swap feedback");
      await drag(title, pane(destination).querySelector(".xterm-screen")!);
      await settled();
      assert(near(bounds("a"), targetBox) && near(bounds(destination), sourceBox),
        `${arrangement}: occupied rectangles did not swap`);
      assert(state.panes === originalPanes, `${arrangement}: a swap changed tab membership`);
      assert(state.focusedPaneId === "pane-a", `${arrangement}: focus did not follow source identity`);
      const expected = order.map(id => id === "pane-a" ? `pane-${destination}`
        : id === `pane-${destination}` ? "pane-a" : id);
      assert(JSON.stringify(terminalWorkspacePaneOrder(state)) === JSON.stringify(expected),
        `${arrangement}: wrong pane order`);
      assert(JSON.stringify(parseTerminalWorkspaceState(serializeTerminalWorkspaceState(state)))
        === JSON.stringify(state), `${arrangement}: saved layout did not roundtrip`);
      swaps++;
      // The blank part of the header also swaps, including a drop over a target tab.
      await drag(pane("a").querySelector(".terminal-pane-head")!,
        pane(destination).querySelector(".terminal-tab")!);
      await settled();
      assert(near(bounds("a"), sourceBox) && near(bounds(destination), targetBox),
        `${arrangement}: header drag did not swap back`);
      swaps++;
      const beforeSelf = state;
      await drag(pane("a").querySelector(".terminal-pane-head")!, pane("a"));
      assert(state === beforeSelf, `${arrangement}: self drop changed the pane`);
      // A tab reorder stays inside its pane and keeps both panes separate.
      await drag(pane("a").querySelector('[data-terminal-id="a"]')!, pane("a"));
      assert(state.panes.find(value => value.id === "pane-a")!.tabs.map(tab => tab.terminalId).join()
        === "a2,a", `${arrangement}: same-pane reorder stopped working`);
      const button = pane("a").querySelector<HTMLButtonElement>('[aria-label="Maximize terminal pane"]')!;
      button.click();
      await settled();
      assert(pane("a").getBoundingClientRect().height > 500, "Swapped pane could not maximize");
      pane("a").querySelector<HTMLButtonElement>('[aria-label="Restore terminal layout"]')!.click();
      await settled();
      for (const [id, handle] of originals) {
        assert(handles.get(id) === handle, `${arrangement}: terminal ${id} was remounted`);
        assert(handle.readLines().some(line => line.includes(`Buffer for ${id}`)),
          `${arrangement}: terminal ${id} lost its buffer`);
        preservedTerminals++;
      }
      assert(document.querySelectorAll("[data-terminal-pane-id]").length === paneIds.length,
        `${arrangement}: panes merged or duplicated`);
      scenarios++;
    }
    return { scenarios, swaps, preservedTerminals };
  } finally {
    if (!holdPreview) flushSync(() => root.render(null));
  }
}
