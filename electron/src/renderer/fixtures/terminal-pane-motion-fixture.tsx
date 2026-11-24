import { FitAddon } from "@xterm/addon-fit";
import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import { TerminalSplitWorkspace } from "../components/TerminalWorkspace";
import { TerminalView, type TerminalViewHandle } from "../components/TerminalView";
import { dropTerminalPane } from "../terminal-pane-drag";
import { TERMINAL_DRAG_FIT_INTERVAL_MS, TERMINAL_LAYOUT_DRAGGING } from "../terminal-pane-motion";
import { EMPTY_TERMINAL_WORKSPACE_STATE, closeTerminalTab, focusTerminalPane, openTerminalTab,
  setTerminalSplitRatio, splitTerminalPane, splitTerminalPaneInPlace, tidyTerminalWorkspace, toggleTerminalPaneMaximized,
  type TerminalWorkspaceState } from "../terminal-workspace";
import { assert, frame, noop } from "./fixture-support";

const near = (a: number, b: number): boolean => Math.abs(a - b) < 2;
const animations = (): Animation[] => document.getAnimations().filter(a => a.id === "terminal-pane-motion");
async function finish(): Promise<void> {
  for (const animation of animations()) animation.finish();
  await frame(); await frame();
}
function sample(progress: number): void {
  for (const animation of animations()) {
    animation.pause();
    animation.currentTime = Number(animation.effect!.getTiming().duration) * progress;
  }
}

/** Real emulators and DOM geometry, with animation time controlled explicitly.
 * No providers or user's processes. Input/buffers and refit count are acceptance. */
export async function terminalPaneMotionFixture(root: Root, hold = false): Promise<unknown> {
  const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;
  const handles = new Map<string, TerminalViewHandle>();
  const writes: string[] = [];
  let refits = 0, mounts = 0, scenarios = 0;
  let ghostCanvases: HTMLCanvasElement[] = [], exitCanvases = 0;
  // The last size each emulator reported for its PTY.
  const reported = new Map<string, { columns: number; rows: number }>();
  const drags: { frames: number; ms: number; fits: number; reflows: number; emulators: number }[] = [];
  let state: TerminalWorkspaceState = EMPTY_TERMINAL_WORKSPACE_STATE;
  const ids = ["a", "b", "c", "d", "e"];
  const terminals = ids.map(id => ({ id, title: `Pane ${id.toUpperCase()} — motion study`,
    profileId: "codex", cwd: "/fixture/motion", status: "running" as const }));
  const render = (): void => flushSync(() => root.render(
    <div className="terminal-center" style={{ height: 600 }}>
      <TerminalSplitWorkspace state={state} terminals={terminals}
        onSelectTab={noop} onCloseTab={id => { state = closeTerminalTab(state, id); render(); }}
        onRequestSplit={noop} onMoveTerminal={(id, target) => { state = dropTerminalPane(state, id, target); render(); }}
        onFocusPane={id => { state = focusTerminalPane(state, id); }}
        onSplitRatioChange={(id, ratio) => { state = setTerminalSplitRatio(state, id, ratio); render(); }}
        onToggleMaximize={id => { state = toggleTerminalPaneMaximized(state, id); render(); }}
        renderTerminal={({ terminalId, active }) => <TerminalView terminalId={terminalId} active={active}
          onData={text => writes.push(`${terminalId}:${text}`)}
          onResize={size => { refits++; reported.set(terminalId, size); }}
          onReady={handle => {
            if (!handle) { handles.delete(terminalId); return; }
            mounts++; handles.set(terminalId, handle);
            handle.write(`\r\n  Buffer for ${terminalId}\r\n\r\n  $ cargo check\r\n  Checking workspace...\r\n  Finished successfully.\r\n\r\n  > Ready for your next task\r\n`, () => handle.completeSetup());
          }} />}
      />
    </div>,
  ));
  const pane = (id: string): HTMLElement => {
    const node = document.querySelector<HTMLElement>(`[data-terminal-pane-id="${id}"]`);
    assert(node, `Missing ${id}`); return node;
  };
  const box = (id: string): DOMRect => pane(id).parentElement!.getBoundingClientRect();
  /** A real pointer drag of one divider, a move per frame. Both sides refit on
   * a trailing throttle rather than every frame, then land on exactly the
   * final geometry, which is also the last size reported for their PTYs. */
  const dragDivider = async (splitId: string): Promise<void> => {
    const split = document.querySelector<HTMLElement>(`[data-terminal-split-id="${splitId}"]`);
    const bar = split?.querySelector<HTMLElement>(":scope > .terminal-splitter");
    assert(split && bar, `Missing the ${splitId} divider`);
    // A synthetic pointer cannot be captured; the drag's window listeners carry it.
    bar.setPointerCapture = noop; bar.releasePointerCapture = noop; bar.hasPointerCapture = () => false;
    const emulators = [...split.querySelectorAll<HTMLElement>("[data-terminal-pane-id]")]
      .map(node => node.dataset.terminalPaneId!).filter(id => handles.has(id));
    assert(emulators.length >= 2, `The ${splitId} divider has ${emulators.length} emulators beside it`);
    // Reduced Motion shortens every transition to 0.01ms rather than removing
    // it, so geometry reaches layout a frame after its style: let it settle.
    for (let paint = 0; paint < 4; paint++) await frame();
    const columns = split.classList.contains("is-columns");
    const grip = bar.getBoundingClientRect();
    const x = grip.left + grip.width / 2, y = grip.top + grip.height / 2;
    const pointer = (type: string, offset: number, buttons: number): PointerEvent => new PointerEvent(type, {
      bubbles: true, cancelable: true, isPrimary: true, pointerId: 41, button: 0, buttons,
      clientX: columns ? x - offset : x, clientY: columns ? y : y - offset });
    const fit = FitAddon.prototype.fit;
    let fits = 0;
    FitAddon.prototype.fit = function (this: FitAddon) { fits++; fit.call(this); };
    try {
      flushSync(() => bar.dispatchEvent(pointer("pointerdown", 0, 1)));
      assert(split.hasAttribute(TERMINAL_LAYOUT_DRAGGING), "A divider drag did not throttle its emulators");
      const reflowsBefore = refits;
      const started = performance.now();
      const frames = 24;
      for (let step = 1; step <= frames; step++) {
        window.dispatchEvent(pointer("pointermove", step * 5, 1));
        await frame();
      }
      const ms = performance.now() - started;
      const dragFits = fits, reflows = refits - reflowsBefore;
      window.dispatchEvent(pointer("pointerup", frames * 5 + 3, 0));
      assert(!split.hasAttribute(TERMINAL_LAYOUT_DRAGGING), "The drag left its emulators throttled");
      for (let paint = 0; paint < 4; paint++) await frame();
      const bound = emulators.length * (Math.ceil(ms / TERMINAL_DRAG_FIT_INTERVAL_MS) + 1);
      assert(dragFits <= bound,
        `A divider drag fitted ${dragFits} times over ${frames} frames in ${Math.round(ms)} ms (at most ${bound})`);
      drags.push({ frames, ms: Math.round(ms), fits: dragFits, reflows, emulators: emulators.length });
      const settled = refits;
      for (const id of emulators) {
        const handle = handles.get(id)!;
        handle.fit();
        assert(refits === settled, `${id} ended the drag short of its final geometry`);
        const size = handle.size(), sent = reported.get(id);
        assert(size !== null && sent?.columns === size.columns && sent.rows === size.rows,
          `${id} reported ${JSON.stringify(sent)} for its PTY, not its final ${JSON.stringify(size)}`);
      }
    } finally { FitAddon.prototype.fit = fit; }
  };
  try {
    // Open a fourth pane from three panes in one row: it takes half of its
    // source, and the other two neither move nor resize.
    state = openTerminalTab(EMPTY_TERMINAL_WORKSPACE_STATE, "a", "a");
    state = splitTerminalPane(state, { paneId: "a", newPaneId: "b", terminalId: "b",
      orientation: "horizontal", splitId: "row-ab", ratio: 1 / 3 });
    state = splitTerminalPane(state, { paneId: "b", newPaneId: "c", terminalId: "c",
      orientation: "horizontal", splitId: "row-bc" });
    render(); await frame(); await frame(); await finish();
    assert(near(box("a").y, box("b").y) && near(box("b").y, box("c").y), "Expected three panes in one row");
    const beforeAdd = new Map(handles), beforeAddMounts = mounts;
    const [a0, b0, c0] = ["a", "b", "c"].map(box);
    const row = state;
    const halve = (): typeof state => splitTerminalPaneInPlace(row, { paneId: "c", newPaneId: "d",
      terminalId: "d", splitId: "in-place", orientation: "vertical" }, index => `in-place-tidy-${index}`);
    state = halve(); render();
    if (!reduced) {
      assert(animations().length > 0, "The halved pane did not animate");
      // React may run the layout updater again; the same layout rebuilt must
      // neither replay nor cut short the motion its first commit started.
      const playing = animations().length;
      state = halve(); render();
      assert(animations().length === playing && !document.querySelector(".terminal-motion-ghost"),
        "A rebuilt identical layout restarted the pane motion");
    }
    await finish();
    const [a, b, c, d] = ["a", "b", "c", "d"].map(box);
    assert([[a, a0], [b, b0]].every(([now, was]) => near(now.x, was.x) && near(now.y, was.y)
      && near(now.width, was.width) && near(now.height, was.height)), "Opening a pane moved its neighbours");
    assert(near(c.x, c0.x) && near(c.y, c0.y) && near(c.width, c0.width) && near(d.x, c0.x)
      && near(d.width, c0.width) && d.y > c.bottom - 1 && near(d.bottom, c0.bottom),
      "The new pane did not take half of its source");
    assert(state.focusedPaneId === "d" && mounts === beforeAddMounts + 1, "Halving lost focus or remounted terminals");
    for (const [id, handle] of beforeAdd) {
      assert(handles.get(id) === handle, `Halving remounted ${id}`);
      assert(handle.readLines().some(line => line.includes(`Buffer for ${id}`)), `Halving lost ${id} buffer`);
      handle.focus();
      const inputCount = writes.length;
      document.activeElement!.dispatchEvent(new KeyboardEvent("keydown", { key: "x", code: "KeyX", keyCode: 88, bubbles: true, cancelable: true }));
      assert(writes.length === inputCount + 1 && writes.at(-1) === `${id}:x`, `Halving misrouted ${id} input`);
    }
    for (const arrangement of ["equal-grid", "split-tree"] as const) {
      flushSync(() => root.render(null));
      state = openTerminalTab(EMPTY_TERMINAL_WORKSPACE_STATE, "a", "a");
      state = splitTerminalPane(state, { paneId: "a", newPaneId: "b", terminalId: "b", orientation: "horizontal", splitId: "ab" });
      state = splitTerminalPane(state, { paneId: "a", newPaneId: "c", terminalId: "c", orientation: "vertical", splitId: "ac" });
      state = splitTerminalPane(state, { paneId: "b", newPaneId: "d", terminalId: "d", orientation: "vertical", splitId: "bd" });
      if (arrangement === "equal-grid") state = tidyTerminalWorkspace(state, index => `tidy-${index}`);
      render(); await frame(); await frame(); await finish();
      const original = new Map(handles), mountCount = mounts;
      const originalBox = box("a"), destination = box("b");
      handles.get("a")!.focus();
      const focused = document.activeElement;
      state = dropTerminalPane(state, "a", "b"); render();
      if (!reduced) {
        assert(animations().length >= 2, `${arrangement}: swap has no motion`);
        sample(0);
        assert(near(box("a").x, originalBox.x) && near(box("a").y, originalBox.y), "Swap jumped at start");
        sample(0.25);
        assert(!near(box("a").x, originalBox.x) || !near(box("a").y, originalBox.y), "Swap did not move");
      } else assert(animations().length === 0, "Reduced motion still animates");
      await finish();
      assert(near(box("a").x, destination.x) && near(box("a").y, destination.y), "Swap missed destination");
      assert(document.activeElement === focused, "Swap lost terminal focus");
      const restoreBox = box("a");
      pane("a").querySelector<HTMLButtonElement>('[aria-label="Maximize terminal pane"]')!.click();
      if (!reduced) {
        assert(animations().length >= 1, "Maximize has no animation");
        sample(0);
        assert(near(box("a").width, restoreBox.width), "Maximize jumped to final width");
        await frame();
        const body = pane("a").querySelector<HTMLElement>(".terminal-pane-body")!;
        const bodyWidth = body.getBoundingClientRect().width, bodyHeight = body.getBoundingClientRect().height;
        const fitted = refits;
        sample(0.35); await frame();
        sample(0.7); await frame();
        assert(near(body.getBoundingClientRect().width, bodyWidth)
          && near(body.getBoundingClientRect().height, bodyHeight), "Terminal text backing changed during animation");
        assert(refits === fitted, `Terminal refit during animation (${fitted} -> ${refits})`);
        handles.get("a")!.focus();
        const inputCount = writes.length;
        document.activeElement!.dispatchEvent(new KeyboardEvent("keydown", { key: "x", code: "KeyX", keyCode: 88, bubbles: true, cancelable: true }));
        assert(writes.length === inputCount + 1 && writes.at(-1) === "a:x", "Typing during motion did not reach the same terminal");
        // Reverse before completion: capture the displayed geometry, not the old target.
        const midway = box("a");
        pane("a").querySelector<HTMLButtonElement>('[aria-label="Restore terminal layout"]')!.click();
        sample(0);
        assert(near(box("a").width, midway.width) && near(box("a").x, midway.x), "Interrupted restore jumped");
      } else {
        assert(animations().length === 0, "Reduced maximize animates");
        state = toggleTerminalPaneMaximized(state, "a"); render();
      }
      await finish();
      assert(near(box("a").width, restoreBox.width) && near(box("a").y, restoreBox.y), "Restore missed slot");
      for (const [id, handle] of original) {
        assert(handles.get(id) === handle, `Remounted terminal ${id}`);
        assert(handle.readLines().some(line => line.includes(`Buffer for ${id}`)), `Lost ${id} buffer`);
      }
      assert(mounts === mountCount, "Motion remounted an emulator");
      state = focusTerminalPane(state, "b"); render();
      assert(animations().length === 0, "Focus started layout motion");
      if (arrangement === "split-tree" && state.root?.kind === "split") {
        const rootSplit = state.root.id;
        state = setTerminalSplitRatio(state, rootSplit, 0.6); render();
        assert(animations().length === 0, "Divider resize was animated");
        await dragDivider(rootSplit);
        assert(animations().length === 0, "Divider drag was animated");
      }
      // Add and remove a real terminal: survivors move, exits cannot receive input.
      state = splitTerminalPane(state, { paneId: "a", newPaneId: "e", terminalId: "e", orientation: "vertical", splitId: "ae" }); render();
      if (!reduced) assert(animations().length > 0, "Added pane did not animate");
      await finish();
      state = closeTerminalTab(state, "e"); render();
      if (!reduced) {
        const ghosts = [...document.querySelectorAll<HTMLElement>(".terminal-motion-ghost")];
        assert(ghosts.length > 0 && ghosts.every(node => node.inert && node.getAttribute("aria-hidden") === "true"), "Exit visual is interactive");
        assert(ghosts.every(node => !node.querySelector("[id], [data-terminal-pane-id]")), "Exit copied terminal identity");
        // Only pixels are copied: no blank link layer, no accessibility tree.
        assert(ghosts.every(node => !node.querySelector(".xterm-link-layer, .xterm-accessibility")),
          "Exit copied a layer it never shows");
        ghostCanvases = ghosts.flatMap(node => [...node.querySelectorAll("canvas")]);
        exitCanvases += ghostCanvases.length;
      }
      await finish();
      assert(!document.querySelector(".terminal-motion-ghost, [data-pane-motion-active], [data-pane-motion-tile]"), "Motion left stale DOM");
      // A removed ghost frees its canvas backing at once, not at garbage collection.
      assert(ghostCanvases.every(canvas => canvas.width === 0 && canvas.height === 0),
        "A finished exit kept its canvas backing");
      ghostCanvases = [];
      for (const [id, handle] of original) assert(handles.get(id) === handle, `Reflow remounted ${id}`);
      scenarios++;
      if (hold && arrangement === "split-tree" && !reduced) {
        state = toggleTerminalPaneMaximized(state, "b"); render();
        for (let paint = 0; paint < 4; paint++) await frame();
        sample(0.25);
        return { scenarios, reduced, refits, mounts, held: true };
      }
    }
    // Disposal in flight must clear every animation, ghost and timer-owned style.
    state = toggleTerminalPaneMaximized(state, "b"); render();
    flushSync(() => root.render(null));
    assert(animations().length === 0 && !document.querySelector(".terminal-motion-ghost"), "Unmount leaked animation");
    return { scenarios, splitInPlace: true, reduced, refits, mounts, input: writes.length, preserved: true, drags,
      exitCanvases };
  } finally { if (!hold) flushSync(() => root.render(null)); }
}
