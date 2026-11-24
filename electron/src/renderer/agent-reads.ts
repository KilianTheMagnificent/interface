import type { AgentRequest } from "../shared/agents";
import type { AgentWorkView } from "../shared/agent-work";
import type { AgentRequestHandler } from "./components/AgentWorkspacePanel";
import { startVisiblePolling, type VisiblePolling } from "./visible-polling";

/**
 * Requests that only read the agent store or record that the owner saw a
 * thread. They keep their place in the agent request queue, behind any change
 * already asked for, but never mark the agent page busy: a background read
 * must not disable the controls under the owner's pointer. Anything not
 * listed here, including a kind added later, still marks it busy.
 */
export function agentRequestIsRead(command: AgentRequest): boolean {
  switch (command.kind) {
    case "work": return command.command.action === "read";
    case "calendar": return command.command.action === "read" || command.command.action === "preview";
    case "markRead":
    case "readAgentFiles":
    case "readAgentDecisions":
      return true;
    default: return false;
  }
}

export const AGENT_WORK_POLL_MS = 5000;

/**
 * The view already on screen when the answer carries the same revision. Rust
 * builds the view from the work store alone and bumps its revision on every
 * commit, so an equal revision is an equal view and needs no redraw.
 */
export function keepAgentWork(current: AgentWorkView | null, next: AgentWorkView): AgentWorkView {
  return current !== null && current.revision === next.revision ? current : next;
}

type Visibility = Pick<Document, "hidden" | "addEventListener" | "removeEventListener">;

/**
 * Reads one agent's task, memory and settings view while its page is on
 * screen: at once, then every AGENT_WORK_POLL_MS after the last answer, and
 * again whenever the window returns. Nothing is read while `visible` is false,
 * so becoming visible restarts it with a fresh read.
 */
export function startAgentWorkReads({ visible, agentId, onRequest, onWork, onError }: {
  visible: boolean; agentId: string; onRequest: AgentRequestHandler;
  onWork: (work: AgentWorkView) => void; onError: (error: string) => void;
}, visibility: Visibility = document): () => void {
  if (!visible) return () => undefined;
  let stopped = false;
  const polling: VisiblePolling = startVisiblePolling(async () => {
    const result = await onRequest({ kind: "work", agentId, command: { action: "read" } });
    if (stopped) return;
    if (result.ok && result.response.work !== undefined) onWork(result.response.work);
    else if (!result.ok) onError(result.error);
  }, AGENT_WORK_POLL_MS, visibility);
  return () => { stopped = true; polling(); };
}
