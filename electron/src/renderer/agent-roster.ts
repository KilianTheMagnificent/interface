import type { AgentControlResponse } from "../shared/agents";
import { sessionKey, sessionKindOf, type SessionSummary } from "../shared/engine";

/** Display projection only: a task preview never changes the saved agent order. */
export function agentLatestTasks(assignments: Readonly<Record<string, string>>,
    sessions: readonly SessionSummary[]): Readonly<Record<string, SessionSummary>> {
  const result: Record<string, SessionSummary> = {};
  for (const session of sessions) {
    if (sessionKindOf(session) !== "agent") continue;
    const agentId = assignments[sessionKey(session.engine, session.sessionId)];
    if (agentId === undefined) continue;
    const previous = result[agentId];
    if (previous === undefined || session.lastActivity > previous.lastActivity) result[agentId] = session;
  }
  return result;
}

type AgentSessionFacts = Pick<AgentControlResponse, "threads" | "runs">;

/** True for a session Interface's agent approval policy answers first: any
 *  agent's current thread, or a routine run's session. */
export function agentBoundSession(state: AgentSessionFacts, key: string): boolean {
  return Object.values(state.threads ?? {}).some(thread => thread.current?.sessionKey === key)
    || state.runs.some(run => run.sessionKey === key);
}

/**
 * Who may answer this approval card. `null`: not an agent session, so today's
 * card. `"owner"`: the Rust policy asked the owner. `"checking"`: the policy
 * has not decided, so the card offers no verdict — the host refuses one anyway.
 */
export function agentApprovalGate(state: AgentSessionFacts & Pick<AgentControlResponse, "pendingApprovals">,
    key: string, approvalId: string): "owner" | "checking" | null {
  if (!agentBoundSession(state, key)) return null;
  return state.pendingApprovals.some(row => row.sessionKey === key && row.approvalId === approvalId)
    ? "owner" : "checking";
}

/** The agent whose thread, current or past, is this session. */
export function agentThreadOwner(state: Pick<AgentControlResponse, "threads">, key: string): string | undefined {
  for (const [agentId, thread] of Object.entries(state.threads ?? {})) {
    if (thread.current?.sessionKey === key || (thread.past ?? []).some(row => row.sessionKey === key)) return agentId;
  }
  return undefined;
}

/**
 * A past thread is read-only: no composer, no resume, no verdicts. `null` for
 * any other session, including an agent's current thread.
 */
export function agentPastThread(state: Pick<AgentControlResponse, "threads">, key: string):
    { agentId: string; currentKey: string | null } | null {
  for (const [agentId, thread] of Object.entries(state.threads ?? {})) {
    if (thread.current?.sessionKey === key) return null;
    if ((thread.past ?? []).some(row => row.sessionKey === key)) {
      return { agentId, currentKey: thread.current?.sessionKey ?? null };
    }
  }
  return null;
}

export type AgentApprovalGate = (approvalId: string) => "owner" | "checking" | null;
const STILL_CHECKING: AgentApprovalGate = () => "checking";

/**
 * The gate a session's conversation gets, or undefined for today's cards.
 * `startKnownKeys` is the session list as it stood when a `startAgentThread`
 * still in flight began: a session outside it may be that new thread, whose
 * key the renderer cannot know yet, so its requests wait for the policy too.
 */
export function agentApprovalGateFor(state: AgentSessionFacts & Pick<AgentControlResponse, "pendingApprovals">,
    key: string, startKnownKeys: ReadonlySet<string> | null): AgentApprovalGate | undefined {
  if (agentBoundSession(state, key)) return approvalId => agentApprovalGate(state, key, approvalId);
  return startKnownKeys !== null && !startKnownKeys.has(key) ? STILL_CHECKING : undefined;
}
