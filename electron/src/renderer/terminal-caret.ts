import type { ITheme, Terminal } from "@xterm/xterm";

/** Share of the row the caret covers, centred on the text. */
export const TERMINAL_CARET_HEIGHT = 0.85;
export const TERMINAL_CARET_WIDTH = 2;
const TRANSPARENT = "#00000000";

export type TerminalCursorStyle = "block" | "underline" | "bar";
export interface TerminalCaretBox { left: number; top: number; width: number; height: number }

/** xterm's own DECSCUSR (`CSI Ps SP q`) mapping. 0 returns to the configured
 * style; an unknown value leaves the current style alone. */
export function terminalCursorStyleAfter(current: TerminalCursorStyle | undefined,
    params: readonly (number | readonly number[])[]): TerminalCursorStyle | undefined {
  const first = params[0];
  const value = params.length === 0 ? 1 : typeof first === "number" ? first : first[0] ?? 0;
  if (value === 0) return undefined;
  if (value === 1 || value === 2) return "block";
  if (value === 3 || value === 4) return "underline";
  if (value === 5 || value === 6) return "bar";
  return current;
}

/** Reference bar: two CSS pixels wide, inset within the row, snapped to device pixels. */
export function terminalCaretBox(cell: { width: number; height: number }, column: number,
    row: number, devicePixelRatio: number): TerminalCaretBox {
  const snap = (value: number): number => Math.round(value * devicePixelRatio) / devicePixelRatio;
  const height = Math.max(1 / devicePixelRatio, snap(cell.height * TERMINAL_CARET_HEIGHT));
  return {
    left: column * cell.width,
    top: row * cell.height + snap((cell.height - height) / 2),
    width: Math.max(1 / devicePixelRatio, snap(TERMINAL_CARET_WIDTH)),
    height,
  };
}

function includes(params: readonly (number | readonly number[])[], mode: number): boolean {
  return params.some(param => param === mode);
}

export interface TerminalCaret {
  /** Applies a palette, keeping xterm's own bar transparent while the caret draws it. */
  setTheme(theme: ITheme): void;
  dispose(): void;
}

/**
 * xterm draws a bar cursor (the configured default, or DECSCUSR 5/6) the full
 * row tall and at least one CSS pixel wide. While a focused terminal shows a
 * bar, this makes xterm's cursor colour transparent and draws the reference
 * blue caret in its place. Block and underline cursors, and the unfocused outline, stay xterm's.
 * Uses the parser hooks, render event and screen size, not xterm private
 * fields. A cursor colour alone is not part of the WebGL glyph atlas key, so
 * switching it repaints without rasterizing glyphs again.
 */
export function installTerminalCaret(terminal: Terminal): TerminalCaret {
  const screen = terminal.element?.querySelector<HTMLElement>(".xterm-screen") ?? null;
  const textarea = terminal.textarea ?? null;
  let theme: ITheme = terminal.options.theme ?? {};
  if (screen === null || textarea === null) {
    return { setTheme(next) { terminal.options.theme = next; }, dispose() {} };
  }
  const caret = screen.ownerDocument.createElement("div");
  caret.className = "terminal-caret";
  caret.setAttribute("aria-hidden", "true");
  caret.hidden = true;
  screen.appendChild(caret);

  let style: TerminalCursorStyle | undefined;
  let hidden = false;
  let owning = false;
  let placed = "";
  // A blur also means the window lost focus while the textarea stayed active.
  let focused = textarea.ownerDocument.hasFocus()
    && textarea.ownerDocument.activeElement === textarea;
  const locate = (): TerminalCaretBox | null => {
    if (!owning || hidden) return null;
    const buffer = terminal.buffer.active;
    const row = buffer.baseY + buffer.cursorY - buffer.viewportY;
    const width = Number.parseFloat(screen.style.width) / terminal.cols;
    const height = Number.parseFloat(screen.style.height) / terminal.rows;
    if (row < 0 || row >= terminal.rows || !(width > 0 && height > 0)) return null;
    return terminalCaretBox({ width, height }, Math.min(buffer.cursorX, terminal.cols - 1), row,
      screen.ownerDocument.defaultView?.devicePixelRatio || 1);
  };
  const place = (): void => {
    const box = locate();
    const key = box === null ? "" : `${box.left} ${box.top} ${box.width} ${box.height}`;
    if (key === placed) return;
    placed = key;
    caret.hidden = box === null;
    if (box === null) return;
    caret.style.transform = `translate(${box.left}px, ${box.top}px)`;
    caret.style.width = `${box.width}px`;
    caret.style.height = `${box.height}px`;
  };
  const reconcile = (): void => {
    const next = (style ?? terminal.options.cursorStyle) === "bar" && focused;
    if (next === owning) return;
    owning = next;
    terminal.options.theme = owning ? { ...theme, cursor: TRANSPARENT } : theme;
    place();
  };
  // Parser hooks run before xterm's own handler; returning false keeps it.
  // Ownership settles after the parsed chunk and before xterm paints it.
  let pending = false;
  let disposed = false;
  const settle = (): void => {
    if (pending) return;
    pending = true;
    queueMicrotask(() => { pending = false; if (!disposed) reconcile(); });
  };
  const parser = terminal.parser;
  const disposables = [
    parser.registerCsiHandler({ intermediates: " ", final: "q" }, params => {
      style = terminalCursorStyleAfter(style, params);
      settle();
      return false;
    }),
    parser.registerCsiHandler({ prefix: "?", final: "h" }, params => {
      if (includes(params, 25)) hidden = false;
      return false;
    }),
    parser.registerCsiHandler({ prefix: "?", final: "l" }, params => {
      if (includes(params, 25)) hidden = true;
      return false;
    }),
    // DECSTR and RIS restore the configured style and show the cursor.
    parser.registerCsiHandler({ intermediates: "!", final: "p" }, () => {
      style = undefined; hidden = false; settle();
      return false;
    }),
    parser.registerEscHandler({ final: "c" }, () => {
      style = undefined; hidden = false; settle();
      return false;
    }),
    // xterm repaints the cursor row after every move, visibility or focus change.
    terminal.onRender(place),
  ];
  const focus = (): void => { focused = textarea.ownerDocument.hasFocus(); reconcile(); };
  const blur = (): void => { focused = false; reconcile(); };
  textarea.addEventListener("focus", focus);
  textarea.addEventListener("blur", blur);
  return {
    setTheme(next) {
      theme = next;
      terminal.options.theme = owning ? { ...theme, cursor: TRANSPARENT } : theme;
    },
    dispose() {
      disposed = true;
      textarea.removeEventListener("focus", focus);
      textarea.removeEventListener("blur", blur);
      for (const disposable of disposables) disposable.dispose();
      caret.remove();
    },
  };
}
