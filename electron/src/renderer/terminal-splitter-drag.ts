import {
  MAX_TERMINAL_SPLIT_RATIO, MIN_TERMINAL_SPLIT_RATIO,
  type TerminalSplitOrientation,
} from "./terminal-workspace";

export interface TerminalSplitterRect {
  readonly left: number;
  readonly top: number;
  readonly width: number;
  readonly height: number;
}

export interface TerminalSplitterPointer {
  readonly pointerId: number;
  readonly clientX: number;
  readonly clientY: number;
  readonly buttons: number;
  preventDefault(): void;
}

interface TerminalSplitterDragListeners {
  readonly move: (event: TerminalSplitterPointer) => void;
  readonly stop: (event: TerminalSplitterPointer | null) => void;
  readonly cancel: (event: TerminalSplitterPointer | null) => void;
}

export interface TerminalSplitterDragEnvironment {
  listen(listeners: TerminalSplitterDragListeners): () => void;
  requestFrame(callback: () => void): number;
  cancelFrame(handle: number): void;
}

export interface TerminalSplitterDragStart {
  readonly pointerId: number;
  readonly clientX: number;
  readonly clientY: number;
  readonly orientation: TerminalSplitOrientation;
  readonly container: TerminalSplitterRect;
  readonly splitter: TerminalSplitterRect;
  readonly onRatioChange: (ratio: number) => void;
  readonly onFinish?: () => void;
}

interface ActiveTerminalSplitterDrag {
  readonly pointerId: number;
  readonly orientation: TerminalSplitOrientation;
  readonly container: TerminalSplitterRect;
  readonly splitterSize: number;
  readonly grabOffset: number;
  readonly onRatioChange: (ratio: number) => void;
  readonly onFinish?: () => void;
  lastRatio: number | null;
}

function finiteRect(rect: TerminalSplitterRect): boolean {
  return Number.isFinite(rect.left) && Number.isFinite(rect.top)
    && Number.isFinite(rect.width) && Number.isFinite(rect.height)
    && rect.width > 0 && rect.height > 0;
}

function clampRatio(ratio: number): number {
  return Math.min(MAX_TERMINAL_SPLIT_RATIO, Math.max(MIN_TERMINAL_SPLIT_RATIO, ratio));
}

/**
 * Convert the pointer position to the fraction consumed by the first grid
 * track. The divider is an `auto` track, so its own width/height is excluded
 * from the flexible distance. Keeping the initial grab offset prevents the bar
 * from jumping when it is picked up near either edge of its hit target.
 */
export function terminalSplitterRatio(input: {
  readonly orientation: TerminalSplitOrientation;
  readonly container: TerminalSplitterRect;
  readonly splitterSize: number;
  readonly grabOffset: number;
  readonly clientX: number;
  readonly clientY: number;
}): number | null {
  const horizontal = input.orientation === "horizontal";
  const start = horizontal ? input.container.left : input.container.top;
  const size = horizontal ? input.container.width : input.container.height;
  const coordinate = horizontal ? input.clientX : input.clientY;
  const travel = size - input.splitterSize;
  if (!Number.isFinite(coordinate) || !Number.isFinite(input.splitterSize)
      || !Number.isFinite(input.grabOffset) || travel <= 0) return null;
  return clampRatio((coordinate - input.grabOffset - start) / travel);
}

/**
 * One global drag session for a terminal workspace. Listening on `window` in
 * the capture phase keeps the gesture alive while the pointer crosses xterm's
 * canvas or the divider moves under it. Updates are coalesced to one per paint;
 * release still flushes the exact final coordinate synchronously.
 */
export class TerminalSplitterDragController {
  private active: ActiveTerminalSplitterDrag | null = null;
  private stopListening: (() => void) | null = null;
  private frame: number | null = null;
  private pendingRatio: number | null = null;

  constructor(private readonly environment: TerminalSplitterDragEnvironment) {}

  start(input: TerminalSplitterDragStart): boolean {
    this.cancel();
    if (!finiteRect(input.container) || !finiteRect(input.splitter)
        || !Number.isInteger(input.pointerId)) return false;
    const horizontal = input.orientation === "horizontal";
    const splitterStart = horizontal ? input.splitter.left : input.splitter.top;
    const splitterSize = horizontal ? input.splitter.width : input.splitter.height;
    const coordinate = horizontal ? input.clientX : input.clientY;
    if (!Number.isFinite(coordinate) || splitterSize <= 0
        || splitterSize >= (horizontal ? input.container.width : input.container.height)) {
      return false;
    }
    this.active = {
      pointerId: input.pointerId,
      orientation: input.orientation,
      container: input.container,
      splitterSize,
      grabOffset: coordinate - splitterStart,
      onRatioChange: input.onRatioChange,
      onFinish: input.onFinish,
      lastRatio: null,
    };
    this.stopListening = this.environment.listen({
      move: event => { this.move(event); },
      stop: event => { this.stop(event); },
      cancel: event => { this.abort(event); },
    });
    return true;
  }

  cancel(): void {
    if (this.frame !== null) this.environment.cancelFrame(this.frame);
    this.frame = null;
    this.pendingRatio = null;
    this.stopListening?.();
    this.stopListening = null;
    const finish = this.active?.onFinish;
    this.active = null;
    finish?.();
  }

  dispose(): void {
    this.cancel();
  }

  cancelPointer(pointerId: number): void {
    if (pointerId !== this.active?.pointerId) return;
    this.abort(null);
  }

  private move(event: TerminalSplitterPointer): void {
    if (event.pointerId !== this.active?.pointerId) return;
    // A release outside the Electron content window can miss pointerup. The
    // first hover event back inside is the recovery edge, not another drag.
    if (event.buttons === 0) {
      this.abort(event);
      return;
    }
    event.preventDefault();
    this.queue(event.clientX, event.clientY);
  }

  private stop(event: TerminalSplitterPointer | null): void {
    if (this.active === null || event !== null && event.pointerId !== this.active.pointerId) return;
    try {
      if (event !== null) {
        event.preventDefault();
        this.queue(event.clientX, event.clientY);
      }
      this.flush();
    } finally {
      this.cancel();
    }
  }

  private abort(event: TerminalSplitterPointer | null): void {
    if (this.active === null || event !== null && event.pointerId !== this.active.pointerId) return;
    // A canceled PointerEvent is not a final coordinate; Chromium may report
    // 0,0 here. Preserve the last real move instead of snapping to the minimum.
    try {
      this.flush();
    } finally {
      this.cancel();
    }
  }

  private queue(clientX: number, clientY: number): void {
    const active = this.active;
    if (active === null) return;
    const ratio = terminalSplitterRatio({
      orientation: active.orientation,
      container: active.container,
      splitterSize: active.splitterSize,
      grabOffset: active.grabOffset,
      clientX,
      clientY,
    });
    if (ratio === null) return;
    this.pendingRatio = ratio;
    if (this.frame !== null) return;
    this.frame = this.environment.requestFrame(() => {
      this.frame = null;
      this.flush();
    });
  }

  private flush(): void {
    if (this.frame !== null) this.environment.cancelFrame(this.frame);
    this.frame = null;
    const active = this.active;
    const ratio = this.pendingRatio;
    this.pendingRatio = null;
    if (active === null || ratio === null || ratio === active.lastRatio) return;
    active.lastRatio = ratio;
    active.onRatioChange(ratio);
  }
}

export function browserTerminalSplitterDragEnvironment(): TerminalSplitterDragEnvironment {
  return {
    listen: listeners => {
      const move = (event: PointerEvent): void => { listeners.move(event); };
      const stop = (event: PointerEvent): void => { listeners.stop(event); };
      const cancel = (event: PointerEvent): void => { listeners.cancel(event); };
      const blur = (): void => { listeners.cancel(null); };
      window.addEventListener("pointermove", move, true);
      window.addEventListener("pointerup", stop, true);
      window.addEventListener("pointercancel", cancel, true);
      window.addEventListener("blur", blur);
      return () => {
        window.removeEventListener("pointermove", move, true);
        window.removeEventListener("pointerup", stop, true);
        window.removeEventListener("pointercancel", cancel, true);
        window.removeEventListener("blur", blur);
      };
    },
    requestFrame: callback => window.requestAnimationFrame(callback),
    cancelFrame: handle => { window.cancelAnimationFrame(handle); },
  };
}
