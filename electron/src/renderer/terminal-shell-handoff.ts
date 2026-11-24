/** When an agent terminal that ended on its own gives its pane back to a shell.
 *
 * An agent CLI exiting cleanly leaves a pane holding nothing but a dead tab.
 * Rust decides the moment the exit lands, whatever this window shows (host
 * protocol v50): it starts a plain shell in the agent's folder, names the
 * agent in the shell's `replaces`, and dismisses the agent. This renderer
 * only reflects that: the shell takes the agent's exact tab slot in whichever
 * workspace lays the agent out, shown or parked, and the agent's saved entry
 * becomes the shell's. Everything here is pure; the component applies it.
 */

import type {
  TerminalLaunchProfileId, TerminalSummary, TerminalWorkspaceRecord,
} from "../shared/ipc";
import {
  parseTerminalWorkspaceState, replaceTerminalTab, serializeTerminalWorkspaceState,
  terminalPaneForTerminal, type TerminalWorkspaceState,
} from "./terminal-workspace";
import { forgetTerminal, rememberTerminal, workspaceForTerminal } from "./terminal-workspaces";

/** The catalog's plain-shell profile; every other id launches an agent CLI. */
export const SHELL_TERMINAL_PROFILE_ID: TerminalLaunchProfileId = "shell";

/** Where a shell Rust handed an exited agent's pane now sits. */
export interface HandedOffShellPlacement {
  /** The live layout, with the shell in the agent's tab when it was shown. */
  readonly layout: TerminalWorkspaceState;
  /** Saved workspaces: parked layouts moved to the shell, and the agent's
   * saved entry replaced by the shell's. */
  readonly records: readonly TerminalWorkspaceRecord[];
}

/**
 * Moves `shell` into the tab slot of the agent it `replaces`, wherever the
 * agent is laid out: the live layout of the shown workspace and any saved
 * layout (a parked workspace, or the shown one's saved copy). The workspace
 * that filed the agent files the shell in its place. Answers null when there
 * is nothing to move: no `replaces`, or no layout or record still names the
 * agent (already moved, or never placed).
 */
export function placeHandedOffShell(input: {
  readonly shell: TerminalSummary;
  readonly layout: TerminalWorkspaceState;
  readonly records: readonly TerminalWorkspaceRecord[];
  readonly now: () => string;
}): HandedOffShellPlacement | null {
  const { shell, now } = input;
  const agentId = shell.replaces;
  if (agentId === undefined || agentId === shell.id) return null;
  let changed = false;
  let layout = input.layout;
  if (terminalPaneForTerminal(layout, agentId) !== undefined) {
    layout = replaceTerminalTab(layout, agentId, shell.id);
    changed = true;
  }
  let records: readonly TerminalWorkspaceRecord[] = input.records.map(record => {
    const saved = parseTerminalWorkspaceState(record.layout ?? null);
    if (terminalPaneForTerminal(saved, agentId) === undefined) return record;
    changed = true;
    return {
      ...record,
      layout: serializeTerminalWorkspaceState(replaceTerminalTab(saved, agentId, shell.id)),
      updatedAt: now(),
    };
  });
  const owner = workspaceForTerminal(records, agentId);
  if (owner !== undefined) {
    records = forgetTerminal(rememberTerminal(records, owner.id, shell, undefined, now),
      agentId, now);
    changed = true;
  }
  return changed ? { layout, records } : null;
}

/**
 * What an ended agent pane says beside its "Start again" button, or null when
 * it offers none. These are the tabs Rust's shell hand-off leaves alone: a
 * crash, a signal, a launch that never started, an exit someone asked for,
 * or a clean exit whose shell could not be started. The host restarts the
 * same terminal under its restart proof, so the output stays readable until
 * the owner asks. A plain shell is not offered one: closing its tab is how it
 * ends.
 */
export function terminalRestartOffer(terminal: Pick<TerminalSummary, "state" | "exit">,
    profileId: string | undefined): string | null {
  if (profileId === undefined || profileId === "" || profileId === SHELL_TERMINAL_PROFILE_ID) return null;
  if (terminal.state === "failed") return "Did not start";
  if (terminal.state !== "exited") return null;
  const code = terminal.exit?.code;
  return code === undefined ? "Ended by a signal" : `Exited with code ${code}`;
}
