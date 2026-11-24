import type { SessionStatus } from "../shared/engine";
import type { ThreadView, ThreadsView } from "../shared/threads";

/**
 * Team tools (protocol 57): the compact "Started threads" line above a
 * thread's composer — each thread its agent started, newest first, as its
 * status mark and title, at most {@link STARTED_THREADS_SHOWN} with "+N more".
 * Derived from the Threads view Rust pushes (threads whose lineage is
 * `{parent: this key, relation: child}`); no channel of its own.
 */
export const STARTED_THREADS_SHOWN = 6;

export type StartedThreadStatus = "running" | "needsYou" | "done" | "failed" | "stopped";

export interface StartedThreadItem {
  key: string;
  title: string;
  status: StartedThreadStatus;
}

export interface StartedThreadsLine {
  items: readonly StartedThreadItem[];
  /** How many more started threads the line leaves out. */
  more: number;
}

/** A started thread's status mark: its run, else how its last run ended. */
export function startedThreadStatus(thread: Pick<ThreadView, "lastRun">,
    session: SessionStatus | undefined): StartedThreadStatus {
  const run = thread.lastRun;
  if (run === undefined || run.endedAt === undefined) {
    return session === "waiting" ? "needsYou" : "running";
  }
  switch (run.outcome) {
    case "failed": return "failed";
    case "interrupted": return "stopped";
    default: return "done";
  }
}

/** The status mark's words, for its label and title. */
export function startedThreadStatusWords(status: StartedThreadStatus): string {
  switch (status) {
    case "running": return "Running";
    case "needsYou": return "Needs you";
    case "done": return "Done";
    case "failed": return "Failed";
    case "stopped": return "Stopped";
  }
}

/**
 * Every caller's line, by its key: only callers that started at least one
 * thread appear. `session` answers a thread's title and registry status;
 * a thread the registry does not know yet is left out until it does.
 */
export function startedThreadsByParent(view: ThreadsView | null,
    session: (key: string) => { title: string; status: SessionStatus } | undefined):
    ReadonlyMap<string, StartedThreadsLine> {
  const byParent = new Map<string, ThreadView[]>();
  for (const thread of view?.threads ?? []) {
    const lineage = thread.lineage;
    if (lineage?.relation !== "child") continue;
    const started = byParent.get(lineage.parent) ?? [];
    started.push(thread);
    byParent.set(lineage.parent, started);
  }
  const lines = new Map<string, StartedThreadsLine>();
  for (const [parent, threads] of byParent) {
    const items = threads
      .map(thread => ({ thread, known: session(thread.key) }))
      .filter((entry): entry is { thread: ThreadView; known: { title: string; status: SessionStatus } } =>
        entry.known !== undefined)
      .sort((left, right) => (right.thread.lineage?.at ?? right.thread.createdAt)
        - (left.thread.lineage?.at ?? left.thread.createdAt))
      .map(({ thread, known }) => ({
        key: thread.key,
        title: known.title,
        status: startedThreadStatus(thread, known.status),
      }));
    if (items.length === 0) continue;
    lines.set(parent, {
      items: items.slice(0, STARTED_THREADS_SHOWN),
      more: Math.max(0, items.length - STARTED_THREADS_SHOWN),
    });
  }
  return lines;
}
