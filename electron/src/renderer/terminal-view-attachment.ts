/** A retained emulator moved into a new visible DOM slot. */
export const TERMINAL_VIEW_ATTACHMENT_EVENT = "interface-terminal-view-attachment";
/** A retained emulator was parked in the pool's hidden container. */
export const TERMINAL_VIEW_DETACHMENT_EVENT = "interface-terminal-view-detachment";
/** The pool's hidden container, outside every workspace and pane screen. */
export const TERMINAL_VIEW_PARKING_SELECTOR = "[data-terminal-parking]";
/**
 * A pane header's request to the emulator it shows (Find, Copy, Paste).
 * Dispatched on the view's `.terminal-xterm` host, as the pane layout event
 * is: the pooled view lives in the pane's DOM, not under it in React.
 */
export const TERMINAL_VIEW_COMMAND_EVENT = "interface-terminal-view-command";
export type TerminalViewCommand = "find" | "copy" | "paste";

/** The active tab's emulator host inside a pane, or null (a Browser tab has none). */
export function activeTerminalViewHost(pane: ParentNode | null): HTMLElement | null {
  return pane?.querySelector<HTMLElement>(".terminal-tabpanel.is-active .terminal-view .terminal-xterm") ?? null;
}

/** Ask the pane's shown emulator to run a header command; false when it has none. */
export function sendTerminalViewCommand(pane: ParentNode | null, command: TerminalViewCommand): boolean {
  const host = activeTerminalViewHost(pane);
  if (host === null) return false;
  host.dispatchEvent(new CustomEvent<TerminalViewCommand>(TERMINAL_VIEW_COMMAND_EVENT, { detail: command }));
  return true;
}

/** Whether the pane's shown emulator has text selected (Copy's enabled state). */
export function terminalViewHasSelection(pane: ParentNode | null): boolean {
  return activeTerminalViewHost(pane)?.closest(".terminal-view")?.hasAttribute("data-terminal-selection") === true;
}
