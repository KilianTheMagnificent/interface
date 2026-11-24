/** Control protocol v1. Rust owns grants, state, deadlines and delivery receipts. */
export type ControlCommand =
  | { kind: "inspect" }
  | { kind: "begin"; runId: string; cwd: string; durationSeconds: number; maxActions: number }
  | { kind: "message"; runId: string; requestId: string; text: string }
  | { kind: "stop"; runId: string }
  | { kind: "quiet"; runId: string; quiet: boolean }
  | { kind: "endConversation"; runId: string }
  | { kind: "approve"; runId: string; requestId: string; approvalId: string; allow: boolean };
export interface ControlRequest { version: 1; command: ControlCommand }
export type ControlTarget = { kind: "pane"; id: string; generation: number } | { kind: "session"; key: string };
export interface ControlReceipt {
  action: { requestId: string; tool: string; input: unknown };
  state: "started" | "accepted" | "unsent" | "unknown";
  detail: string;
  result: unknown;
}
export interface ControlRun {
  id: string; cwd: string; startedAtMs: number; deadlineAtMs: number; maxActions: number;
  state: "starting" | "active" | "stopped" | "expired" | "recoveryRequired";
  sessionKey: string | null; quiet: boolean; conversationOpen: boolean;
  targets: ControlTarget[]; controlledTargets: ControlTarget[]; receipts: ControlReceipt[];
  stops: { target: ControlTarget; state: ControlReceipt["state"]; detail: string }[];
  coordinatorBusy: boolean; observations: { sessionKey: string; status: string }[];
  pendingUpdates: ControlReceipt["action"][];
  notes: { role: string; text: string }[];
  approvals: { id: string; sessionKey: string; providerApprovalId: string; description: string }[];
}
export interface ControlSnapshot {
  version: 1; revision: number; runs: ControlRun[]; storageUncertain: boolean; audioConnected: boolean;
}
