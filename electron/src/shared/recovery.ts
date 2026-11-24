import type { PromptContextDescriptor } from "./ipc.js";
export type RecoveryMode = "retry" | "continue";
export interface RecoverySource { id: string; mode: RecoveryMode }
export interface PromptRecoveryInput { text: string; context: PromptContextDescriptor[]; source?: RecoverySource }
export interface PromptRecoveryReceipt { id: string; key: string; input: PromptRecoveryInput; serialized: string; delivery: "unsent" | "accepted" | "unknown"; at: number }
export type RecoveryRequest = { version: 1; key: string } & ({ kind: "read" } | { kind: "listPending" } | { kind: "acknowledgeLaunch"; id: string } | { kind: "retainDrafts"; paths: string[] } | { kind: "prepare"; id: string; mode: RecoveryMode } | { kind: "forget"; id: string });
export interface RecoveryResponse { version: 1; receipt: PromptRecoveryReceipt | null; prepared: PromptRecoveryInput | null; pending?: PromptRecoverySummary[] }

export interface PromptRecoverySummary { key: string; id: string; preview: string; delivery: PromptRecoveryReceipt["delivery"]; sessionKey?: string }
