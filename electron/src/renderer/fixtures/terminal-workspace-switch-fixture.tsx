import { WebglAddon } from "@xterm/addon-webgl";
import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import type { TerminalSummary, TerminalWorkspaceRecord } from "../../shared/ipc";
import { IntegratedTerminalCenter, type TerminalRendererApi } from "../components/IntegratedTerminalCenter";
import type { TerminalWorkspaceSwitcher } from "../components/TerminalWorkspace";
import { attachTerminalsBalanced, EMPTY_TERMINAL_WORKSPACE_STATE, serializeTerminalWorkspaceState } from "../terminal-workspace";
import {
  TERMINAL_WEBGL_IDLE_RELEASE_MS, TERMINAL_WEBGL_LIMIT, terminalWebglPool, type TerminalWebglClock,
} from "../terminal-webgl-pool";
import { TERMINAL_ANNOUNCEMENT_LIMIT } from "../terminal-announcements";
import { assert, frames, noop, wait } from "./fixture-support";

/** Full workspace switches, real xterm/GPU renderers, synthetic provider bytes. */
export async function terminalWorkspaceSwitchFixture(root: Root, panesPerWorkspace = 4,
    recoverLostContext = false): Promise<Record<string, unknown>> {
  const stamp = "2026-09-23T00:00:00Z";
  const terminals: TerminalSummary[] = Array.from({ length: 5 * panesPerWorkspace }, (_, index) => ({
    id: `00000000-0000-4000-8000-${String(1201 + index).padStart(12, "0")}`,
    generation: 1, title: `Switch ${index}`, root: "/fixture/switch", cwd: "/fixture/switch",
    state: "running", size: { columns: 80, rows: 24 },
    launch: { version: 1, profileId: "codex" }, createdAt: stamp, updatedAt: stamp,
  }));
  const records: TerminalWorkspaceRecord[] = Array.from({ length: 5 }, (_, index) => {
    const members = terminals.slice(index * panesPerWorkspace, (index + 1) * panesPerWorkspace);
    return { id: `switch-workspace-${index}`, name: `Switch ${index}`, createdAt: stamp, updatedAt: stamp,
      terminals: members, layout: serializeTerminalWorkspaceState(attachTerminalsBalanced(
        EMPTY_TERMINAL_WORKSPACE_STATE, members.map(terminal => terminal.id),
        pane => `switch-pane-${index}-${pane}`, split => `switch-split-${index}-${split}`)) };
  });
  let switcher: TerminalWorkspaceSwitcher | null = null;
  let receive: Parameters<TerminalRendererApi["onTerminalData"]>[0] | undefined;
  let outstanding = 0;
  const errors: string[] = [];
  const inputs: { id: string; generation: number; text: string }[] = [];
  const encoder = new TextEncoder();
  const emit = (index: number, text: string): void => {
    outstanding++;
    receive!({ kind: "data", terminalId: terminals[index].id, generation: 1, sequence: "0",
      data: encoder.encode(text) }, () => { outstanding--; });
  };
  const unexpected = (): never => { throw new Error("Switch fixture attempted an unrelated operation"); };
  const api: TerminalRendererApi = {
    listTerminals: async () => ({ revision: 1, terminals }), listTerminalLaunchProfiles: async () => [],
    getTerminalWorkspaces: async () => records, setTerminalWorkspaces: async value => value,
    onTerminalData: callback => { receive = callback; return noop; }, onTerminalLifecycle: () => noop,
    onTerminalDataFailure: () => noop,
    sendTerminalInput: (id, generation, bytes) => { inputs.push({ id, generation, text: new TextDecoder().decode(bytes) }); },
    resizeTerminal: async request => ({ ...terminals.find(terminal => terminal.id === request.id)!, size: request.size }),
    createTerminal: unexpected, closeTerminal: unexpected, dismissTerminal: unexpected,
    pathForFile: unexpected, pickDirectory: unexpected,
    savePastedImages: unexpected, startAdversarialReview: unexpected,
  };
  const live = new Set<WebglAddon>();
  let activations = 0;
  let disposals = 0;
  let maxLive = 0;
  let maxContexts = 0;
  const contexts = new Set<WebGL2RenderingContext>();
  let activationMs = 0;
  let disposalMs = 0;
  const activate = WebglAddon.prototype.activate;
  const dispose = WebglAddon.prototype.dispose;
  WebglAddon.prototype.activate = function (terminal) {
    const started = performance.now();
    activate.call(this, terminal);
    activationMs += performance.now() - started;
    activations++;
    live.add(this);
    maxLive = Math.max(maxLive, live.size);
    for (const canvas of terminal.element!.querySelectorAll("canvas")) {
      const context = canvas.getContext("webgl2");
      if (context !== null) contexts.add(context);
    }
    maxContexts = Math.max(maxContexts, [...contexts].filter(context => !context.isContextLost()).length);
  };
  WebglAddon.prototype.dispose = function () {
    const started = performance.now();
    dispose.call(this);
    disposalMs += performance.now() - started;
    if (live.delete(this)) disposals++;
  };
  const until = async (ready: () => boolean, message: string, timeoutMs = 5000): Promise<void> => {
    const deadline = performance.now() + timeoutMs;
    while (!ready()) { assert(performance.now() < deadline, `${message}: ${errors.join("; ")}`); await wait(10); }
  };
  const view = (index: number): HTMLElement | null =>
    document.querySelector(`[data-terminal-id="${terminals[index].id}:1"]`);
  const text = (index: number): string => view(index)?.querySelector(".xterm-accessibility-tree")?.textContent ?? "";
  const gpu = (index: number): WebGL2RenderingContext | null => {
    for (const canvas of view(index)?.querySelectorAll("canvas") ?? []) {
      const context = canvas.getContext("webgl2");
      if (context !== null) return context;
    }
    return null;
  };
  const select = async (index: number): Promise<number> => {
    const started = performance.now();
    flushSync(() => switcher!.onSelect(records[index].id));
    await until(() => records[index].terminals.every(terminal => {
      const element = document.querySelector<HTMLElement>(`[data-terminal-id="${terminal.id}:1"]`);
      return element?.hasAttribute("data-terminal-ready") && element.getBoundingClientRect().height > 0;
    }), "Switched workspace did not become ready");
    await frames();
    return performance.now() - started;
  };
  // The idle window passes only when this fixture says so: real switches take
  // milliseconds, so every warm switch below must stay warm.
  let idleNow = 0;
  const idleTimers = new Map<number, { at: number; task: () => void }>();
  let idleTimerId = 0;
  const idleClock: TerminalWebglClock = {
    now: () => idleNow,
    setTimeout: (task, ms) => { idleTimers.set(++idleTimerId, { at: idleNow + ms, task }); return idleTimerId; },
    clearTimeout: timer => { idleTimers.delete(timer as number); },
  };
  const passIdleWindow = (): void => {
    idleNow += TERMINAL_WEBGL_IDLE_RELEASE_MS;
    for (const [id, timer] of [...idleTimers].sort(([, a], [, b]) => a.at - b.at)) {
      if (timer.at > idleNow || !idleTimers.delete(id)) continue;
      timer.task();
    }
  };
  terminalWebglPool.useClock(idleClock);
  try {
    flushSync(() => root.render(<div style={{ height: 640, display: "flex" }}>
      <IntegratedTerminalCenter api={api} activeCwd="/fixture/switch"
        visible onSwitcherChange={value => { switcher = value; }} onError={message => errors.push(message)} />
    </div>));
    await until(() => switcher !== null, "Switch workspaces did not load");
    await select(0);
    for (let index = 0; index < 2 * panesPerWorkspace; index++) emit(index, `\x1b[2J\x1b[HRetained conversation ${index}\r\nDraft ${index}`);
    await select(1);
    await until(() => outstanding === 0, "Output did not drain");
    const original = terminals.slice(0, 2 * panesPerWorkspace).map((_, index) => view(index));
    const before = { activations, disposals, activationMs, disposalMs };
    const switchMs: number[] = [];
    for (let round = 0; round < 8; round++) {
      const index = round % 2;
      switchMs.push(await select(index));
      for (let pane = index * panesPerWorkspace; pane < (index + 1) * panesPerWorkspace; pane++) {
        assert(view(pane) === original[pane], `Switch replaced emulator ${pane}`);
        await until(() => text(pane).includes(`Draft ${pane}`),
          `Switch lost history ${pane}: ${text(pane)}`);
      }
      const input = view(index * panesPerWorkspace)!.querySelector("textarea")!;
      input.focus();
      input.dispatchEvent(new KeyboardEvent("keypress", { key: "x", charCode: 120,
        keyCode: 120, bubbles: true, cancelable: true }));
      assert(inputs.at(-1)?.id === terminals[index * panesPerWorkspace].id && inputs.at(-1)?.generation === 1,
        "Switch sent input to the wrong pane");
    }
    const warm = { activations: activations - before.activations, disposals: disposals - before.disposals,
      activationMs: activationMs - before.activationMs, disposalMs: disposalMs - before.disposalMs, switchMs };
    const necessaryEvictions = switchMs.length * Math.max(0, 2 * panesPerWorkspace - TERMINAL_WEBGL_LIMIT);
    assert(warm.activations <= necessaryEvictions && warm.disposals <= necessaryEvictions,
      `Warm workspace switches rebuilt ${warm.activations} GPU renderers and disposed ${warm.disposals}`);
    // Out of view past the idle window, the other workspace's renderers are
    // given back. Coming back reloads them before the switch's first frame:
    // checked after the commit's microtasks, before any frame can paint.
    const shown = (round: number): number => round % 2;
    const away = 1 - shown(switchMs.length - 1);
    const beforeIdle = { activations, disposals, parked: live.size - panesPerWorkspace };
    assert(beforeIdle.parked > 0, "No renderer was left out of view to give back");
    const reader = (pane: number): boolean => view(pane)?.querySelector(".xterm-accessibility") !== null;
    const awayPanes = Array.from({ length: panesPerWorkspace }, (_, pane) => away * panesPerWorkspace + pane);
    const shownPanes = Array.from({ length: panesPerWorkspace }, (_, pane) => (1 - away) * panesPerWorkspace + pane);
    assert(awayPanes.every(reader), "A pane left out of view dropped screen-reader mode before the idle window");
    passIdleWindow();
    assert(live.size === panesPerWorkspace && disposals - beforeIdle.disposals === beforeIdle.parked,
      `Idle release left ${live.size} GPU renderers (${disposals - beforeIdle.disposals} given back)`);
    // Past the same window, panes out of view stop building announcements;
    // the panes on screen keep theirs.
    assert(awayPanes.every(pane => !reader(pane)), "A pane out of view kept screen-reader mode past the idle window");
    assert(shownPanes.every(reader), "A pane on screen lost screen-reader mode");
    // A spinner redraws one row with no line feed: the announcement xterm
    // would keep for it grows without bound while nothing renders.
    for (const pane of awayPanes) {
      emit(pane, `\x1b[10;1H${Array.from({ length: TERMINAL_ANNOUNCEMENT_LIMIT },
        (_, frame) => `\rWorking ${frame}`).join("")}`);
    }
    await until(() => outstanding === 0, "Hidden output did not drain");
    const returned = performance.now();
    flushSync(() => switcher!.onSelect(records[away].id));
    // Back on before the commit paints, with the announcement still bounded.
    assert(awayPanes.every(reader), "A returning pane had no screen-reader mode before its first frame");
    assert(awayPanes.every(pane => (view(pane)!.querySelector(".xterm-accessibility .live-region")
      ?.textContent?.length ?? 0) <= TERMINAL_ANNOUNCEMENT_LIMIT), "A returning pane announced unbounded output");
    await Promise.resolve();
    for (let pane = away * panesPerWorkspace; pane < (away + 1) * panesPerWorkspace; pane++) {
      assert(gpu(pane)?.isContextLost() === false, `Returning pane ${pane} painted before its GPU renderer reloaded`);
    }
    assert(activations - beforeIdle.activations === panesPerWorkspace,
      `Returning workspace rebuilt ${activations - beforeIdle.activations} GPU renderers`);
    await until(() => records[away].terminals.every(terminal => document.querySelector<HTMLElement>(
      `[data-terminal-id="${terminal.id}:1"]`)?.hasAttribute("data-terminal-ready")), "Returning workspace did not become ready");
    await frames();
    const idleReturnMs = performance.now() - returned;
    for (let pane = away * panesPerWorkspace; pane < (away + 1) * panesPerWorkspace; pane++) {
      assert(view(pane) === original[pane] && text(pane).includes(`Draft ${pane}`), `Idle release lost pane ${pane}`);
    }
    let recoveryMs: number | null = null;
    if (recoverLostContext) {
      // Sleep, display changes and GPU process restarts lose contexts. A
      // visible pane must return to WebGL without a workspace switch: the DOM
      // fallback draws the same text visibly thinner.
      await select(0);
      const lost = gpu(0);
      assert(lost !== null && !lost.isContextLost(), "Visible pane was not on the GPU renderer");
      const started = performance.now();
      const reloaded = activations;
      lost.getExtension("WEBGL_lose_context")!.loseContext();
      await until(() => activations > reloaded && gpu(0)?.isContextLost() === false,
        "Visible pane stayed on the DOM renderer after losing its GPU context", 12_000);
      recoveryMs = performance.now() - started;
      assert(view(0) === original[0] && text(0).includes("Draft 0"), "GPU recovery lost the terminal buffer");
    }
    // Visit more panes than Chromium's context limit, then return to an old view.
    for (const index of [2, 3, 4, 0]) await select(index);
    assert(view(0) === original[0] && text(0).includes("Retained conversation 0"),
      "Resource eviction lost the terminal buffer");
    assert(maxLive <= TERMINAL_WEBGL_LIMIT && maxContexts <= TERMINAL_WEBGL_LIMIT,
      `GPU budget exceeded: ${maxLive} addons, ${maxContexts} live contexts`);
    assert(activations > terminals.length && disposals > 0, "Never exercised GPU cache eviction and reacquisition");
    // One text size for every workspace: a size chosen while a workspace is
    // out of view reaches its panes when it comes back.
    const fontSize = (pane: number): string =>
      view(pane)?.querySelector<HTMLElement>(".xterm-accessibility")?.style.fontSize ?? "";
    const panesOf = (index: number): number[] =>
      Array.from({ length: panesPerWorkspace }, (_, pane) => index * panesPerWorkspace + pane);
    const page = document.documentElement;
    const chosenSize = page.style.getPropertyValue("--workspace-terminal-font-size");
    page.style.setProperty("--workspace-terminal-font-size", "14");
    page.dispatchEvent(new CustomEvent("interface-theme-change"));
    try {
      await until(() => panesOf(0).every(pane => fontSize(pane) === "14px"),
        `A pane on screen kept its text size: ${panesOf(0).map(fontSize).join(", ")}`);
      await select(4);
      await until(() => panesOf(4).every(pane => fontSize(pane) === "14px"),
        `A returning pane kept the text size it read out of view: ${panesOf(4).map(fontSize).join(", ")}`);
    } finally {
      if (chosenSize === "") page.style.removeProperty("--workspace-terminal-font-size");
      else page.style.setProperty("--workspace-terminal-font-size", chosenSize);
      page.dispatchEvent(new CustomEvent("interface-theme-change"));
      await frames();
    }
    assert(outstanding === 0 && errors.length === 0, errors.join("; "));
    return { panesPerWorkspace, workspaces: 5, warm, idleReturnMs, recoveryMs, maxLive, maxContexts,
      live: live.size, activations, disposals };
  } finally {
    flushSync(() => root.render(null));
    terminalWebglPool.useClock();
    WebglAddon.prototype.activate = activate;
    WebglAddon.prototype.dispose = dispose;
    assert(live.size === 0, "Unmount leaked a terminal GPU renderer");
    assert([...contexts].every(context => context.isContextLost()), "Unmount left GPU contexts waiting for garbage collection");
  }
}
