import type { TerminalSplitTree, TerminalWorkspaceState } from "./terminal-workspace";

function topology(tree: TerminalSplitTree | null): unknown {
  if (tree === null) return null;
  return tree.kind === "pane" ? tree.paneId
    : [tree.orientation, topology(tree.first), topology(tree.second)];
}

/** Ratios deliberately do not participate: a divider follows the pointer/keys
 * directly. Focus, titles and output must never restart a layout animation. */
export function terminalPaneMotionKey(layout: TerminalWorkspaceState): string {
  return JSON.stringify([layout.arrangement, layout.maximizedPaneId, topology(layout.root)]);
}

function ratios(tree: TerminalSplitTree | null, into: number[] = []): number[] {
  if (tree === null || tree.kind === "pane") return into;
  into.push(tree.ratio);
  ratios(tree.first, into);
  ratios(tree.second, into);
  return into;
}

/** A divider moved between two layouts with the same motion key. A rebuilt
 * but identical tree (React re-running a layout updater) is not a move, and
 * must not cut short the glide its first commit started. */
export function terminalPaneRatiosChanged(previous: TerminalWorkspaceState,
    next: TerminalWorkspaceState): boolean {
  if (previous.root === next.root) return false;
  const before = ratios(previous.root), after = ratios(next.root);
  return before.length !== after.length || before.some((ratio, index) => ratio !== after[index]);
}

/** Renderer-local notification after a mounted emulator changes its visual slot. */
export const TERMINAL_PANE_LAYOUT_EVENT = "interface-terminal-pane-layout";

/** Set on an ancestor whose columns are sliding; emulators inside skip resize fits. */
export const TERMINAL_LAYOUT_SLIDING = "data-layout-sliding";

/** Set on a split whose divider is being dragged; emulators inside throttle their resize fits. */
export const TERMINAL_LAYOUT_DRAGGING = "data-layout-dragging";

/** How often an emulator under a dragged divider refits: a fit clears, resizes
 * and reflows the emulator (5,000 lines of scrollback) and its canvas, which
 * every animation frame of a drag cannot afford for both sides of the bar. */
export const TERMINAL_DRAG_FIT_INTERVAL_MS = 50;

/** Holds every emulator in `container` at its current grid while the
 * container's column transition runs, then refits each once. A fit per frame
 * blanks the canvases and resizes every PTY a dozen times over one slide.
 * Returns the settle function (safe to call early), or null with no slide. */
export function holdTerminalsWhileSliding(container: HTMLElement): (() => void) | null {
  return holdTerminalsDuring(container, container.getAnimations?.().find(animation =>
    (animation as CSSTransition).transitionProperty === "grid-template-columns"));
}

/** The same hold for any animation that resizes `container`'s panes, such as
 * a side panel sliding in beside them. Null when there is no `slide`. */
export function holdTerminalsDuring(container: HTMLElement, slide: Animation | undefined): (() => void) | null {
  if (slide === undefined) return null;
  const settle = holdTerminals(container);
  // A re-toggle cancels the animation, which rejects `finished`: settle too.
  slide.finished.then(settle, settle);
  return settle;
}

/** Holds every emulator in `container` at its current grid until the returned
 * settle runs (once; later calls do nothing), then refits each once: for a
 * resize the pointer drives, such as dragging a side panel's edge. */
export function holdTerminals(container: HTMLElement): () => void {
  return markTerminals(container, TERMINAL_LAYOUT_SLIDING);
}

/** Throttles every emulator in `container` while a divider drag resizes it:
 * each refits live at most once per TERMINAL_DRAG_FIT_INTERVAL_MS rather than
 * every frame, and each refits once more, to the exact final geometry, when
 * the returned settle runs (once; later calls do nothing). */
export function throttleTerminalFits(container: HTMLElement): () => void {
  return markTerminals(container, TERMINAL_LAYOUT_DRAGGING);
}

function markTerminals(container: HTMLElement, attribute: string): () => void {
  container.setAttribute(attribute, "");
  let held = true;
  return () => {
    if (!held) return;
    held = false;
    container.removeAttribute(attribute);
    for (const host of container.querySelectorAll(".terminal-xterm")) {
      host.dispatchEvent(new Event(TERMINAL_PANE_LAYOUT_EVENT));
    }
  };
}

/** One emulator's fits under a dragged divider, as a trailing throttle: the
 * first resize of a quiet spell schedules a fit `intervalMs` later, which
 * measures the geometry as it is by then; resizes before it fires ride along.
 * The settle's pane layout event cancels a pending fit and fits exactly. */
export class TerminalFitThrottle {
  private timer: ReturnType<typeof setTimeout> | null = null;

  constructor(private readonly fit: () => void,
    private readonly intervalMs = TERMINAL_DRAG_FIT_INTERVAL_MS) {}

  request(): void {
    if (this.timer !== null) return;
    this.timer = setTimeout(() => {
      this.timer = null;
      this.fit();
    }, this.intervalMs);
  }

  cancel(): void {
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
  }
}
