import { Component, createRef, type ReactNode } from "react";
import type { TerminalWorkspaceState } from "../terminal-workspace";
import { terminalPaneMotionKey, terminalPaneRatiosChanged, TERMINAL_PANE_LAYOUT_EVENT } from "../terminal-pane-motion";
import "../terminal-pane-motion.css";

interface Props {
  layout: TerminalWorkspaceState;
  className: string;
  children: ReactNode;
}
interface Tile { node: HTMLElement; rect: DOMRect; opacity: string }
interface Snapshot { tiles: Map<string, Tile>; ghosts: HTMLElement[] }

function tiles(container: HTMLElement): Map<string, Tile> {
  const result = new Map<string, Tile>();
  const viewport = container.getBoundingClientRect();
  for (const pane of container.querySelectorAll<HTMLElement>("[data-terminal-pane-id]")) {
    const node = pane.parentElement?.classList.contains("terminal-pane-frame")
      ? pane.parentElement : pane;
    const rect = node.getBoundingClientRect();
    if (rect.width > 0 && rect.height > 0 && rect.right > viewport.left
        && rect.left < viewport.right && rect.bottom > viewport.top && rect.top < viewport.bottom) {
      result.set(pane.dataset.terminalPaneId!, { node, rect, opacity: getComputedStyle(node).opacity });
    }
  }
  return result;
}

/** A 120ms visual exit only: never a second terminal, input target or store.
 * Copy canvases before React disposes an exiting view; keep nothing after fade.
 * Only pixels are copied: xterm's link layer is blank unless a link is hovered,
 * yet a copy is a full pane-sized canvas, and the accessibility tree is
 * transparent text. Neither shows in the fade, so neither is kept. */
function exitGhost(tile: Tile, viewport: DOMRect): HTMLElement {
  const clone = tile.node.cloneNode(true) as HTMLElement;
  const originals = tile.node.querySelectorAll("canvas");
  clone.querySelectorAll("canvas").forEach((canvas, index) => {
    if (canvas.classList.contains("xterm-link-layer")) { canvas.remove(); return; }
    try { if (originals[index]) canvas.getContext("2d")?.drawImage(originals[index], 0, 0); }
    catch { /* A lost WebGL context may have no drawable frame. */ }
  });
  for (const tree of clone.querySelectorAll(".xterm-accessibility")) tree.remove();
  for (const element of [clone, ...clone.querySelectorAll<HTMLElement>("*")]) {
    for (const attribute of [...element.attributes]) {
      if (attribute.name === "id" || attribute.name.startsWith("data-")
          || attribute.name.startsWith("aria-")) element.removeAttribute(attribute.name);
    }
  }
  // Avoid maximize selectors hiding the outgoing visual after the state commit.
  clone.classList.remove("terminal-pane-frame");
  Object.assign(clone.style, { display: "grid", position: "fixed", margin: "0",
    left: `${tile.rect.left}px`, top: `${tile.rect.top}px`, width: `${tile.rect.width}px`,
    height: `${tile.rect.height}px`, transform: "none", opacity: tile.opacity,
    gridTemplateColumns: getComputedStyle(tile.node).gridTemplateColumns,
    gridTemplateRows: getComputedStyle(tile.node).gridTemplateRows,
    clipPath: `inset(${Math.max(0, viewport.top - tile.rect.top)}px ${Math.max(0, tile.rect.right - viewport.right)}px ${Math.max(0, tile.rect.bottom - viewport.bottom)}px ${Math.max(0, viewport.left - tile.rect.left)}px)` });
  clone.classList.add("terminal-motion-ghost");
  clone.inert = true;
  clone.setAttribute("aria-hidden", "true");
  return clone;
}

/** Frees a ghost's canvas backing stores now rather than at garbage collection. */
function releaseGhost(ghost: HTMLElement): void {
  for (const canvas of ghost.querySelectorAll("canvas")) { canvas.width = 0; canvas.height = 0; }
  ghost.remove();
}

/** React's before-mutation snapshot is essential here: a layout-effect cleanup
 * runs too late to measure the old grid. No canonical layout or terminal state
 * lives in this boundary; it only animates the existing DOM between commits. */
export class TerminalPaneMotion extends Component<Props> {
  private container = createRef<HTMLElement>();
  private reduced: MediaQueryList | null = null;
  private cleanups = new Set<() => void>();
  private stop = (): void => {
    for (const cleanup of [...this.cleanups]) cleanup();
    this.container.current?.removeAttribute("data-pane-motion-active");
  };
  private visibility = (): void => { if (document.hidden) this.stop(); };
  private scroll = (event: Event): void => {
    // Terminal scrollback remains interactive; only moving the workspace itself
    // invalidates viewport coordinates used by the short-lived exit visuals.
    if (event.target === this.container.current) this.stop();
  };
  componentDidMount(): void {
    this.reduced = window.matchMedia("(prefers-reduced-motion: reduce)");
    this.reduced.addEventListener("change", this.stop);
    window.addEventListener("resize", this.stop);
    document.addEventListener("visibilitychange", this.visibility);
    this.container.current?.addEventListener("scroll", this.scroll);
  }
  componentWillUnmount(): void {
    this.stop();
    this.reduced?.removeEventListener("change", this.stop);
    window.removeEventListener("resize", this.stop);
    document.removeEventListener("visibilitychange", this.visibility);
    this.container.current?.removeEventListener("scroll", this.scroll);
  }
  getSnapshotBeforeUpdate(previous: Props): Snapshot | null {
    if (terminalPaneMotionKey(previous.layout) === terminalPaneMotionKey(this.props.layout)) {
      if (terminalPaneRatiosChanged(previous.layout, this.props.layout)) this.stop();
      return null;
    }
    const container = this.container.current;
    if (!container || this.reduced?.matches || document.hidden
        || typeof container.animate !== "function") { this.stop(); return null; }
    const before = tiles(container);
    const next = new Set(this.props.layout.panes.map(pane => pane.id));
    const maximized = this.props.layout.maximizedPaneId;
    const ghosts: HTMLElement[] = [];
    for (const [id, tile] of before) {
      if (!next.has(id) || (maximized !== null && id !== maximized)) ghosts.push(exitGhost(tile, container.getBoundingClientRect()));
    }
    // Measure the CURRENT interpolated positions before cancelling. Rapid
    // maximize/restore or swaps therefore continue from the visible position.
    this.stop();
    return { tiles: before, ghosts };
  }
  componentDidUpdate(_previous: Props, _state: unknown, snapshot: Snapshot | null): void {
    if (!snapshot) return;
    if (!this.container.current) { snapshot.ghosts.forEach(releaseGhost); return; }
    const container = this.container.current;
    const after = tiles(container);
    // Read every destination and terminal body before applying any size override.
    const bodies = new Map<HTMLElement, { node: HTMLElement; width: number; height: number }[]>();
    for (const { node } of after.values()) {
      bodies.set(node, [...node.querySelectorAll<HTMLElement>(".terminal-pane-body")]
        .map(body => ({ node: body, width: body.offsetWidth, height: body.offsetHeight })));
    }
    for (const ghost of snapshot.ghosts) {
      document.body.append(ghost);
      this.play(ghost, [{ opacity: ghost.style.opacity }, { opacity: 0 }], 120, () => releaseGhost(ghost));
    }
    for (const [id, tile] of after) {
      const before = snapshot.tiles.get(id);
      if (!before) {
        this.play(tile.node, [{ opacity: 0 }, { opacity: 1 }], 140);
        continue;
      }
      const dx = before.rect.left - tile.rect.left, dy = before.rect.top - tile.rect.top;
      const resized = Math.abs(before.rect.width - tile.rect.width) > 0.5
        || Math.abs(before.rect.height - tile.rect.height) > 0.5;
      if (!resized && Math.abs(dx) < 0.5 && Math.abs(dy) < 0.5) continue;
      const restoreBodies: (() => void)[] = [];
      if (resized) for (const body of bodies.get(tile.node) ?? []) {
        const style = body.node.getAttribute("style");
        Object.assign(body.node.style, { flex: "none", width: `${body.width}px`, height: `${body.height}px` });
        restoreBodies.push(() => {
          if (style === null) body.node.removeAttribute("style"); else body.node.setAttribute("style", style);
        });
      }
      tile.node.setAttribute("data-pane-motion-tile", "");
      this.play(tile.node, [
        { transform: `translate(${dx}px, ${dy}px)`, width: `${before.rect.width}px`,
          height: `${before.rect.height}px`, opacity: before.opacity },
        { transform: "translate(0px, 0px)", width: `${tile.rect.width}px`,
          height: `${tile.rect.height}px`, opacity: 1 },
      ], resized ? 260 : 200, () => {
        tile.node.removeAttribute("data-pane-motion-tile");
        for (const restore of restoreBodies) restore();
      });
    }
    for (const { node } of after.values()) {
      for (const host of node.querySelectorAll(".terminal-xterm")) {
        host.dispatchEvent(new Event(TERMINAL_PANE_LAYOUT_EVENT));
      }
    }
  }
  private play(node: HTMLElement, frames: Keyframe[], duration: number, release = (): void => {}): void {
    this.container.current?.setAttribute("data-pane-motion-active", "");
    const animation = node.animate(frames, { duration, easing: "cubic-bezier(.16, 1, .3, 1)", fill: "both" });
    animation.id = "terminal-pane-motion";
    let timer: ReturnType<typeof setTimeout>;
    const cleanup = (): void => {
      this.cleanups.delete(cleanup);
      clearTimeout(timer);
      animation.onfinish = animation.oncancel = null;
      animation.cancel();
      release();
      if (this.cleanups.size === 0) this.container.current?.removeAttribute("data-pane-motion-active");
    };
    this.cleanups.add(cleanup);
    animation.onfinish = animation.oncancel = cleanup;
    // Occlusion can suspend animation events; no frozen body survives the bound.
    timer = setTimeout(cleanup, duration + 100);
  }
  render(): ReactNode {
    return <section ref={this.container} className={this.props.className}
      data-terminal-arrangement={this.props.layout.arrangement} aria-label="Terminal workspace">
      {this.props.children}
    </section>;
  }
}
