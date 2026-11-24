/**
 * Terminals that run with no pane anywhere, and when to end them.
 *
 * Closing a tab ends its terminal (owner, 2026-10-01), so a terminal still
 * running unseen got past that: a create that landed during a renderer
 * reload, a roster save the host refused, a pane parked by an older build.
 * Nothing on the Mac lists such a terminal, so once it has sat hidden and
 * quiet for the grace it is ended. One that is working or waiting is left
 * alone until it is not. Everything here is pure; the component ends them.
 */

import type { TerminalWorkspaceRecord } from "../shared/ipc";
import { savedLayoutFacts } from "./terminal-workspace";

/**
 * How long a terminal sits hidden and quiet before it is ended. Far longer
 * than a create, a reopen or a roster save takes to place its tab.
 */
export const HIDDEN_TERMINAL_GRACE_MS = 30_000;

/**
 * Every terminal a pane shows or will show again: the open workspace's live
 * layout and each saved one. A record saved before layouts existed counts
 * every terminal it files.
 */
export function attachedTerminalIds(records: readonly TerminalWorkspaceRecord[],
    openLayoutIds: ReadonlySet<string>): Set<string> {
  const attached = new Set(openLayoutIds);
  for (const record of records) {
    const ids = record.layout === undefined
      ? record.terminals.map(entry => entry.id)
      : savedLayoutFacts(record.layout).terminalIds;
    for (const id of ids) attached.add(id);
  }
  return attached;
}

export interface HiddenTerminal {
  /** One process: a restart under the same id starts a new clock. */
  readonly key: string;
  /** Starting, working or waiting: never ended while so. */
  readonly busy: boolean;
}

export interface HiddenTerminalSweep {
  /** Hidden and quiet for the whole grace: end these. */
  readonly due: readonly string[];
  /** When each quiet hidden terminal was first seen so, to pass back next time. */
  readonly since: ReadonlyMap<string, number>;
  /** Milliseconds until the next clock runs out, or null when none is running. */
  readonly nextCheckMs: number | null;
}

/**
 * Which hidden terminals have sat quiet for the whole grace. A busy one has
 * no clock, so its grace starts when it goes quiet; one no longer hidden is
 * forgotten.
 */
export function sweepHiddenTerminals(hidden: readonly HiddenTerminal[],
    since: ReadonlyMap<string, number>, now: number,
    graceMs = HIDDEN_TERMINAL_GRACE_MS): HiddenTerminalSweep {
  const next = new Map<string, number>();
  const due: string[] = [];
  let nextCheckMs: number | null = null;
  for (const terminal of hidden) {
    if (terminal.busy) continue;
    const started = since.get(terminal.key) ?? now;
    const left = started + graceMs - now;
    if (left <= 0) {
      due.push(terminal.key);
      continue;
    }
    next.set(terminal.key, started);
    nextCheckMs = nextCheckMs === null ? left : Math.min(nextCheckMs, left);
  }
  return { due, since: next, nextCheckMs };
}
