import { useCallback, useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { FOREGROUND_DIALOG_SELECTOR } from "./components/sidebar-controls";

/**
 * The hidden sidebar's hover reveal (owner, 2026-10-20, after Codex): with the
 * sidebar hidden, resting the pointer on its toggle beside the traffic lights
 * slides it back over the work, right under the pointer, and it slides away
 * again once the pointer leaves. It floats rather than reopening its column,
 * so the panes never move or refit. The window's left edge did this first; the
 * owner found it too far to reach.
 */

/** How long the pointer rests on the toggle before the panel comes. Long
 * enough that a pointer passing it on the way to the traffic lights never
 * summons it, short enough to feel like the toggle answered. */
export const SIDEBAR_PEEK_OPEN_MS = 120;

/** The grace after the pointer leaves before the panel goes: overshooting its
 * edge by a few pixels and coming straight back keeps it. */
export const SIDEBAR_PEEK_CLOSE_MS = 300;

/** What counts as still being in the revealed panel: the panel, the menus and
 * hover card the sidebar portals to document.body, and whatever carries
 * `data-sidebar-peek-keep` (the rail's toggle, where a click keeps it for
 * good). */
export const SIDEBAR_PEEK_KEEP_SELECTOR =
  ".sidebar, .row-menu, .session-peek, [data-sidebar-peek-keep]";

export interface PeekClock {
  set(run: () => void, ms: number): number;
  clear(handle: number): void;
}

const BROWSER_CLOCK: PeekClock = {
  set: (run, ms) => window.setTimeout(run, ms),
  clear: handle => { window.clearTimeout(handle); },
};

/** The reveal's decisions, apart from React and the DOM so a fake clock can
 * drive them. `held` says something in the panel still needs it (an open
 * menu, its own dialog, a search being typed); `onShow` reports each change. */
export class SidebarPeek {
  private enabled = true;
  private shown = false;
  private inside = false;
  private timer: number | null = null;

  constructor(private readonly onShow: (shown: boolean) => void,
      private readonly held: () => boolean,
      private readonly clock: PeekClock = BROWSER_CLOCK) {}

  /** Whether the sidebar is hidden, so the toggle reveals it. Docking it
   * again slides the reveal away at once. */
  setEnabled(enabled: boolean): void {
    this.enabled = enabled;
    if (!enabled) this.dismiss();
  }

  /** The pointer came onto the toggle. A pressed button is a drag passing
   * over it, which never summons the panel; so is the pointer that was already
   * there when a click hid the sidebar, until it leaves and comes back. */
  triggerEnter(buttons: number): void {
    if (!this.enabled) return;
    this.inside = true;
    if (this.shown) this.cancel();
    else if (buttons === 0 && this.timer === null) this.arm(SIDEBAR_PEEK_OPEN_MS, () => { this.show(true); });
  }

  /** The pointer left the toggle before the panel came. */
  triggerLeave(): void {
    if (this.shown) return;
    this.inside = false;
    this.cancel();
  }

  /** The pointer is over the revealed panel or something it owns. */
  enter(): void {
    this.inside = true;
    if (this.shown) this.cancel();
  }

  /** The pointer is over anything else, or has left the window. */
  leave(): void {
    this.inside = false;
    if (!this.shown) this.cancel();
    else if (this.timer === null) this.arm(SIDEBAR_PEEK_CLOSE_MS, () => { this.settle(); });
  }

  /** Slides it away now: a press outside it, or the reveal switched off. */
  dismiss(): void {
    this.cancel();
    this.inside = false;
    this.show(false);
  }

  /** Unmounting: stop the clock without reporting anything. */
  dispose(): void { this.cancel(); }

  private settle(): void {
    if (this.inside) return;
    // Whatever holds it (a menu, a confirmation, focus in its search box)
    // ends on its own; looking again shortly is simpler than tracking each.
    if (this.held()) this.arm(SIDEBAR_PEEK_CLOSE_MS, () => { this.settle(); });
    else this.show(false);
  }

  private arm(ms: number, run: () => void): void {
    this.cancel();
    this.timer = this.clock.set(() => { this.timer = null; run(); }, ms);
  }

  private cancel(): void {
    if (this.timer === null) return;
    this.clock.clear(this.timer);
    this.timer = null;
  }

  private show(shown: boolean): void {
    if (shown === this.shown) return;
    this.shown = shown;
    this.onShow(shown);
  }
}

/** Whether `target` is the panel or something that belongs to it. */
export function keepsSidebarPeek(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest(SIDEBAR_PEEK_KEEP_SELECTOR) !== null;
}

/** Something in the panel is still in use: a menu or confirmation it opened,
 * or a text field in it that has focus (a search being typed). */
export function sidebarPeekHeld(doc: Document = document): boolean {
  if (doc.querySelector(`.row-menu, ${FOREGROUND_DIALOG_SELECTOR}`) !== null) return true;
  const active = doc.activeElement;
  return active !== null && active.closest(".sidebar") !== null
    && active.matches("input, textarea, [contenteditable='true']");
}

export interface SidebarPeekTrigger {
  onPointerEnter: (event: ReactPointerEvent) => void;
  onPointerLeave: () => void;
}

/** Wires the reveal while `enabled` (the sidebar is hidden on a desktop-width
 * window). Spread `trigger` on the sidebar toggle; `shown` is whether the
 * panel is out. Switching it off slides the panel away at once. */
export function useSidebarPeek(enabled: boolean): { shown: boolean; trigger: SidebarPeekTrigger } {
  const [shown, setShown] = useState(false);
  const peekRef = useRef<SidebarPeek | null>(null);
  peekRef.current ??= new SidebarPeek(setShown, sidebarPeekHeld);
  const peek = peekRef.current;
  useEffect(() => { peek.setEnabled(enabled); }, [enabled, peek]);
  useEffect(() => () => { peek.dispose(); }, [peek]);
  useEffect(() => {
    if (!shown) return undefined;
    // Capture phase, so a handler that stops propagation cannot hide where the
    // pointer went.
    const over = (event: PointerEvent): void => {
      if (keepsSidebarPeek(event.target)) peek.enter();
      else peek.leave();
    };
    // Off the window, or into the rail's drag region, which takes no events.
    const out = (event: PointerEvent): void => { if (event.relatedTarget === null) peek.leave(); };
    const down = (event: PointerEvent): void => {
      if (!keepsSidebarPeek(event.target) && !sidebarPeekHeld()) peek.dismiss();
    };
    document.addEventListener("pointerover", over, true);
    document.addEventListener("pointerout", out, true);
    document.addEventListener("pointerdown", down, true);
    return () => {
      document.removeEventListener("pointerover", over, true);
      document.removeEventListener("pointerout", out, true);
      document.removeEventListener("pointerdown", down, true);
    };
  }, [peek, shown]);
  // Entering only, never moving: a click that hides the sidebar leaves the
  // pointer resting on the toggle, and that must not bring it straight back.
  const onPointerEnter = useCallback((event: ReactPointerEvent): void => { peek.triggerEnter(event.buttons); }, [peek]);
  const onPointerLeave = useCallback((): void => { peek.triggerLeave(); }, [peek]);
  return { shown: enabled && shown, trigger: { onPointerEnter, onPointerLeave } };
}
