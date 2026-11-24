import { KittyKeyboardStacks } from "./terminal-kitty-keyboard";
import type { TerminalReplayBoundary } from "./terminal-output-state";

/**
 * Input modes outlive the bounded screen replay. Track only modes needed to
 * restore a fresh xterm before a TUI repaint; never replay queries, titles,
 * synchronized-output fences, or arbitrary terminal output as setup.
 */
export class TerminalReplayModes {
  private parameters: string | null = null;
  /** OpenTUI asks CSI 14 t at startup and again on each resize it processes. */
  queriedPixelSize = false;
  private readonly modes = new Map<number, { mode: number; enabled: boolean }>();
  /** Codex pushes kitty keyboard flags once at startup; ⌘C forwarding needs them. */
  private readonly keyboard = new KittyKeyboardStacks();

  private set(mode: number, enabled: boolean): void {
    // xterm treats these as mutually exclusive groups, including on reset.
    const group = mode === 47 || mode === 1047 || mode === 1049 ? 1049
      : mode === 9 || mode === 1000 || mode === 1002 || mode === 1003 ? 1000
      : mode === 1006 || mode === 1016 ? 1006
      : mode === 1 || mode === 2004 ? mode : undefined;
    if (group === 1049 && enabled && !this.alternateScreen) this.keyboard.enterAlternate();
    if (group !== undefined) this.modes.set(group, { mode, enabled });
  }

  private get screen(): "normal" | "alternate" {
    return this.alternateScreen ? "alternate" : "normal";
  }

  /** Called by the replay boundary parser, which skips ordinary ground text. */
  readonly observe = (boundary: TerminalReplayBoundary, byte: number): void => {
    if (boundary === "escape") {
      this.parameters = byte === 0x5b ? "" : null;
      if (byte === 0x63) { // RIS resets both buffers and input modes.
        for (const mode of [1, 1049, 1000, 1006, 2004]) this.set(mode, false);
        this.keyboard.reset();
      }
      return;
    }
    if (boundary !== "csi-parameter" && boundary !== "csi-intermediate") return;
    if (byte === 0x18 || byte === 0x1a || byte === 0x1b) {
      this.parameters = null;
    } else if (byte >= 0x20 && byte <= 0x3f) {
      // An incomplete or malicious sequence cannot grow with the output tail.
      this.parameters = this.parameters !== null && this.parameters.length < 128
        ? this.parameters + String.fromCharCode(byte) : null;
    } else if (byte >= 0x40 && byte <= 0x7e) {
      const parameters = this.parameters;
      this.parameters = null;
      if (parameters === null) return;
      if (byte === 0x74 && parameters === "14") this.queriedPixelSize = true;
      const keyboard = byte === 0x75 ? /^([<=>])(\d*)(?:;(\d*))?$/.exec(parameters) : null;
      if (keyboard !== null) {
        this.keyboard.apply(this.screen, keyboard[1]!,
          [keyboard[2], keyboard[3]].filter(value => value !== undefined).map(value => Number(value || 0)));
      }
      if ((byte === 0x68 || byte === 0x6c) && /^\?[\d;]+$/.test(parameters)) {
        for (const mode of parameters.slice(1).split(";")) this.set(Number(mode), byte === 0x68);
      } else if (byte === 0x70 && /^[\d;]*!$/.test(parameters)) {
        // DECSTR resets xterm's core input modes, but keeps buffer/mouse state.
        this.set(1, false);
        this.set(2004, false);
      }
    }
  };

  /** A full-screen program owns the frame, whatever the pane was launched as. */
  get alternateScreen(): boolean {
    return this.modes.get(1049)?.enabled === true;
  }

  /**
   * Each screen's kitty stack is pushed while that screen is active: the main
   * screen's before the buffer modes switch screens, the alternate's after.
   */
  get restoreSequence(): string {
    const modes = [...this.modes.values()].map(({ mode, enabled }) => `\x1b[?${mode}${enabled ? "h" : "l"}`).join("");
    return this.keyboard.restoreSequence("normal") + modes
      + (this.alternateScreen ? this.keyboard.restoreSequence("alternate") : "");
  }
}
