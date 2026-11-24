import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import type { TerminalSummary, TerminalWorkspaceRecord } from "../../shared/ipc";
import { IntegratedTerminalCenter, type TerminalRendererApi } from "../components/IntegratedTerminalCenter";
import type { TerminalWorkspaceSwitcher } from "../components/TerminalWorkspace";
import { attachTerminalsBalanced, EMPTY_TERMINAL_WORKSPACE_STATE, serializeTerminalWorkspaceState } from "../terminal-workspace";
import { MAX_REPLAY_OUTPUT_BYTES } from "../terminal-replay-buffer";
import { assert, noop, wait } from "./fixture-support";

/** Codex uses normal-buffer history that a SIGWINCH cannot reconstruct. */
export async function terminalPreservationFixture(root: Root): Promise<Record<string, unknown>> {
  const stamp = "2026-09-19T00:00:00Z";
  const terminals: TerminalSummary[] = [0, 1].map(index => ({
    id: `00000000-0000-4000-8000-${String(901 + index).padStart(12, "0")}`,
    generation: 1, title: `Codex ${index}`, root: "/fixture/preservation", cwd: "/fixture/preservation",
    state: "running", size: { columns: 80, rows: 24 },
    launch: { version: 1, profileId: "codex" }, createdAt: stamp, updatedAt: stamp,
  }));
  const records: TerminalWorkspaceRecord[] = terminals.map((terminal, index) => ({
    id: `preserved-workspace-${index}`, name: `Preserved ${index}`, createdAt: stamp, updatedAt: stamp,
    terminals: [terminal], layout: serializeTerminalWorkspaceState(attachTerminalsBalanced(
      EMPTY_TERMINAL_WORKSPACE_STATE, [terminal.id], () => `preserved-pane-${index}`, () => "unused")),
  }));
  let switcher: TerminalWorkspaceSwitcher | null = null;
  let receive: Parameters<TerminalRendererApi["onTerminalData"]>[0] | undefined;
  let lifecycle: Parameters<TerminalRendererApi["onTerminalLifecycle"]>[0] | undefined;
  let outstanding = 0;
  let revision = 1;
  const errors: string[] = [];
  const inputs: { id: string; generation: number; text: string }[] = [];
  const encoder = new TextEncoder();
  const emit = (index: number, data: string | Uint8Array, generation = terminals[index].generation): void => {
    outstanding++;
    receive!({ kind: "data", terminalId: terminals[index].id, generation, sequence: "0",
      data: typeof data === "string" ? encoder.encode(data) : data }, () => { outstanding--; });
  };
  const unexpected = (): never => { throw new Error("Preservation performed an unrelated provider operation"); };
  const api: TerminalRendererApi = {
    listTerminals: async () => ({ revision, terminals }), listTerminalLaunchProfiles: async () => [],
    getTerminalWorkspaces: async () => records, setTerminalWorkspaces: async value => value,
    onTerminalData: callback => { receive = callback; return noop; },
    onTerminalLifecycle: callback => { lifecycle = callback; return noop; }, onTerminalDataFailure: () => noop,
    sendTerminalInput: (id, generation, bytes) => { inputs.push({ id, generation, text: new TextDecoder().decode(bytes) }); },
    // Deliberately no repaint: a process need not redraw its history on resize.
    resizeTerminal: async request => ({ ...terminals.find(terminal => terminal.id === request.id)!, size: request.size }),
    createTerminal: unexpected, closeTerminal: unexpected, dismissTerminal: unexpected,
    pathForFile: unexpected, pickDirectory: unexpected,
    savePastedImages: unexpected, startAdversarialReview: unexpected,
  };
  const until = async (ready: () => boolean, message: string): Promise<void> => {
    const deadline = performance.now() + 4000;
    while (!ready()) { assert(performance.now() < deadline, `${message}: ${errors.join("; ")}`); await wait(20); }
  };
  const view = (index: number, generation = terminals[index].generation): HTMLElement | null =>
    document.querySelector(`[data-terminal-id="${terminals[index].id}:${generation}"]`);
  const text = (index: number): string => view(index)?.querySelector(".xterm-accessibility-tree")?.textContent ?? "";
  const select = async (index: number): Promise<void> => {
    flushSync(() => switcher!.onSelect(records[index].id));
    await until(() => view(index)?.hasAttribute("data-terminal-ready") === true
      && view(index)!.getBoundingClientRect().height > 0, "Selected terminal did not become visible");
  };
  try {
    flushSync(() => root.render(<div style={{ height: 540, display: "flex" }}>
      <IntegratedTerminalCenter api={api} activeCwd="/fixture/preservation"
        visible onSwitcherChange={value => { switcher = value; }} onError={message => errors.push(message)} />
    </div>));
    await until(() => switcher !== null, "Preservation workspaces did not load");
    await select(0);
    const original = view(0)!;
    emit(0, "\x1b[?2004h\x1b[2J\x1b[H" + Array.from({ length: 60 }, (_, index) =>
      `Earlier history ${index}\r\n`).join("") + "Original Codex conversation\r\nDraft: keep this text");
    // Exceed both byte and chunk retention limits without replacing the screen.
    const noise = "\x1b[0m".repeat(4096);
    for (let i = 0; i < MAX_REPLAY_OUTPUT_BYTES / (noise.length) + 2; i++) emit(0, noise);
    for (let i = 0; i < 4100; i++) emit(0, "\x1b[0m");
    await until(() => outstanding === 0 && text(0).includes("Draft: keep this text"), "Long Codex output did not render");
    for (let round = 0; round < 3; round++) {
      // Preserve parser state too: split a CSI and a UTF-8 character across moves.
      emit(0, "\x1b[3");
      await until(() => outstanding === 0, "Partial escape did not drain");
      await select(1);
      assert(original.isConnected && original.getBoundingClientRect().height === 0,
        "Leaving the workspace destroyed the long Codex emulator");
      assert(original.closest("[inert]") !== null && !original.contains(document.activeElement),
        "Hidden terminal remained interactive");
      emit(0, "2m\r\nBackground result ");
      emit(0, new Uint8Array([0xe2, 0x9c]));
      await until(() => outstanding === 0, "Hidden output credits did not drain");
      await select(0);
      emit(0, new Uint8Array([0x93]));
      emit(0, "\x1b[0m\r\nReady for input");
      await until(() => outstanding === 0 && text(0).includes("Background result ✓"), "Parser state or hidden output was lost");
      assert(view(0) === original, "Returning recreated the Codex emulator");
      assert(text(0).includes("Original Codex conversation") && text(0).includes("Draft: keep this text")
        && !text(0).includes("Earlier background output"), "Long Codex screen was replaced by truncated replay");
      const input = original.querySelector("textarea")!;
      input.focus();
      input.dispatchEvent(new KeyboardEvent("keypress", { key: "x", charCode: 120,
        keyCode: 120, bubbles: true, cancelable: true }));
      const clipboard = new DataTransfer();
      clipboard.setData("text/plain", "one\ntwo");
      input.dispatchEvent(new ClipboardEvent("paste", { clipboardData: clipboard, bubbles: true, cancelable: true }));
      await until(() => inputs.some(item => item.text === "\x1b[200~one\rtwo\x1b[201~"), "Bracketed paste mode was lost");
      assert(inputs.some(item => item.text === "x") && inputs.every(item => item.id === terminals[0].id
        && item.generation === 1), "Input crossed a terminal generation or workspace");
      inputs.length = 0;
    }
    const firstRow = (): number => Number(original.querySelector(".xterm-accessibility-tree")
      ?.firstElementChild?.getAttribute("aria-posinset"));
    const latestRow = firstRow();
    const screen = original.querySelector(".xterm-screen")!;
    const bounds = screen.getBoundingClientRect();
    const wheel = new WheelEvent("wheel", { bubbles: true, cancelable: true, deltaY: -400,
      clientX: bounds.left + 30, clientY: bounds.top + 30 });
    Object.defineProperties(wheel, { wheelDeltaY: { value: 400 }, wheelDelta: { value: 400 },
      wheelDeltaX: { value: 0 } });
    screen.dispatchEvent(wheel);
    await until(() => firstRow() < latestRow && text(0).includes("Earlier history"),
      "Workspace switches lost normal-buffer scrollback");
    assert(inputs.length === 0, "Normal-buffer scrolling sent provider input");
    // Restart while parked: drop the obsolete emulator, never replay its bytes.
    await select(1);
    terminals[0] = { ...terminals[0], generation: 2 };
    flushSync(() => lifecycle!({ kind: "upsert", revision: ++revision, terminal: terminals[0] }));
    await until(() => !original.isConnected, "Restart retained the previous generation");
    emit(0, "STALE OUTPUT", 1);
    emit(0, "Fresh generation");
    await select(0);
    await until(() => text(0).includes("Fresh generation"), "New generation did not restore");
    assert(!text(0).includes("STALE OUTPUT") && !text(0).includes("Original Codex conversation"), "Restart mixed generations");
    const retired = view(0)!;
    await select(1);
    flushSync(() => lifecycle!({ kind: "removed", revision: ++revision, id: terminals[0].id, generation: 2 }));
    await until(() => !retired.isConnected, "Dismissal retained a parked emulator");
    assert(outstanding === 0 && errors.length === 0, errors.join("; "));
    return { workspaceSwitches: 6, replayByteLimitExceeded: true, replayChunkLimitExceeded: true,
      preservedScreen: true, scrollback: true, splitParserState: true, typingAndPaste: true, hiddenCredits: true,
      generationCleanup: true, dismissalCleanup: true };
  } finally { flushSync(() => root.render(null)); }
}
