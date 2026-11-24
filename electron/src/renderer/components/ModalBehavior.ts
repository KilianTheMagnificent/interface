import { useEffect, useRef } from "react";
import type {
  KeyboardEvent as ReactKeyboardEvent,
  RefObject,
} from "react";

export type ModalKeyAction =
  | { type: "dismiss" }
  | { type: "focus"; index: number }
  | { type: "focus-container" };

export type RovingTabAction = { index: number };

export const MODAL_FOCUSABLE_SELECTOR = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled]):not([type='hidden'])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "[tabindex]:not([tabindex='-1'])",
].join(",");

/**
 * Pure keyboard decision used by every modal surface. Focus wraps at both
 * ends, Shift+Tab entering from outside lands on the final control, and a
 * control-free dialog retains focus on its labelled container.
 */
export function modalKeyAction(
  key: string,
  activeIndex: number,
  shiftKey = false,
  controlCount = 0,
  escapeEnabled = true,
): ModalKeyAction | null {
  if (key === "Escape" && escapeEnabled) return { type: "dismiss" };
  if (key !== "Tab") return null;
  if (controlCount <= 0) return { type: "focus-container" };
  if (activeIndex < 0 || activeIndex >= controlCount) {
    return { type: "focus", index: shiftKey ? controlCount - 1 : 0 };
  }
  const delta = shiftKey ? -1 : 1;
  return {
    type: "focus",
    index: (activeIndex + delta + controlCount) % controlCount,
  };
}

/** The keys modalKeyAction can answer with anything but null. */
export function modalKeyMayAct(key: string, escapeEnabled = true): boolean {
  return key === "Tab" || (key === "Escape" && escapeEnabled);
}

/** One-tab-stop movement for a horizontal tablist. */
export function rovingTabKeyAction(
  key: string,
  currentIndex: number,
  count: number,
): RovingTabAction | null {
  if (count <= 0) return null;
  if (key === "Home") return { index: 0 };
  if (key === "End") return { index: count - 1 };
  if (key === "ArrowRight" || key === "ArrowDown") {
    return { index: (Math.max(currentIndex, 0) + 1) % count };
  }
  if (key === "ArrowLeft" || key === "ArrowUp") {
    return { index: (currentIndex <= 0 ? count : currentIndex) - 1 };
  }
  return null;
}

/** Keep focus on the same row position, falling back to the previous row. */
export function logicalListFocusIndex(previousIndex: number, nextLength: number): number {
  if (nextLength <= 0) return -1;
  return Math.min(Math.max(previousIndex, 0), nextLength - 1);
}

/**
 * Its own selector, deliberately NOT `MODAL_FOCUSABLE_SELECTOR`: a popover Tab
 * hands the caret to the next control in the DOCUMENT, so it has to match what
 * the browser's own tab order would, contenteditable included, and it does not
 * need the modal list's hidden-input exclusion.
 */
const POPOVER_TAB_SELECTOR = [
  "a[href]", "button:not([disabled])", "input:not([disabled])",
  "select:not([disabled])", "textarea:not([disabled])",
  "[tabindex]:not([tabindex='-1'])", "[contenteditable='true']",
].join(",");

/**
 * The control Tab would reach from `trigger` if the open popover were not
 * there. `exclude` is the popover itself: its own controls are not tab stops on
 * the way out. Returns null when the trigger is not in the document's tab order
 * at all, which is the caller's cue to leave focus where it is.
 */
export function focusPastTrigger(
  trigger: HTMLElement | null,
  backwards: boolean,
  exclude: Element | null,
): HTMLElement | null {
  if (trigger === null) return null;
  const focusable = [...document.querySelectorAll<HTMLElement>(POPOVER_TAB_SELECTOR)]
    .filter(node => node.getClientRects().length > 0
      && node.closest("[hidden], [inert]") === null
      && exclude?.contains(node) !== true);
  const at = focusable.indexOf(trigger);
  return at < 0 ? null : focusable[at + (backwards ? -1 : 1)] ?? null;
}

interface UseModalFocusOptions<T extends HTMLElement> {
  open: boolean;
  containerRef: RefObject<T | null>;
  initialFocus?: () => HTMLElement | null;
  onEscape: () => void;
  escapeEnabled?: boolean;
  /** Override the captured opener when a completed action needs another target. */
  returnFocusRef?: RefObject<HTMLElement | null>;
}

const canReceiveFocus = (node: HTMLElement | null): node is HTMLElement =>
  node !== null
  && node.isConnected
  && !node.matches(":disabled")
  && node.closest("[hidden], [aria-hidden='true'], [inert]") === null;

const focusableControls = (container: HTMLElement | null): HTMLElement[] =>
  [...(container?.querySelectorAll<HTMLElement>(MODAL_FOCUSABLE_SELECTOR) ?? [])]
    .filter(node => canReceiveFocus(node) && node.getClientRects().length > 0);

/**
 * Shared modal lifecycle: capture the opener, move to a safe initial target,
 * contain Tab focus, use Escape as the safe exit, and restore focus on close.
 */
export function useModalFocus<T extends HTMLElement>({
  open,
  containerRef,
  initialFocus,
  onEscape,
  escapeEnabled = true,
  returnFocusRef,
}: UseModalFocusOptions<T>): (event: ReactKeyboardEvent<T>) => void {
  const priorFocus = useRef<HTMLElement | null>(null);
  const latest = useRef({ initialFocus, onEscape, escapeEnabled, returnFocusRef });
  latest.current = { initialFocus, onEscape, escapeEnabled, returnFocusRef };

  useEffect(() => {
    if (!open) return;
    priorFocus.current = document.activeElement instanceof HTMLElement
      ? document.activeElement : null;
    const container = containerRef.current;
    const requested = latest.current.initialFocus?.() ?? null;
    const target = canReceiveFocus(requested)
      ? requested : focusableControls(container)[0] ?? container;
    target?.focus();

    return () => {
      const override = latest.current.returnFocusRef?.current ?? null;
      const targetToRestore = canReceiveFocus(override)
        ? override : canReceiveFocus(priorFocus.current) ? priorFocus.current : null;
      priorFocus.current = null;
      targetToRestore?.focus();
    };
  }, [containerRef, open]);

  return (event: ReactKeyboardEvent<T>): void => {
    // Every character typed into a dialog's field reaches this handler; only
    // the keys modalKeyAction can act on pay for the focusable-control scan.
    if (!modalKeyMayAct(event.key, latest.current.escapeEnabled)) return;
    const controls = focusableControls(containerRef.current);
    const activeIndex = controls.findIndex(node => node === document.activeElement);
    const action = modalKeyAction(
      event.key,
      activeIndex,
      event.shiftKey,
      controls.length,
      latest.current.escapeEnabled,
    );
    if (action === null) return;
    event.preventDefault();
    event.stopPropagation();
    if (action.type === "dismiss") latest.current.onEscape();
    else if (action.type === "focus-container") containerRef.current?.focus();
    else controls[action.index]?.focus();
  };
}
