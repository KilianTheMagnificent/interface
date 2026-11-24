export type UpdateSessionsCommand = { readonly command: "read" | "prepare" | "cancel" | "resume" };
export type UpdateSessionsPhase = "idle" | "checkpointing" | "ready" | "installing"
  | "resuming" | "complete" | "cancelled" | "blocked";
export interface UpdateSessionView {
  readonly id: string;
  readonly title: string;
  readonly phase: "pending" | "requestStarted" | "waiting" | "ready" | "resumeStarted" | "resumed" | "blocked";
  readonly detail: string;
}
export interface UpdateSessionsView {
  readonly version: 1;
  readonly phase: UpdateSessionsPhase;
  readonly sessions: readonly UpdateSessionView[];
  readonly detail: string;
}
export function isUpdateSessionsView(value: unknown): value is UpdateSessionsView {
  if (typeof value !== "object" || value === null) return false;
  const view = value as Record<string, unknown>;
  return view.version === 1
    && ["idle", "checkpointing", "ready", "installing", "resuming", "complete", "cancelled", "blocked"].includes(String(view.phase))
    && typeof view.detail === "string"
    && Array.isArray(view.sessions) && view.sessions.length <= 512
    && view.sessions.every((row: unknown) => {
      if (typeof row !== "object" || row === null) return false;
      const item = row as Record<string, unknown>;
      return typeof item.id === "string" && typeof item.title === "string" && typeof item.detail === "string"
        && ["pending", "requestStarted", "waiting", "ready", "resumeStarted", "resumed", "blocked"].includes(String(item.phase));
    });
}
