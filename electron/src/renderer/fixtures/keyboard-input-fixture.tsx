import { flushSync } from "react-dom";
import type { Root } from "react-dom/client";
import { TerminalView } from "../components/TerminalView";
import { installKeyboardInputGuard } from "../keyboard-input";

/** Native Electron input, ordinary editing and the actual production xterm. */
export function createKeyboardInputFixture(root: Root) {
  let sent = "";
  let removeGuard: (() => void) | undefined;
  return {
    guard(enabled: boolean): void {
      removeGuard?.();
      removeGuard = enabled ? installKeyboardInputGuard(window) : undefined;
    },
    async setup(mode: "text" | "terminal"): Promise<void> {
      sent = "";
      flushSync(() => root.render(null));
      flushSync(() => root.render(<div style={{ height: 400 }}>
        {mode === "text" ? <input aria-label="Keyboard fixture" /> :
          <TerminalView terminalId="keyboard-fixture" active onData={data => {
            sent += data;
          }} onReady={handle => { handle?.completeSetup(); }} />}
      </div>));
      await new Promise(requestAnimationFrame);
      await new Promise(requestAnimationFrame);
      document.querySelector<HTMLInputElement | HTMLTextAreaElement>("input, textarea")!.focus();
      await new Promise(requestAnimationFrame);
    },
    read(): string { return document.querySelector<HTMLInputElement>("input")?.value ?? sent; },
    /** What the terminal sent its program, whatever else has an input open. */
    sent(): string { return sent; },
    searchOpen(): boolean { return document.querySelector(".terminal-search") !== null; },
  };
}
