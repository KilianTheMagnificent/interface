/** Rust-owned delegation projection, desktop protocol v12. */
import type { TerminalSummary } from "./ipc.js";
export interface ReadTerminalDelegation { id: string; generation: number; taskId?: string }
export interface DelegationActivityView {
  entries: { sequence: number; kind: string; text: string }[]; truncated: boolean;
}
export interface DelegationTaskView {
  id: string; title: string; model: string; state: string; notes: string | null;
  activity: DelegationActivityView | null;
  terminal: TerminalSummary | null;
  files: string[]; elapsedMs: number | null; usage: Record<string, number | null> | null;
}
export interface DelegationWorkflowView {
  id: string; title: string; decision: string; reason: string | null; state: string;
  hookObserved: boolean; parallelTools: number; checks: string[]; tasks: DelegationTaskView[];
}
export interface TerminalDelegationView {
  version: 3; available: boolean; workflows: DelegationWorkflowView[];
  detail: { task: { state: string }; resultSha256: string;
    result: { summary: string; changes: { path: string; before?: string | null;
      after: string | null; beforeSha256: string | null; sourceChanged: boolean }[] } | null;
  } | null;
}
