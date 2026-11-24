import {
  useEffect, useLayoutEffect, useRef, useState,
  type KeyboardEvent as ReactKeyboardEvent, type RefObject,
} from "react";
import { positionAppSelect, type AppSelectPosition } from "./AppSelect";
import { ProviderLogo } from "./ProviderLogo";
import "../terminal.css";

export type TerminalMenuStatus = "starting" | "running" | "exited" | "failed";

export interface TerminalMenuTerminal {
  readonly id: string;
  readonly title: string;
  readonly cwd: string;
  readonly status: TerminalMenuStatus;
  readonly active?: boolean;
  readonly attached?: boolean;
  readonly exitCode?: number | null;
  /** Session association copied by profile-less pane splits. */
  readonly sessionKey?: string;
  /** Renderer-derived: working, waiting for input, or quiet. */
  readonly activity?: "idle" | "working" | "waiting";
  /**
   * Launch profile this terminal was opened with, absent for a plain shell.
   * Titles follow the running program's own OSC announcements, so the engine
   * is no longer readable from the label and has to be shown as a mark.
   */
  readonly profileId?: string;
}

/** The mark a Browser tab carries where a terminal carries its engine's. */
export const BROWSER_PROFILE_ID = "browser";

/**
 * The engine mark for a terminal. Profile-less splits keep their source's
 * session association, which is enough to inherit its provider mark without
 * relaunching the provider. An explicitly selected shell remains a shell.
 */
export function TerminalProfileIcon({ profileId, sessionKey, providerLabel, size = 13 }: {
  profileId?: string;
  sessionKey?: string;
  /** Public catalog label when the stable profile id names a successor CLI. */
  providerLabel?: string;
  size?: number;
}) {
  if (profileId === "shell") {
    return <span className="codicon codicon-terminal" aria-hidden="true" />;
  }
  if (profileId === BROWSER_PROFILE_ID) {
    return <span className="codicon codicon-globe" aria-hidden="true" />;
  }
  const separator = sessionKey?.indexOf("#") ?? -1;
  const sessionEngine = separator > 0 ? sessionKey?.slice(0, separator) : undefined;
  const engine = profileId === undefined || profileId === "" ? sessionEngine : profileId;
  if (engine === undefined || engine === "") {
    return <span className="codicon codicon-terminal" aria-hidden="true" />;
  }
  const label = providerLabel?.trim()
    || (engine === "gemini" || engine === "acp:gemini" ? "Antigravity" : engine);
  return <ProviderLogo engine={engine} label={label} size={size} />;
}

export type TerminalMenuMove = "next" | "previous" | "first" | "last";

export function moveTerminalMenuIndex(count: number, current: number,
    move: TerminalMenuMove): number {
  if (count <= 0) return -1;
  if (move === "first") return 0;
  if (move === "last") return count - 1;
  if (move === "previous") return current < 0 ? count - 1 : (current - 1 + count) % count;
  return current < 0 ? 0 : (current + 1) % count;
}

export function terminalMenuTypeaheadIndex(labels: readonly string[], current: number,
    query: string): number {
  const needle = query.trim().toLocaleLowerCase();
  if (needle === "" || labels.length === 0) return -1;
  for (let offset = 1; offset <= labels.length; offset += 1) {
    const index = (Math.max(-1, current) + offset) % labels.length;
    if ((labels[index] ?? "").trim().toLocaleLowerCase().startsWith(needle)) return index;
  }
  return -1;
}

/** Placement, focus, dismissal, and roving keys shared by every terminal popover.
 *  `anchorRef` opens the popover under another control's button (the window's
 *  ··· menu) instead of its own trigger. That anchor is not part of the
 *  popover: pressing it dismisses the panel like any other outside press. */
export function useTerminalMenuPopover({ open, onOpenChange, minWidth, placeOn, maxHeight, align,
    anchorRef }: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  minWidth: number;
  placeOn?: unknown;
  maxHeight?: number;
  align?: "start" | "end";
  anchorRef?: RefObject<HTMLElement | null>;
}) {
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const [box, setBox] = useState<AppSelectPosition | null>(null);
  const opener = (): HTMLElement | null => anchorRef?.current ?? triggerRef.current;

  const close = (restoreFocus: boolean): void => {
    onOpenChange(false);
    if (restoreFocus) requestAnimationFrame(() => { opener()?.focus(); });
  };

  useLayoutEffect(() => {
    if (!open) {
      setBox(null);
      return;
    }
    let retry = 0;
    const place = (): void => {
      const trigger = opener();
      const menu = menuRef.current;
      if (trigger === null || menu === null) {
        // Another component's button may attach later in this commit.
        if (anchorRef !== undefined) retry = requestAnimationFrame(place);
        return;
      }
      // Unrounded, then up: offsetHeight rounds a fractional menu down, and the
      // max-height built from it left the menu half a pixel short and scrolling.
      const size = menu.getBoundingClientRect();
      setBox(positionAppSelect(trigger.getBoundingClientRect(), {
        width: Math.ceil(Math.max(size.width, minWidth)),
        height: Math.ceil(Math.max(size.height, menu.scrollHeight)),
      }, { width: window.innerWidth, height: window.innerHeight }, maxHeight, align));
    };
    place();
    window.addEventListener("resize", place);
    return () => {
      cancelAnimationFrame(retry);
      window.removeEventListener("resize", place);
    };
  }, [open, placeOn, maxHeight, align]);

  // Focus lands on the first item once per open. `box` changes on every resize while
  // the menu is up, and re-focusing then yanks the caret off whatever the user moved to.
  const focusedForOpen = useRef(false);
  useEffect(() => {
    if (!open) {
      focusedForOpen.current = false;
      return;
    }
    if (box === null || focusedForOpen.current) return;
    const launcher = opener();
    const previous = document.activeElement;
    const deadline = performance.now() + 1000;
    let frame = 0;
    const focusVisibleItem = (): void => {
      const active = document.activeElement;
      if (menuRef.current?.contains(active)
          || (active !== previous && active !== launcher && active !== document.body)) {
        focusedForOpen.current = true;
        return;
      }
      const first = menuRef.current?.querySelector<HTMLButtonElement>('[role^="menuitem"]:not(:disabled)');
      if (first && getComputedStyle(first).visibility === "visible") {
        first.focus();
        if (document.activeElement === first) { focusedForOpen.current = true; return; }
      }
      if (performance.now() < deadline) frame = requestAnimationFrame(focusVisibleItem);
    };
    frame = requestAnimationFrame(focusVisibleItem);
    return () => cancelAnimationFrame(frame);
  }, [box, open]);

  useEffect(() => {
    if (!open) return;
    const onPointer = (event: PointerEvent): void => {
      if (!(event.target instanceof Node)) return;
      if (menuRef.current?.contains(event.target) === true
          || (anchorRef === undefined && triggerRef.current?.contains(event.target) === true)) return;
      close(false);
    };
    const onKey = (event: KeyboardEvent): void => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      close(true);
    };
    window.addEventListener("pointerdown", onPointer);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("pointerdown", onPointer);
      window.removeEventListener("keydown", onKey);
    };
  }, [open, onOpenChange]);

  const onMenuKey = (event: ReactKeyboardEvent<HTMLDivElement>): void => {
    const items = [...event.currentTarget.querySelectorAll<HTMLButtonElement>(
      '[role^="menuitem"]:not(:disabled)',
    )];
    const current = items.indexOf(document.activeElement as HTMLButtonElement);
    let move: TerminalMenuMove | null = null;
    if (event.key === "ArrowDown") move = "next";
    else if (event.key === "ArrowUp") move = "previous";
    else if (event.key === "Home") move = "first";
    else if (event.key === "End") move = "last";
    else if (event.key.length === 1 && !event.metaKey && !event.ctrlKey && !event.altKey) {
      const next = terminalMenuTypeaheadIndex(items.map(item => item.textContent ?? ""),
        current, event.key);
      if (next >= 0) {
        event.preventDefault();
        items[next]?.focus();
      }
      return;
    }
    if (move !== null) {
      event.preventDefault();
      items[moveTerminalMenuIndex(items.length, current, move)]?.focus();
    }
  };

  return { triggerRef, menuRef, box, close, onMenuKey };
}
