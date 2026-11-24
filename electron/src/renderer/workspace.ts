/** Maximum number of agent tiles the workspace can display at once. */
export const MAX_WORKSPACE_TILES = 4;

/**
 * The small, JSON-safe part of the agent workspace that survives a reload.
 *
 * Invariants maintained by every function in this module:
 *  - openKeys contains at most four unique, non-empty strings;
 *  - focusedKey is null exactly when there are no open keys;
 *  - maximizedKey is either null or the focused, open key.
 */
export interface WorkspaceState {
  readonly openKeys: readonly string[];
  readonly focusedKey: string | null;
  readonly maximizedKey: string | null;
}

const EMPTY_KEYS: readonly string[] = Object.freeze([]);

export const EMPTY_WORKSPACE_STATE: WorkspaceState = Object.freeze({
  openKeys: EMPTY_KEYS,
  focusedKey: null,
  maximizedKey: null,
});

type UnknownRecord = Record<string, unknown>;

function record(value: unknown): UnknownRecord | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? value as UnknownRecord
    : null;
}

function usableKey(value: unknown): value is string {
  // Do not trim an identity. A session key containing whitespace is unusual but
  // still distinct, and changing it here could make persisted state point at a
  // different session.
  return typeof value === "string" && value.length > 0;
}

function uniqueKeys(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  const keys: string[] = [];
  const seen = new Set<string>();
  for (const candidate of value) {
    if (!usableKey(candidate) || seen.has(candidate)) continue;
    seen.add(candidate);
    keys.push(candidate);
    if (keys.length === MAX_WORKSPACE_TILES) break;
  }
  return keys;
}

/**
 * Turn an untrusted value into one valid state. This is deliberately used at
 * every public boundary, not only JSON parsing, so a corrupt caller cannot make
 * later operations retain duplicate or dangling keys.
 */
function normalize(value: unknown): WorkspaceState {
  const source = record(value);
  if (source === null) return EMPTY_WORKSPACE_STATE;

  const openKeys = uniqueKeys(source.openKeys);
  if (openKeys.length === 0) return EMPTY_WORKSPACE_STATE;

  const storedFocus = usableKey(source.focusedKey) && openKeys.includes(source.focusedKey)
    ? source.focusedKey
    : openKeys[0];
  const maximizedKey = usableKey(source.maximizedKey)
      && openKeys.includes(source.maximizedKey)
    ? source.maximizedKey
    : null;

  // A maximized tile is necessarily the one receiving keyboard focus. Keeping
  // those fields aligned prevents a hidden tile from becoming the focus target.
  return {
    openKeys,
    focusedKey: maximizedKey ?? storedFocus,
    maximizedKey,
  };
}

/** Parse persisted JSON. Missing, malformed, or wrong-shaped data becomes empty. */
export function parseWorkspaceState(raw: string | null | undefined): WorkspaceState {
  if (typeof raw !== "string" || raw.length === 0) return EMPTY_WORKSPACE_STATE;
  try {
    return normalize(JSON.parse(raw) as unknown);
  } catch {
    return EMPTY_WORKSPACE_STATE;
  }
}

/**
 * Drop tiles for sessions that no longer exist. The known-key iterable is also
 * treated as untrusted at runtime: empty and non-string entries grant nothing.
 */
export function reconcileWorkspaceState(state: WorkspaceState,
    knownKeys: Iterable<string>): WorkspaceState {
  const current = normalize(state);
  const known = new Set<string>();
  for (const key of knownKeys as Iterable<unknown>) {
    if (usableKey(key)) known.add(key);
  }

  const openKeys = current.openKeys.filter((key) => known.has(key));
  if (openKeys.length === 0) return EMPTY_WORKSPACE_STATE;

  const maximizedKey = current.maximizedKey !== null && known.has(current.maximizedKey)
    ? current.maximizedKey
    : null;
  const focusedKey = maximizedKey
    ?? (current.focusedKey !== null && known.has(current.focusedKey)
      ? current.focusedKey
      : openKeys[0]);
  const next = { openKeys, focusedKey, maximizedKey };
  // Nothing dropped: hand the caller its own object so its setState bails
  // out instead of re-rendering and re-persisting an unchanged layout on
  // every registry push.
  return sameWorkspaceState(next, state) ? state : next;
}

/** Field-wise equality, total over the corrupt input normalize() tolerates. */
function sameWorkspaceState(a: WorkspaceState, b: WorkspaceState): boolean {
  return Array.isArray(b.openKeys) && a.openKeys.length === b.openKeys.length
    && a.openKeys.every((key, index) => key === b.openKeys[index])
    && a.focusedKey === b.focusedKey && a.maximizedKey === b.maximizedKey;
}

/**
 * `next`, or the caller's own `state` when a transition changed nothing, so a
 * click or focus inside the focused pane lets setState bail out instead of
 * re-rendering and re-persisting the same layout. A value normalize() had to
 * repair is never equal, and is replaced.
 *
 * closeWorkspaceKey and cycleWorkspaceFocus deliberately do not use it: App
 * arms a pending tile focus from their result, and the fresh state is what
 * renders and runs the effect that spends it. Kept, the focus would wait for
 * the next registry push and then jump.
 */
function keep(state: WorkspaceState, next: WorkspaceState): WorkspaceState {
  return record(state) !== null && sameWorkspaceState(next, state) ? state : next;
}

/**
 * Focus an existing pane without reordering it, or explicitly append a new
 * one. At the four-pane ceiling a fifth split is refused; ordinary navigation
 * uses switchWorkspaceKey and therefore never reaches that ceiling.
 */
export function openWorkspaceKey(state: WorkspaceState, key: string): WorkspaceState {
  const current = normalize(state);
  if (!usableKey(key)) return keep(state, current);

  if (current.openKeys.includes(key)) {
    return keep(state, {
      ...current,
      focusedKey: key,
      // Selecting another session while maximized keeps the workspace in
      // maximized mode, but moves that mode to the selected session.
      maximizedKey: current.maximizedKey === null ? null : key,
    });
  }

  if (current.openKeys.length === MAX_WORKSPACE_TILES) return keep(state, current);
  return {
    openKeys: [...current.openKeys, key],
    focusedKey: key,
    // An explicit request for another pane should reveal the grid. Keeping the
    // previous maximized mode would mount the pane while leaving it invisible.
    maximizedKey: null,
  };
}

/** Whether a known number of new panes can be appended without partial placement. */
export function hasWorkspacePaneCapacity(state: WorkspaceState, additional: number): boolean {
  if (!Number.isInteger(additional) || additional < 0) return false;
  return normalize(state).openKeys.length + additional <= MAX_WORKSPACE_TILES;
}

/**
 * Gate an explicit pane-open request while a multi-pane launch owns a bounded
 * reservation. Existing panes may still be focused; no new pane may race the
 * reserved append before React commits its state update.
 */
export function mayOpenWorkspaceKeyWithReservation(
    state: WorkspaceState, key: string, reserved: number): boolean {
  const current = normalize(state);
  if (!usableKey(key) || !Number.isInteger(reserved) || reserved < 0) return false;
  if (current.openKeys.includes(key)) return true;
  return reserved === 0 && current.openKeys.length < MAX_WORKSPACE_TILES;
}

/**
 * Switch the focused pane to a session without creating another pane.
 *
 * Selecting a session that is already open only moves focus. Otherwise the
 * session replaces the currently focused slot in place, preserving the order
 * and number of panes. This makes ordinary navigation safe at the four-pane
 * ceiling while leaving openWorkspaceKey as the explicit add-pane operation.
 */
export function switchWorkspaceKey(state: WorkspaceState, key: string): WorkspaceState {
  const current = normalize(state);
  if (!usableKey(key)) return keep(state, current);

  if (current.openKeys.length === 0) {
    return { openKeys: [key], focusedKey: key, maximizedKey: null };
  }

  if (current.openKeys.includes(key)) {
    return keep(state, {
      ...current,
      focusedKey: key,
      maximizedKey: current.maximizedKey === null ? null : key,
    });
  }

  const focusedIndex = current.openKeys.indexOf(current.focusedKey!);
  const openKeys = [...current.openKeys];
  openKeys[focusedIndex] = key;
  return {
    openKeys,
    focusedKey: key,
    maximizedKey: current.maximizedKey === null ? null : key,
  };
}

/**
 * A thread whose provider was switched (protocol 59) runs on `to` now: the
 * pane that showed `from` shows `to` in the same place, focused (maximized
 * still, when it was). Without a pane on `from`, `to` is switched to as any
 * selection is.
 */
export function replaceWorkspaceKey(state: WorkspaceState, from: string, to: string): WorkspaceState {
  const current = normalize(state);
  if (!usableKey(to)) return keep(state, current);
  const at = current.openKeys.indexOf(from);
  if (at < 0 || current.openKeys.includes(to)) return switchWorkspaceKey(state, to);
  const openKeys = [...current.openKeys];
  openKeys[at] = to;
  return {
    openKeys,
    focusedKey: to,
    maximizedKey: current.maximizedKey === from ? to : current.maximizedKey === null ? null : to,
  };
}

/**
 * Close a tile. Closing the focus chooses its right-hand neighbour, falling
 * back to the tile immediately to its left when it was last in the order.
 */
export function closeWorkspaceKey(state: WorkspaceState, key: string): WorkspaceState {
  const current = normalize(state);
  const index = current.openKeys.indexOf(key);
  if (index < 0) return current;

  const openKeys = current.openKeys.filter((candidate) => candidate !== key);
  if (openKeys.length === 0) return EMPTY_WORKSPACE_STATE;

  const focusedKey = current.focusedKey === key
    ? openKeys[index] ?? openKeys[index - 1]
    : current.focusedKey;
  return {
    openKeys,
    focusedKey,
    // Closing the maximized tile returns to the grid; maximizing a replacement
    // would be an additional user decision.
    maximizedKey: current.maximizedKey === key ? null : current.maximizedKey,
  };
}

export type WorkspaceCycleDirection = "next" | "previous";

/** Move focus one tile with wraparound, without changing tile order. */
export function cycleWorkspaceFocus(state: WorkspaceState,
    direction: WorkspaceCycleDirection): WorkspaceState {
  const current = normalize(state);
  const count = current.openKeys.length;
  if (count < 2) return current;

  const at = current.openKeys.indexOf(current.focusedKey!);
  const offset = direction === "previous" ? -1 : 1;
  const focusedKey = current.openKeys[(at + offset + count) % count];
  return {
    ...current,
    focusedKey,
    maximizedKey: current.maximizedKey === null ? null : focusedKey,
  };
}

export interface WorkspaceShortcutEvent {
  key: string;
  metaKey: boolean;
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
}

export interface WorkspaceOverlayState {
  composerOpen: boolean;
  settingsOpen: boolean;
  foregroundDialog: boolean;
}

export type WorkspaceOverlayShortcut = "palette" | "prevent-browser" | "pass";

/**
 * New Chat owns ordinary workspace chords so they cannot stack a second modal
 * behind the launch card. The command palette stays reachable because the
 * first-run surface IS that card.
 */
export function workspaceOverlayShortcut(event: WorkspaceShortcutEvent,
    overlay: WorkspaceOverlayState): WorkspaceOverlayShortcut {
  const modifier = event.metaKey || event.ctrlKey;
  const key = event.key.toLowerCase();
  const paletteChord = modifier && event.shiftKey && !event.altKey && key === "p";
  if (overlay.foregroundDialog || overlay.settingsOpen) {
    return paletteChord || (modifier && (key === "p" || key === "n" || key === "f"))
      ? "prevent-browser" : "pass";
  }
  if (paletteChord) return "palette";
  if (overlay.composerOpen && modifier && (key === "p" || key === "n" || key === "f")) {
    return "prevent-browser";
  }
  return "pass";
}

/** What ⌘W closes: the thing in front of the reader, never the window. */
export type CloseChordTarget = "settings" | "terminal-pane" | "tile" | "none";

export interface CloseChordState extends WorkspaceOverlayState {
  /** Code is showing its terminal panes. */
  terminalsShown: boolean;
  /** Threads or Agent is showing its tiles. */
  sessionsShown: boolean;
  hasFocusedTile: boolean;
}

/**
 * ⌘W closes the selected pane (owner, 2026-11-15); it used to close the
 * window, which now takes ⇧⌘W. A dialog in front keeps its own keys, so the
 * chord closes nothing behind it; Settings covers the panes and is what
 * closes; the new-thread page is left to Escape.
 */
export function closeChordTarget(state: CloseChordState): CloseChordTarget {
  if (state.foregroundDialog) return "none";
  if (state.settingsOpen) return "settings";
  if (state.composerOpen) return "none";
  if (state.terminalsShown) return "terminal-pane";
  return state.sessionsShown && state.hasFocusedTile ? "tile" : "none";
}

/** Maximize an open tile, or restore the grid when that tile is already maximized. */
export function toggleWorkspaceMaximized(state: WorkspaceState, key: string): WorkspaceState {
  const current = normalize(state);
  if (!current.openKeys.includes(key)) return keep(state, current);
  return {
    ...current,
    focusedKey: key,
    maximizedKey: current.maximizedKey === key ? null : key,
  };
}
