import { type KeyboardEvent as ReactKeyboardEvent, useLayoutEffect } from "react";
import type { SidebarRow } from "../sidebar-view";
import { productLabel } from "../product";

/**
 * The row controls the session list and the Threads inbox share: the overlay
 * placement, the menu keyboard model, the destructive-action confirmation and
 * the hover-preview timing. Both lists are drawn inside the Sidebar; keeping
 * these here lets the inbox reuse them without importing the Sidebar back.
 */

/**
 * A dialog layered IN FRONT of this pane. The narrow sidebar overlay is a
 * `role="dialog"` itself, and it is the surface these checks are made FOR, so
 * it carves itself out by the marker attribute it stamps on that overlay.
 */
export const FOREGROUND_DIALOG_SELECTOR =
  '[role="dialog"]:not([data-mobile-sidebar-dialog="true"]), [role="alertdialog"]';

/**
 * How long the pointer has to rest on a row before its preview opens.
 *
 * Long enough that crossing the list on the way somewhere else never summons a
 * card, short enough to feel like an answer rather than a wait. Once one card
 * is open the delay is spent: moving to the next row swaps instantly, because
 * a user who is already reading previews has asked the question once.
 */
export const SESSION_PEEK_HOVER_MS = 350;

/**
 * The grace between leaving a row and the card going away. It exists only so
 * that walking from one row to the next counts as "still previewing" — the
 * pointerleave of the old row fires before the pointerenter of the new one.
 */
export const SESSION_PEEK_LEAVE_MS = 120;

/** The two one-way row actions that are confirmed before they run. */
export type SessionConfirmKind = "delete" | "engine";

export function sessionConfirmDialog(
  row: Pick<SidebarRow, "title" | "engine">,
  kind: SessionConfirmKind,
  live: boolean,
): { title: string; description: string; confirmLabel: string; tone: "danger" | "default" } {
  const liveNote = live ? " Its live provider connection will be closed first." : "";
  if (kind === "engine") {
    return {
      title: `Archive ${row.title} in ${row.engine}?`,
      description: `It is marked archived in ${row.engine}'s own store as well as here, so it leaves both listings. Nothing in ${productLabel()} can undo that. Only ${row.engine}'s own CLI can. The transcript itself is not deleted.${liveNote}`,
      confirmLabel: `Archive in ${row.engine}`,
      tone: "default",
    };
  }
  return {
    title: `Delete ${row.title}?`,
    description: `Permanently delete this chat from ${row.engine}? The transcript is removed from the engine's own store. This cannot be undone.${liveNote}`,
    confirmLabel: live ? "Close and delete permanently" : "Delete permanently",
    tone: "danger",
  };
}

/**
 * An overlay placed against the rect captured when it opened: measured once it
 * is in the document, re-measured on every window resize, and cleared when it
 * closes. The row menu, the aux menu and the session peek card all do exactly
 * this and differ only in what "placed" means.
 */
export function useAnchoredOverlay<Box>(
  open: unknown,
  node: { current: HTMLDivElement | null },
  anchor: { current: DOMRect | null },
  setBox: (box: Box | null) => void,
  position: (anchor: DOMRect, size: { width: number; height: number },
    viewport: { width: number; height: number }) => Box,
  minWidth = 0,
): void {
  useLayoutEffect(() => {
    if (open === null) {
      setBox(null);
      return;
    }
    const place = (): void => {
      const element = node.current;
      const rect = anchor.current;
      if (element === null || rect === null) return;
      setBox(position(rect, {
        width: Math.max(element.offsetWidth, minWidth),
        height: element.offsetHeight,
      }, { width: window.innerWidth, height: window.innerHeight }));
    };
    place();
    window.addEventListener("resize", place);
    return () => { window.removeEventListener("resize", place); };
  }, [open, node, anchor, setBox, position, minWidth]);
}

/**
 * Roving tabindex for a menu: the focused item is its one Tab stop, so Tab
 * leaves the menu instead of walking every item. The items are conditional
 * JSX rather than a mapped list, so the DOM is the index, as in menuKeyNav.
 */
export function roveMenuTabIndex(menu: HTMLElement, target: EventTarget | null): void {
  for (const item of menu.querySelectorAll<HTMLElement>('[role="menuitem"]')) {
    item.tabIndex = item === target ? 0 : -1;
  }
}

/**
 * Roving focus inside an open menu: arrows wrap, Home/End jump, a printable key
 * type-aheads to the next item that starts with it, Escape closes. The row menu
 * and the aux menu differ only in which items they let focus land on and in
 * what closing means, so both hand those two facts to this.
 */
export function menuKeyNav(event: ReactKeyboardEvent<HTMLDivElement>, selector: string,
    onEscape: () => void): void {
  if (event.key === "Escape") {
    event.preventDefault();
    onEscape();
    return;
  }
  const items = [...event.currentTarget.querySelectorAll<HTMLButtonElement>(selector)];
  const current = items.indexOf(document.activeElement as HTMLButtonElement);
  let next = -1;
  if (event.key === "ArrowDown") next = current < 0 ? 0 : (current + 1) % items.length;
  else if (event.key === "ArrowUp") {
    next = current < 0 ? items.length - 1 : (current - 1 + items.length) % items.length;
  } else if (event.key === "Home") next = 0;
  else if (event.key === "End") next = items.length - 1;
  else if (event.key.length === 1 && !event.metaKey && !event.ctrlKey) {
    const needle = event.key.toLowerCase();
    const start = Math.max(0, current + 1);
    const ordered = [...items.slice(start), ...items.slice(0, start)];
    const match = ordered.find(item => item.textContent?.trim().toLowerCase()
      .startsWith(needle));
    if (match !== undefined) {
      event.preventDefault();
      match.focus();
    }
    return;
  }
  if (next >= 0 && items[next] !== undefined) {
    event.preventDefault();
    items[next].focus();
  }
}
