import { describe, expect, it } from "vitest";
import { sessionKey, type SessionSummary } from "../shared/engine";
import { emptyAgentControlResponse, type AgentControlResponse } from "../shared/agents";
import {
  agentApprovalGate, agentApprovalGateFor, agentBoundSession, agentLatestTasks, agentPastThread, agentThreadOwner,
} from "./agent-roster";
import { AGENT_CHARACTERS } from "./agent-appearance";
import { AGENT_COLORS } from "./agent-profiles";
import fixtureV23 from "../fixtures/agent-workspace-v23.json";

describe("agent roster display", () => {
  it("takes each agent's latest assigned task without borrowing code or another agent's task", () => {
    const task: SessionSummary = { engine: "codex", sessionId: "old", title: "Earlier work", cwd: "/fixture",
      status: "idle", cloud: false, archived: false, firstSeen: "2026-09-07T08:00:00Z",
      lastActivity: "2026-09-07T08:00:00Z", kind: "agent" };
    const newer = { ...task, sessionId: "new", title: "Newer work", lastActivity: "2026-09-07T10:00:00Z" };
    const code = { ...newer, sessionId: "code", kind: "code" as const, lastActivity: "2026-09-07T12:00:00Z" };
    const other = { ...newer, sessionId: "other", title: "Other agent" };
    const assignments = Object.fromEntries([task, newer, code].map(row => [sessionKey(row.engine, row.sessionId), "a"]));
    assignments[sessionKey(other.engine, other.sessionId)] = "b";
    expect(agentLatestTasks(assignments, [newer, code, other, task])).toEqual({ a: newer, b: other });
    expect(agentLatestTasks({}, [newer])).toEqual({});
  });

  it("gates an agent session's approval on the host's pending list, and leaves other sessions alone", () => {
    const state: AgentControlResponse = {
      ...emptyAgentControlResponse(),
      threads: { a: { current: { sessionKey: "claude#thread", startedAtMs: 1, status: "waiting", interrupted: false },
        past: [{ sessionKey: "claude#past", startedAtMs: 0, endedAtMs: 1 }], lastReadAtMs: 0, lastActivityAtMs: 1 } },
      runs: [{ id: "r", routineId: "x", agentId: "a", name: "Run", scheduledAtMs: 0, updatedAtMs: 0,
        status: "waiting", sessionKey: "codex#run", detail: "", prompt: "", cwd: "/f", began: true,
        profile: { id: "a", name: "A", engine: "codex", model: "", effort: "", instructions: "", color: "teal" } }],
      pendingApprovals: [{ agentId: "a", sessionKey: "claude#thread", approvalId: "ask-1", tool: "Bash",
        summary: "Run tests", reason: "", askedAtMs: 1 }],
    };

    expect(agentApprovalGate(state, "claude#thread", "ask-1")).toBe("owner");
    expect(agentApprovalGate(state, "claude#thread", "ask-2")).toBe("checking");
    expect(agentApprovalGate(state, "codex#run", "ask-1")).toBe("checking");
    expect(agentApprovalGate(state, "claude#plain", "ask-1")).toBeNull();
    // A past thread is no longer the agent's live session.
    expect(agentApprovalGate(state, "claude#past", "ask-1")).toBeNull();
    expect(agentBoundSession(state, "codex#run")).toBe(true);
    expect(agentBoundSession(state, "claude#plain")).toBe(false);
  });

  it("names a past thread read-only with the way back, and owns thread sessions by agent", () => {
    const state: Pick<AgentControlResponse, "threads"> = { threads: {
      a: { current: { sessionKey: "claude#now", startedAtMs: 2, status: "idle", interrupted: false },
        past: [{ sessionKey: "claude#then", startedAtMs: 0, endedAtMs: 1 }], lastReadAtMs: 0, lastActivityAtMs: 2 },
      b: { past: [{ sessionKey: "codex#old", startedAtMs: 0, endedAtMs: 1 }], lastReadAtMs: 0, lastActivityAtMs: 1 },
    } };

    expect(agentPastThread(state, "claude#then")).toEqual({ agentId: "a", currentKey: "claude#now" });
    expect(agentPastThread(state, "codex#old")).toEqual({ agentId: "b", currentKey: null });
    expect(agentPastThread(state, "claude#now")).toBeNull();
    expect(agentPastThread(state, "claude#plain")).toBeNull();
    expect(agentThreadOwner(state, "claude#now")).toBe("a");
    expect(agentThreadOwner(state, "codex#old")).toBe("b");
    expect(agentThreadOwner(state, "claude#plain")).toBeUndefined();
  });

  it("reads an agent whose first thread has no past list, as Rust sends it", () => {
    // Rust omits an empty `past` (skip_serializing_if), so a first thread arrives without it.
    const state: Pick<AgentControlResponse, "threads"> = { threads: {
      a: { current: { sessionKey: "claude#first", startedAtMs: 1, status: "working", interrupted: false },
        lastReadAtMs: 0, lastActivityAtMs: 1 },
    } };

    expect(agentPastThread(state, "claude#first")).toBeNull();
    expect(agentPastThread(state, "other#key")).toBeNull();
    expect(agentThreadOwner(state, "claude#first")).toBe("a");
    expect(agentThreadOwner(state, "other#key")).toBeUndefined();
    const response = fixtureV23.response as AgentControlResponse;
    expect(response.threads?.writer?.past).toBeUndefined();
    expect(agentPastThread(response, "claude#plain")).toBeNull();
    expect(agentThreadOwner(response, "claude#33333333-3333-4333-8333-333333333333")).toBe("writer");
  });

  it("holds approvals on a session nobody knew while a thread is starting", () => {
    const state: AgentControlResponse = { ...emptyAgentControlResponse(),
      threads: { a: { current: { sessionKey: "claude#thread", startedAtMs: 1, status: "working", interrupted: false },
        past: [], lastReadAtMs: 0, lastActivityAtMs: 1 } },
      pendingApprovals: [{ agentId: "a", sessionKey: "claude#thread", approvalId: "ask", tool: "Bash",
        summary: "", reason: "", askedAtMs: 1 }] };
    const known = new Set(["claude#thread", "codex#plain"]);

    // No start in flight: an ordinary session keeps today's card.
    expect(agentApprovalGateFor(state, "claude#new", null)).toBeUndefined();
    // A start in flight: the session that appeared since may be its thread.
    expect(agentApprovalGateFor(state, "claude#new", known)?.("any")).toBe("checking");
    // A session that was already there before the start is not the new thread.
    expect(agentApprovalGateFor(state, "codex#plain", known)).toBeUndefined();
    // A bound session answers from the pending list either way.
    expect(agentApprovalGateFor(state, "claude#thread", known)?.("ask")).toBe("owner");
    expect(agentApprovalGateFor(state, "claude#thread", null)?.("other")).toBe("checking");
  });

  it("gives every existing saved colour a distinct character silhouette and fill", () => {
    const characters = AGENT_COLORS.map(color => AGENT_CHARACTERS[color]);
    expect(new Set(characters.map(character => character.path)).size).toBe(AGENT_COLORS.length);
    expect(new Set(characters.map(character => character.color)).size).toBe(AGENT_COLORS.length);
  });
});
