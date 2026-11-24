import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import { IntegratedTerminalCenter, type TerminalRendererApi } from "../components/IntegratedTerminalCenter";
import type { TerminalWorkspaceSwitcher } from "../components/TerminalWorkspace";
import type { TerminalSummary, TerminalWorkspaceRecord } from "../../shared/ipc";
import { attachTerminalsBalanced, EMPTY_TERMINAL_WORKSPACE_STATE, serializeTerminalWorkspaceState } from "../terminal-workspace";
import { MAX_REPLAY_OUTPUT_BYTES } from "../terminal-replay-buffer";
import { assert, noop, wait } from "./fixture-support";

/** OpenTUI debounces SIGWINCH and can be busy while buffered delta frames arrive.
 *  The second OpenCode was typed into a shell pane, so only its stream says it is a TUI. */
export async function terminalRedrawFixture(root: Root): Promise<{
  panes: number; remounts: number; workspaceSwitches: number; wheelChecks: number; normalWheelChecks: number;
}> {
  const stamp = "2026-09-07T00:00:00.000Z";
  const terminals: TerminalSummary[] = [0, 1, 2].map(index => ({
    id: `00000000-0000-4000-8000-${String(100 + index).padStart(12, "0")}`,
    generation: 1, title: `OpenCode ${index}`, root: "/fixture/redraw", cwd: "/fixture/redraw",
    state: "running", size: { columns: 80, rows: 24 },
    launch: { version: 1, profileId: index === 0 ? "opencode" : "shell" },
    createdAt: stamp, updatedAt: stamp,
  }));
  const records: TerminalWorkspaceRecord[] = [[0, 1], [2]].map((indices, index) => ({
    id: `redraw-workspace-${index}`, name: `Redraw ${index}`, createdAt: stamp, updatedAt: stamp,
    terminals: indices.map(at => terminals[at]),
    layout: serializeTerminalWorkspaceState(attachTerminalsBalanced(EMPTY_TERMINAL_WORKSPACE_STATE,
      indices.map(at => terminals[at].id), at => `redraw-pane-${index}-${at}`, at => `redraw-split-${index}-${at}`)),
  }));
  let switcher: TerminalWorkspaceSwitcher | null = null;
  let receive: Parameters<TerminalRendererApi["onTerminalData"]>[0] | undefined;
  const sizes = new Map(terminals.map(terminal => [terminal.id, terminal.size]));
  const resizeTimers = new Map<string, ReturnType<typeof setTimeout>>();
  let busyUntil = 0;
  const errors: string[] = [];
  const inputs: { id: string; data: string }[] = [];
  const history = new Map(terminals.map(terminal => [terminal.id, false]));
  let wheelChecks = 0;
  let pendingWrites = 0;
  const emit = (id: string, text: string): void => {
    if (receive === undefined) return;
    pendingWrites++;
    receive({ kind: "data", terminalId: id,
      generation: 1, sequence: "0", data: new TextEncoder().encode(text) }, () => { pendingWrites--; });
  };
  const paint = (id: string): void => {
    const size = sizes.get(id)!;
    emit(id, `\x1b[2J\x1b[HOpenCode ${id.slice(-3)}\x1b[3;1H${history.get(id) ? "Earlier conversation" : "Complete conversation"}\x1b[5;1HAsk anything\x1b[7;1H${size.columns}x${size.rows}`);
  };
  const unexpected = (): never => { throw new Error("Redraw performed an unrelated provider operation"); };
  const api: TerminalRendererApi = {
    listTerminals: async () => ({ revision: 1, terminals }),
    listTerminalLaunchProfiles: async () => [], getTerminalWorkspaces: async () => records,
    setTerminalWorkspaces: async value => value,
    onTerminalData: callback => { receive = callback; return () => { receive = undefined; }; },
    onTerminalLifecycle: () => noop, onTerminalDataFailure: () => noop,
    sendTerminalInput: (id, _generation, bytes) => {
      const data = new TextDecoder().decode(bytes);
      inputs.push({ id, data });
      if (/\x1b\[<6[45];\d+;\d+M/.test(data)) {
        history.set(id, data.includes("[<64;"));
        paint(id);
      }
    },
    resizeTerminal: async request => {
      clearTimeout(resizeTimers.get(request.id));
      resizeTimers.set(request.id, setTimeout(() => {
        const previous = sizes.get(request.id)!;
        if (previous.columns === request.size.columns && previous.rows === request.size.rows) return;
        sizes.set(request.id, request.size);
        if (request.id !== terminals[2].id) {
          // OpenTUI 0.4.5 queries pixel geometry when it processes a resize.
          // Deliberately split the query across PTY transport frames.
          emit(request.id, "\x1b[1");
          emit(request.id, "4t");
          paint(request.id);
        }
      }, Math.max(100, busyUntil - performance.now())));
      return { ...terminals.find(terminal => terminal.id === request.id)!, size: request.size };
    },
    closeTerminal: unexpected, createTerminal: unexpected, dismissTerminal: unexpected,
    pathForFile: unexpected, pickDirectory: unexpected,
    savePastedImages: unexpected, startAdversarialReview: unexpected,
  };
  const until = async (ready: () => boolean, message: string): Promise<void> => {
    const deadline = performance.now() + 3000;
    while (!ready()) { assert(performance.now() < deadline, `${message}: ${errors.join("; ")}`); await wait(20); }
  };
  const text = (id: string): string => document.querySelector(
    `[data-terminal-id="${id}:1"] .xterm-accessibility-tree`)?.textContent ?? "";
  const checkScrolling = async (phase: string): Promise<void> => {
    for (const terminal of terminals.slice(0, 2)) {
      const screen = document.querySelector(`[data-terminal-id="${terminal.id}:1"] .xterm-screen`)!;
      const bounds = screen.getBoundingClientRect();
      for (const deltaY of [-3, 3]) {
        inputs.length = 0;
        screen.dispatchEvent(new WheelEvent("wheel", { bubbles: true, cancelable: true,
          deltaMode: WheelEvent.DOM_DELTA_LINE, deltaY,
          clientX: bounds.left + bounds.width / 2, clientY: bounds.top + bounds.height / 2 }));
        const button = deltaY < 0 ? 64 : 65;
        await until(() => inputs.some(input => input.id === terminal.id
          && input.data.includes(`\x1b[<${button};`)), `OpenCode did not receive scroll ${deltaY} ${phase}`);
        assert(inputs.every(input => input.id === terminal.id), "Wheel input reached another pane");
        await until(() => text(terminal.id).includes(deltaY < 0 ? "Earlier conversation" : "Complete conversation"),
          `OpenCode history did not scroll ${phase}`);
        wheelChecks++;
      }
    }
  };
  let animation: ReturnType<typeof setInterval> | undefined;
  try {
    flushSync(() => root.render(<div style={{ height: 420, display: "flex" }}>
      <IntegratedTerminalCenter api={api} activeCwd="/fixture/redraw"
        visible onError={message => errors.push(message)} onSwitcherChange={value => { switcher = value; }} />
    </div>));
    await until(() => switcher !== null, "Redraw workspaces did not load");
    flushSync(() => switcher!.onSelect(records[0].id));
    await wait(400);
    for (const terminal of terminals.slice(0, 2)) {
      // Modes and the pixel query emitted by the installed OpenCode/OpenTUI at startup.
      emit(terminal.id, "\x1b[?1049h\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h\x1b[?2004h\x1b[14t");
      paint(terminal.id);
    }
    await until(() => terminals.slice(0, 2).every(terminal => text(terminal.id).includes("Ask anything")), "Initial OpenCode frames missing");
    await checkScrolling("before remount");
    animation = setInterval(() => {
      for (const terminal of terminals.slice(0, 2)) emit(terminal.id, "\x1b[9;1H.");
    }, 15);
    for (let round = 0; round < 3; round++) {
      const targets = terminals.slice(0, 2).map(terminal => ({ ...sizes.get(terminal.id)! }));
      const oldViews = terminals.slice(0, 2).map(terminal => document.querySelector(`[data-terminal-id="${terminal.id}:1"]`));
      flushSync(() => switcher!.onSelect(records[1].id));
      await wait(50);
      for (const terminal of terminals.slice(0, 2)) {
        // Background animation evicts the original full frame from the bounded replay tail.
        emit(terminal.id, "\x1b[0m".repeat(MAX_REPLAY_OUTPUT_BYTES / 4 + 1));
      }
      // The old 500ms fallback restored the original PTY geometry before the
      // busy process ran: its two resize signals then collapsed to a no-op.
      busyUntil = performance.now() + (round === 1 ? 800 : 0);
      flushSync(() => switcher!.onSelect(records[0].id));
      await until(() => terminals.slice(0, 2).every((terminal, index) => {
        const view = document.querySelector(`[data-terminal-id="${terminal.id}:1"]`);
        const screen = text(terminal.id);
        return view === oldViews[index] && screen.includes("Complete conversation") && screen.includes("Ask anything")
          && screen.includes(`${targets[index].columns}x${targets[index].rows}`)
          && !screen.includes("Earlier background output");
      }), `OpenCode lost its complete frame after remount ${round + 1}`);
      await checkScrolling(`after remount ${round + 1}`);
    }
    // A program leaving its TUI must retain normal shell scrolling on remount.
    clearInterval(animation);
    flushSync(() => switcher!.onSelect(records[1].id));
    await wait(50);
    for (const terminal of terminals.slice(0, 2)) {
      emit(terminal.id, "\x1b[?1049;1003;1006;2004l");
      emit(terminal.id, "\x1b[0m".repeat(MAX_REPLAY_OUTPUT_BYTES / 4 + 1));
    }
    flushSync(() => switcher!.onSelect(records[0].id));
    // The same emulators consumed the mode reset while parked. No replay or
    // synthetic resize is needed, including the shell's original normal buffer.
    await until(() => terminals.slice(0, 2).every(terminal =>
      document.querySelector(`[data-terminal-id="${terminal.id}:1"]`)?.getBoundingClientRect().height),
      "Exited OpenCode panes did not become visible");
    assert(terminals.slice(0, 2).every(terminal => !text(terminal.id).includes("Earlier background output")),
      "Workspace switch replaced a retained emulator with truncated output");
    await until(() => pendingWrites === 0, "Parked OpenCode mode resets did not drain");
    for (const terminal of terminals.slice(0, 2)) {
      const screen = document.querySelector(`[data-terminal-id="${terminal.id}:1"] .xterm-screen`)!;
      const bounds = screen.getBoundingClientRect();
      inputs.length = 0;
      screen.dispatchEvent(new WheelEvent("wheel", { bubbles: true, cancelable: true,
        deltaMode: WheelEvent.DOM_DELTA_LINE, deltaY: -3,
        clientX: bounds.left + bounds.width / 2, clientY: bounds.top + bounds.height / 2 }));
      await wait(50);
      assert(inputs.length === 0, "Remount re-enabled TUI wheel input after OpenCode disabled it");
    }
    assert(errors.length === 0, errors.join("; "));
    return { panes: 2, remounts: 0, workspaceSwitches: 6, wheelChecks, normalWheelChecks: 2 };
  } finally {
    clearInterval(animation);
    for (const timer of resizeTimers.values()) clearTimeout(timer);
    flushSync(() => root.render(null));
  }
}
