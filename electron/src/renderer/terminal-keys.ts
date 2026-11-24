export type TerminalCenterKeyAction =
  | "previous-terminal"
  | "next-terminal"
  | "split-horizontal"
  | "split-vertical"
  | "new-pane";

export interface TerminalCenterKeyEvent {
  readonly metaKey: boolean;
  readonly ctrlKey: boolean;
  readonly shiftKey: boolean;
  readonly altKey: boolean;
  readonly key: string;
  readonly code: string;
  readonly repeat?: boolean;
  readonly isComposing?: boolean;
  readonly keyCode?: number;
}

/** Ctrl+U is the portable terminal spelling of "delete to the start of the line". */
export const TERMINAL_DELETE_TO_LINE_START = "\u0015";

/**
 * Extra macOS editing keys that xterm does not translate itself.
 *
 * xterm maps Backspace to DEL regardless of Meta, which makes ⌘Backspace no
 * different from ordinary Backspace, and it emits nothing at all for a ⌘arrow,
 * so ⌘← / ⌘→ sat dead in a pane. They become readline's Ctrl+A / Ctrl+E, the
 * way VS Code's terminal spells them. Keep every other modifier combination in
 * xterm's hands so shells and full-screen programs retain their own bindings.
 */
const META_KEY_INPUT: ReadonlyMap<string, string> = new Map([
  ["Backspace", TERMINAL_DELETE_TO_LINE_START],
  ["ArrowLeft", "\u0001"],
  ["ArrowRight", "\u0005"],
]);

export function terminalInputForKey(event: TerminalCenterKeyEvent): string | null {
  if (!event.metaKey || event.ctrlKey || event.altKey || event.shiftKey
      || event.isComposing === true || event.keyCode === 229) return null;
  return META_KEY_INPUT.get(event.key) ?? null;
}

/** Shift+Tab as the terminal's reverse-tab report, CSI Z. */
export const TERMINAL_REVERSE_TAB = "\x1b[Z";

/**
 * Whether plain Shift+Tab belongs to the program in the pane.
 *
 * Provider TUIs read Shift+Tab as their permission-mode cycle, and xterm
 * does send it CSI Z — but in screen-reader mode, which every visible pane
 * runs, xterm leaves the event uncancelled, so the browser also moved
 * focus backwards into another pane. The pane claims the chord and sends
 * the report itself instead. Ctrl/Meta/Alt spellings stay with xterm and
 * the app: Ctrl+Shift+Tab is the previous-pane chord outside a terminal,
 * and xterm already cancels that spelling inside one.
 */
export function terminalReverseTabGoesToProgram(
  event: TerminalCenterKeyEvent,
): boolean {
  return event.key === "Tab" && event.shiftKey
    && !event.ctrlKey && !event.metaKey && !event.altKey
    && event.isComposing !== true && event.keyCode !== 229;
}

/** ⌘C as a kitty keyboard report: Super+C, which Codex reads as copy. */
export const TERMINAL_PROGRAM_COPY_KEY = "\x1b[99;9u";

export interface TerminalCopyContext {
  /** xterm's own selection, which ⌘C always copies. */
  readonly hasSelection: boolean;
  /** The program receives pointer drags, so any highlight on screen is its own. */
  readonly mouseTracking: boolean;
  /** The program asked for kitty key reports, so it can read Super+C. */
  readonly readsKeyReports: boolean;
}

/**
 * Whether ⌘C belongs to the program in the pane rather than to xterm.
 *
 * Codex takes the pointer and draws its own selection, so a drag selects
 * nothing in xterm and ⌘C copied nothing. Codex copies its selection on
 * Super+C, which only a kitty key report can express; a program that never
 * asked for those reports could misread one as keystrokes, so it keeps the
 * old behavior.
 */
export function terminalCopyGoesToProgram(
  event: TerminalCenterKeyEvent, pane: TerminalCopyContext,
): boolean {
  return event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey
    && event.isComposing !== true && event.key.toLowerCase() === "c"
    && !pane.hasSelection && pane.mouseTracking && pane.readsKeyReports;
}

/**
 * Whether a key in a focused pane opens Interface's terminal search box.
 *
 * Strict ⌘F on macOS and Ctrl+Shift+F elsewhere. Ctrl+F belongs to the program,
 * as every Control-only chord in a pane does (`shellOwnsChord`): cursor forward
 * in a shell, page down in vim and less.
 */
export function terminalOpensSearch(event: TerminalCenterKeyEvent, mac = true): boolean {
  if (event.isComposing === true || event.keyCode === 229 || event.altKey) return false;
  if (event.key.toLowerCase() !== "f") return false;
  return mac
    ? event.metaKey && !event.ctrlKey && !event.shiftKey
    : event.ctrlKey && event.shiftKey && !event.metaKey;
}

/**
 * Capture-phase chords owned by the terminal center.
 *
 * ⌘D / ⌘⇧D match this module's split orientations: horizontal is left/right,
 * vertical is top/bottom. Control+D is left alone so a PTY still receives EOF.
 * ⌘↩ opens a pane too (owner, 2026-11-15), on whichever side the focused
 * pane has more room, as its `+` menu's Terminal row does. Plain Enter and
 * every other modified Enter stay the program's.
 */
export function terminalCenterKeyAction(
  event: TerminalCenterKeyEvent,
): TerminalCenterKeyAction | null {
  if (event.altKey) return null;
  if (event.shiftKey && (event.metaKey || event.ctrlKey)
      && (event.code === "BracketLeft" || event.code === "BracketRight")) {
    return event.code === "BracketLeft" ? "previous-terminal" : "next-terminal";
  }
  if (event.key === "Enter") {
    // A held chord auto-repeats, and each repeat would mint another shell.
    return event.metaKey && !event.ctrlKey && !event.shiftKey && event.repeat !== true
      && event.isComposing !== true && event.keyCode !== 229 ? "new-pane" : null;
  }
  if (!event.metaKey || event.ctrlKey || event.key.toLowerCase() !== "d") return null;
  // A held ⌘D auto-repeats. Each repeat would mint another untitled shell.
  if (event.repeat === true) return null;
  return event.shiftKey ? "split-vertical" : "split-horizontal";
}

/**
 * Chords a focused shell keeps from the window handler.
 *
 * Ctrl+P / Ctrl+N are readline's history keys, Ctrl+K is kill-to-end-of-line,
 * and Ctrl+Tab is the shell's own; the app claimed all four for navigation,
 * which swallowed them in a pane. Only the Control-only spelling yields:
 * ⌘P / ⌘N / ⌘K / ⌘⇧P (and ⌃⇧P) stay the app's everywhere, and Ctrl+` keeps
 * opening the terminals.
 *
 * The Sidebar's own ⌘K listener asks this too, so which Ctrl+K belongs to the
 * shell is decided once rather than once per window listener.
 */
export function shellOwnsChord(
  event: TerminalCenterKeyEvent, targetInsideTerminal: boolean,
): boolean {
  if (!targetInsideTerminal || !event.ctrlKey || event.metaKey || event.altKey) return false;
  if (event.key === "Tab") return true;
  if (event.shiftKey) return false;
  const key = event.key.toLowerCase();
  return key === "p" || key === "n" || key === "k";
}

/**
 * Whether ⌘↩ pressed here may open a pane. The launcher launches on the same
 * chord, and a field of the app's own (a page's address, Find, a rename)
 * keeps its Enter; xterm's hidden input is the pane itself.
 */
export function newPaneChordAllowedAt(target: {
  readonly insideLauncher: boolean;
  /** The nearest input, textarea, select or editable element, if any. */
  readonly field: "none" | "terminal-input" | "other";
}): boolean {
  return !target.insideLauncher && target.field !== "other";
}

/**
 * ⌘W closes the selected pane (owner, 2026-11-15); it used to close the
 * window. Strictly ⌘: Control+W deletes a word in every shell, and ⇧⌘W is
 * the menu's Close Window.
 */
export function isClosePaneChord(event: TerminalCenterKeyEvent): boolean {
  return event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey
    && event.isComposing !== true && event.keyCode !== 229
    && event.key.toLowerCase() === "w";
}
