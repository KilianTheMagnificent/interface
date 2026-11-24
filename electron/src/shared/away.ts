/** Desktop project Away v1; all scheduling, presets and receipts belong to Rust. */
export interface AwayOptions {
  objective: string; provider: "codex" | "claude"; model: string | null; durationSeconds: number; maxIterations: number;
  /** Protocol 47: the provider whose fresh sessions review each round; absent, the worker's own. */
  reviewer?: "codex" | "claude";
}
export type AwayCommand =
  | { kind: "inspect" }
  | { kind: "start"; runId: string; cwd: string; options: AwayOptions | null }
  | { kind: "stop"; runId: string }
  | { kind: "save"; cwd: string; options: AwayOptions };
export type AwayRequest = { version: 1; command: AwayCommand } | { version: 2; command: { kind: "status" } };
export type AwayRole = "work" | "review";
export type AwayState = "running" | "stopping" | "completed" | "stopped" | "expired" | "blocked" | "unknown";
export interface AwayTurn {
  iteration: number; role: AwayRole; sessionKey: string | null; state: string; report: string;
  /** Protocol 45: a work turn's ordinal in its run's thread. */
  threadTurn?: number;
}
/** Protocol 45: the thread a loop runs as, in the loop's own worktree. */
export interface AwayRunThread { key: string; worktree: string; branch?: string }
export interface AwayRun {
  id: string; cwd: string; options: AwayOptions; startedAtMs: number; deadlineAtMs: number;
  state: AwayState;
  detail: string; turns: AwayTurn[];
  /** Protocol 45: absent until the loop's thread exists, and for loops from before. */
  thread?: AwayRunThread;
}

/** Protocol 45: a round's verdict, from its `AWAY_RESULT` line. */
export type AwayVerdict = "continue" | "complete" | "revise" | "blocked";
export type AwayRoundState = "started" | "completed" | "blocked" | "unknown" | "cancelled";
/** One round of a loop as its thread shows it; texts are cut for display. */
export interface ThreadAwayRound {
  iteration: number; role: AwayRole; state: AwayRoundState;
  /** A work round: the thread turn it ran as. */
  turn?: number;
  /** A review round: its own session, kept out of Threads. */
  sessionKey?: string;
  verdict?: AwayVerdict; summary?: string; checks?: string[];
}
/** Protocol 45: Rust's projection of the Away loop a thread runs as. */
export interface ThreadAway {
  runId: string; state: AwayState; detail: string; provider: string; model?: string;
  /** Protocol 47: the provider that reviews each round. */
  reviewer?: string;
  objective: string;
  startedAt: number; deadlineAt: number; maxIterations: number; rounds: ThreadAwayRound[];
}
export interface AwaySnapshot { version: 1; revision: number; defaults: AwayOptions; presets: { cwd: string; options: AwayOptions }[]; runs: AwayRun[]; storageUncertain: boolean; cancelledStarts: string[] }

/** Read-only v2 summary; v1 persisted data and mutation replies are unchanged. */
export interface AwayStatus { version: 2; revision: number; storageUncertain: boolean; activeRuns: { id: string; cwd: string }[]; runIds: string[]; cancelledStarts: string[] }
export type AwayResponse = AwaySnapshot | AwayStatus;
