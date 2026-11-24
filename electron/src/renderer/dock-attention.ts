import { sessionKey, type SessionSummary } from "../shared/engine";
import type { DockAttentionV1 } from "../shared/dock-attention";
import type { TerminalSummary } from "../shared/ipc";

/**
 * The renderer's Dock badge report: live terminal panes waiting for input —
 * the same panes the attention tray lists as needing you. A pane whose session
 * the registry already lists as waiting is that session, which main counts
 * already, so it is left out.
 */
export function dockAttention(
  terminals: readonly Pick<TerminalSummary, "state" | "activity" | "sessionKey">[],
  sessions: readonly Pick<SessionSummary, "engine" | "sessionId" | "status">[],
): DockAttentionV1 {
  const waitingSessions = new Set(sessions.filter(session => session.status === "waiting")
    .map(session => sessionKey(session.engine, session.sessionId)));
  let waitingPanes = 0;
  for (const terminal of terminals) {
    if (terminal.state !== "running" && terminal.state !== "starting") continue;
    if (terminal.activity !== "waiting") continue;
    if (terminal.sessionKey !== undefined && waitingSessions.has(terminal.sessionKey)) continue;
    waitingPanes++;
  }
  return { version: 1, waitingPanes };
}
