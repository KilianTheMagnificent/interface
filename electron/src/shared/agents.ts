import type { AgentCalendarCommand, AgentCalendarView, AgentWorkCommand, AgentWorkView } from "./agent-work.js";
export const AGENT_SHAPES = ["droplet", "blossom", "pebble", "triangle", "diamond", "arch", "star", "hexagon", "square"] as const;
export type AgentShape = typeof AGENT_SHAPES[number];
export type AgentColor = "teal" | "coral" | "sky" | "sand" | "violet" | "moss";
export interface AgentProfile {
  id: string; name: string; engine: string; model: string; effort: string; instructions: string;
  color: AgentColor; shape?: AgentShape;
  /** Rust-owned: set when the home folder is created. Renderer values are ignored. */
  home?: string;
  /** Rust-owned: changed only by `saveAgentFolders`. */
  folders?: string[];
  /** SHA-256 (lowercase hex) of the instruction files Interface last wrote. */
  instructionsFileSha256?: string;
}
export interface AgentRoutine { id: string; agentId: string; name: string; prompt: string; cwd: string; enabled: boolean; startAtMs: number; intervalMinutes: number | null; nextRunAtMs: number | null }
export type AgentRunStatus = "queued" | "starting" | "running" | "waiting" | "succeeded" | "failed" | "unknown";
export interface AgentRun { id: string; routineId: string; agentId: string; name: string; scheduledAtMs: number; updatedAtMs: number; status: AgentRunStatus; sessionKey: string | null; detail: string; profile: AgentProfile; prompt: string; cwd: string; began: boolean }

export type AgentThreadStatus = "idle" | "working" | "waiting";
export interface AgentThread { sessionKey: string; startedAtMs: number; status: AgentThreadStatus; interrupted: boolean }
export interface AgentPastThread { sessionKey: string; startedAtMs: number; endedAtMs: number }
/** Rust leaves `past` off the wire while it is empty (an agent's first thread). */
export interface AgentThreadState { current?: AgentThread; past?: AgentPastThread[]; lastReadAtMs: number; lastActivityAtMs: number }

export interface AgentWorkspace {
  version: 1 | 2; revision: number; agents: AgentProfile[]; assignments: Record<string, string>;
  routines: AgentRoutine[]; runs: AgentRun[]; threads?: Record<string, AgentThreadState>;
}

/** How an agent gets permission to act. Rust-owned; an agent missing from `permissionModes` is gated. */
export type AgentPermissionMode = "gated" | "auto" | "bypass";
export type AgentStatus = "idle" | "working" | "needsAttention" | "unread";
export interface AgentPendingApproval {
  agentId: string; sessionKey: string; approvalId: string; tool: string; summary: string; reason: string;
  askedAtMs: number;
}
export interface AgentFileText { text: string; sha256: string; tooLarge: boolean }
export interface AgentFiles {
  home: string | null; exists: boolean; markerOk: boolean; instructions: AgentFileText | null;
  instructionsConflict: boolean; memory: AgentFileText | null;
}
/** Provider-neutral description of an approval request for Interface's agent policy. */
export type ApprovalFacts =
  | { kind: "read"; paths: string[] }
  | { kind: "search"; paths: string[] }
  | { kind: "write"; paths: string[] }
  | { kind: "delete"; paths: string[] }
  | { kind: "shell"; command: string; cwd: string | null }
  | { kind: "webSearch" }
  | { kind: "webFetch"; url: string; method: string }
  | { kind: "mcp"; server: string; tool: string }
  /** Harness-internal bookkeeping with no external effect (todo lists, subagent spawn). */
  | { kind: "internal"; tool: string }
  | { kind: "unknown"; tool: string };
export type AgentDecisionKind = "allowed" | "asked" | "ownerAllowed" | "ownerDenied" | "failedClosed";
export type AgentDeliveryOutcome = "sent" | "unsent" | "unknown" | "pending";
export interface AgentDecision {
  atMs: number; sessionKey: string; approvalId: string; tool: string; summary: string; facts: ApprovalFacts;
  decision: AgentDecisionKind; reason: string; outcome: AgentDeliveryOutcome;
}

/** Every `iface:agentControl` answer. Never persisted. */
export type AgentControlResponse = AgentWorkspace & {
  statuses: Record<string, AgentStatus>; pendingApprovals: AgentPendingApproval[];
  calendar?: AgentCalendarView; work?: AgentWorkView; files?: AgentFiles; decisions?: AgentDecision[]; sessionKey?: string;
  /** Agents whose mode is not gated, by id. */
  permissionModes?: Record<string, AgentPermissionMode>;
};

export type AgentMutation =
  | { kind: "saveAgent"; profile: AgentProfile }
  | { kind: "removeAgent"; id: string }
  | { kind: "assign"; sessionKey: string; agentId: string }
  | { kind: "saveRoutine"; routine: AgentRoutine }
  | { kind: "removeRoutine"; id: string }
  | { kind: "runNow"; routineId: string; requestId: string }
  | { kind: "acknowledgeRun"; id: string }
  | { kind: "newThread"; agentId: string }
  | { kind: "saveAgentFolders"; agentId: string; folders: string[] };

/** Host-only or read-side agent commands. None of these carry a revision. */
export type AgentRequest =
  | { kind: "calendar"; agentId: string; command: AgentCalendarCommand }
  | { kind: "work"; agentId: string; command: AgentWorkCommand }
  /** Host-only. Creates the agent's thread with `text` as its first prompt. */
  | { kind: "startAgentThread"; agentId: string; text: string; requestId: string }
  /**
   * Host-only (protocol 38). The owner's one-off task for the agent, launched
   * behind its gate in `cwd` with its home, folders and permission mode. The
   * answer's `sessionKey` names the new task. One `requestId` launches at most once.
   */
  | { kind: "startAgentTask"; agentId: string; text: string; cwd: string; requestId: string;
    title?: string; model?: string; effort?: string; recovery?: import("./recovery.js").PromptRecoveryInput }
  | { kind: "markRead"; agentId: string }
  | { kind: "readAgentFiles"; agentId: string }
  | { kind: "saveAgentMemory"; agentId: string; expectedSha256: string; text: string }
  | { kind: "overwriteAgentInstructionsFile"; agentId: string; expectedSha256: string }
  | { kind: "readAgentDecisions"; agentId: string; limit: number }
  | { kind: "recreateAgentHome"; agentId: string }
  | { kind: "revealAgentHome"; agentId: string }
  /** Applies from the agent's next thread start, routine run or resume. */
  | { kind: "savePermissionMode"; agentId: string; mode: AgentPermissionMode };

export type AgentCommand =
  | { kind: "read" }
  | { kind: "importLegacy"; agents: AgentProfile[]; assignments: Record<string, string> }
  | AgentRequest
  | (AgentMutation & { expectedRevision: number });

export const emptyAgentWorkspace = (): AgentWorkspace =>
  ({ version: 2, revision: 0, agents: [], assignments: {}, routines: [], runs: [] });
export const emptyAgentControlResponse = (): AgentControlResponse =>
  ({ ...emptyAgentWorkspace(), statuses: {}, pendingApprovals: [] });
export const agentPermissionMode = (response: AgentControlResponse, agentId: string): AgentPermissionMode =>
  response.permissionModes?.[agentId] ?? "gated";
/** The request that saves an agent's new permission mode, or null when it did not change. */
export const permissionModeChange = (agentId: string, current: AgentPermissionMode, next: AgentPermissionMode):
  Extract<AgentRequest, { kind: "savePermissionMode" }> | null =>
  current === next ? null : { kind: "savePermissionMode", agentId, mode: next };
