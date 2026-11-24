export type MouseTrackingMode = "none" | "x10" | "vt200" | "drag" | "any";

export interface AltScreenWheelState {
  readonly bufferType: "normal" | "alternate";
  readonly mouseTrackingMode: MouseTrackingMode;
  readonly applicationCursorKeys: boolean;
  readonly cols: number;
  readonly rows: number;
  readonly col: number;
  readonly row: number;
}

export interface AltScreenWheelEvent {
  readonly deltaY: number;
  /** WheelEvent.DOM_DELTA_PIXEL (0, the default), _LINE (1) or _PAGE (2). */
  readonly deltaMode?: number;
  readonly shiftKey?: boolean;
  readonly altKey?: boolean;
  readonly ctrlKey?: boolean;
}

const SGR_SHIFT = 4;
const SGR_ALT = 8;
const SGR_CTRL = 16;

function reportsWheel(mode: MouseTrackingMode): boolean {
  return mode === "vt200" || mode === "drag" || mode === "any";
}

function clampCell(value: number, max: number): number {
  if (max < 1) return 1;
  if (!Number.isFinite(value) || value < 1) return 1;
  return Math.min(Math.floor(value), max);
}

export interface WheelAccumulator { partial: number }

/**
 * xterm's own consumeWheelEvent accounting, so a TUI scrolls at the rate the
 * shell's scrollback does: pixel deltas become lines by cell height, deltas
 * under 50px are damped to 0.3 (a macOS trackpad fires dozens of tiny events
 * per flick), and the fraction carries over to the next event. One sequence
 * per event instead of per line made alt screens scroll 5-10x too fast.
 */
export function altScreenWheelLines(
  event: AltScreenWheelEvent,
  grid: { readonly rows: number; readonly cellHeight: number },
  acc: WheelAccumulator,
): number {
  let lines = event.deltaY;
  if (event.deltaMode === 2) {
    lines *= grid.rows;
  } else if (event.deltaMode !== 1) {
    lines /= Math.max(1, grid.cellHeight);
    if (Math.abs(event.deltaY) < 50) lines *= 0.3;
  }
  acc.partial += lines;
  const whole = Math.trunc(acc.partial) || 0; // trunc(-0.3) is -0
  acc.partial -= whole;
  return whole;
}

/**
 * xterm 6 drops alt-screen wheel events whose pixel delta floors to 0 lines,
 * so a trackpad never reaches Grok/vim. The sequence for one line, in the
 * direction of deltaY; the caller repeats it per line from
 * altScreenWheelLines. SGR wheel when the TUI asked for mouse reports, else
 * a single up/down arrow.
 */
export function altScreenWheelSequence(
  event: AltScreenWheelEvent,
  state: AltScreenWheelState,
): string | null {
  if (state.bufferType !== "alternate" || event.deltaY === 0) return null;
  if (reportsWheel(state.mouseTrackingMode)) {
    let code = event.deltaY < 0 ? 64 : 65;
    if (event.shiftKey === true) code |= SGR_SHIFT;
    if (event.altKey === true) code |= SGR_ALT;
    if (event.ctrlKey === true) code |= SGR_CTRL;
    return `\x1b[<${code};${clampCell(state.col, state.cols)};${
      clampCell(state.row, state.rows)}M`;
  }
  const prefix = state.applicationCursorKeys ? "\x1bO" : "\x1b[";
  return prefix + (event.deltaY < 0 ? "A" : "B");
}

export function altScreenWheelCell(
  clientX: number,
  clientY: number,
  bounds: { left: number; top: number; width: number; height: number },
  cols: number,
  rows: number,
): { col: number; row: number } {
  if (bounds.width <= 0 || bounds.height <= 0 || cols < 1 || rows < 1) {
    return { col: 1, row: 1 };
  }
  return {
    col: clampCell(((clientX - bounds.left) / bounds.width) * cols + 1, cols),
    row: clampCell(((clientY - bounds.top) / bounds.height) * rows + 1, rows),
  };
}
