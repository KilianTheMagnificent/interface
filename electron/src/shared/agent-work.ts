/** Protocol v27. Rust owns admission, budgets, verification and memory authority. */
export interface AgentBudget { wallSeconds: number; noProgressSeconds: number; maxActions: number; maxReportedTokens: number | null }
export interface AgentArtifactCheck { path: string; format: "nonempty" | "json" | "text"; contains: string | null }
export interface AgentInputRequirement { path: string; maxAgeSeconds: number }
export interface AgentWorkSpec { expectedOutput: string; artifacts: AgentArtifactCheck[]; inputs: AgentInputRequirement[]; budget: AgentBudget; readOnly: boolean; allowOpaqueShellCommands: boolean; allowWebSearch?: boolean; consultation?: AgentConsultationGrant }
export type AgentExecutionState = "starting" | "running" | "completed" | "failed" | "stopped" | "unknown";
export type AgentVerificationState = "unverified" | "verified" | "failed" | "stale";
export interface AgentEvidence { path: string; sha256: string; bytes: number; checkedAtMs: number }
export interface AgentTaskRecord {
  id: string; agentId: string; sessionKey: string | null; goal: string; spec: AgentWorkSpec; roots: string[]; cwd: string;
  startedAtMs: number; deadlineAtMs: number; progressAtMs: number; progressSha256: string; began: boolean; actions: number; reportedTokens: number | null;
  execution: AgentExecutionState; verification: AgentVerificationState; inputEvidence: AgentEvidence[]; artifacts: AgentEvidence[];
  detail: string; nextAction: string; stopRequested: boolean; notes: string[];
}
export interface AgentMemoryRevision { sha256: string; text: string; observedAtMs: number; source: string }
export interface AgentMemoryFact { id: string; text: string; source: string; observedAtMs: number; expiresAtMs: number | null; supersedes: string | null; state: "proposed" | "ownerConfirmed" | "superseded" }
export interface AgentWorkView { version: 1; revision: number; spec: AgentWorkSpec; tasks: AgentTaskRecord[]; memoryHistory: AgentMemoryRevision[]; facts: AgentMemoryFact[]; consultations?: AgentConsultation[]; expiredApprovals?: import("./agents.js").AgentPendingApproval[] }
export type AgentWorkCommand =
  | { action: "read" }
  | { action: "configure"; expectedRevision: number; spec: AgentWorkSpec }
  | { action: "verify" | "stop" | "acknowledge"; taskId: string }
  | { action: "restoreMemory"; sha256: string; expectedSha256: string }
  | { action: "proposeFact"; fact: AgentMemoryFact }
  | { action: "confirmFact"; id: string };
export interface AgentCalendarSchedule { timezone: string; weekdays: number[]; hour: number; minute: number; missed: "skip" | "runOnce"; pauseOnFailure: boolean }
export interface AgentCalendarView { routineId: string; schedule: AgentCalendarSchedule | null; nextOccurrencesMs: number[]; tested: boolean }
export type AgentCalendarCommand =
  | { action: "read"; routineId: string }
  | { action: "save"; expectedRevision: number; routineId: string; schedule: AgentCalendarSchedule | null }
  | { action: "preview"; schedule: AgentCalendarSchedule; afterMs: number };

export interface AgentConsultationGrant { model: string; files: string[]; writableFiles: string[]; maxChildren: number }
export interface AgentChildProposal { path: string; expectedSha256: string; text: string; appliedSha256: string | null }
export interface AgentConsultation { id: string; taskId: string; model: string; brief: string; inputs: AgentEvidence[]; resultSha256: string | null; report: string; proposals: AgentChildProposal[]; status: string; review: string | null; elapsedMs: number | null; reportedTokens: number | null }
