/**
 * Thread references (T3 section 9): the `@`/+ context picker's Thread kind
 * in a project thread's composer. This module only decides what the picker
 * OFFERS (the project's other threads, newest activity first) and the chip a
 * choice becomes. Whether the sender may read the thread, and what the chip
 * says at send, are Rust's (`validatePromptContext` answers the block in the
 * chip's `safeContent`); the renderer never builds that block.
 */

import { sessionKey, type SessionKey, type SessionSummary } from "../shared/engine";
import type { ThreadsView, ThreadView } from "../shared/threads";
import type { PromptContextCapability, PromptContextItem } from "./prompt-context";
import { providerLabel } from "./provider-labels";

export interface ThreadReferenceChoice {
  key: SessionKey;
  title: string;
  /** "Claude", "Codex"… */
  provider: string;
  /** Milliseconds since the Unix epoch: the last run's end or start, else creation. */
  lastActivityAt: number;
}

/** A chip's label, in characters. */
const LABEL_MAX_CHARS = 80;

function projectOf(thread: ThreadView | undefined): string | null {
  const workspace = thread?.workspace;
  if (workspace === undefined || workspace.mode === "none" || workspace.projectRoot === "") return null;
  return workspace.projectRoot;
}

function lastActivity(thread: ThreadView): number {
  const run = thread.lastRun;
  return Math.max(thread.createdAt, run === undefined ? 0 : run.endedAt ?? run.startedAt);
}

/**
 * The threads `sender` may refer to, as the picker lists them: listed
 * threads of the sender's project, not the sender, not hidden workers, not
 * agents' sessions, newest activity first. Empty when the sender is not a
 * project thread. Rust checks the same rule again at send.
 */
export function threadReferenceChoices(view: ThreadsView | null,
    sessions: readonly SessionSummary[], sender: SessionKey): ThreadReferenceChoice[] {
  if (view === null) return [];
  const project = projectOf(view.threads.find(thread => thread.key === sender));
  if (project === null) return [];
  const hidden = new Set(view.hiddenKeys ?? []);
  const summaries = new Map(sessions.map(summary =>
    [sessionKey(summary.engine, summary.sessionId), summary] as const));
  return view.threads
    .filter(thread => thread.key !== sender && !hidden.has(thread.key)
      && projectOf(thread) === project)
    .flatMap(thread => {
      const summary = summaries.get(thread.key);
      if (summary === undefined || summary.kind === "agent") return [];
      return [{
        key: thread.key,
        title: summary.title.trim() || thread.key,
        provider: providerLabel(summary.engine),
        lastActivityAt: lastActivity(thread),
      }];
    })
    .sort((left, right) => right.lastActivityAt - left.lastActivityAt);
}

/** Whether the picker's Thread kind is offered in `sender`'s composer, and why not. */
export function threadReferenceCapability(view: ThreadsView | null,
    sender: SessionKey): PromptContextCapability {
  const thread = view?.threads.find(candidate => candidate.key === sender);
  if (thread === undefined) {
    return { available: false,
      unavailableReason: "A thread can refer to other threads; this pane is not a thread." };
  }
  if (projectOf(thread) === null) {
    return { available: false,
      unavailableReason: "A thread with a project can refer to that project's other threads." };
  }
  return { available: true };
}

/** The chip a chosen thread becomes. Its value is the thread's key. */
export function threadReferenceItem(choice: ThreadReferenceChoice, id: string): PromptContextItem {
  const chars = [...choice.title.replace(/\s+/gu, " ").trim()];
  const label = chars.length > LABEL_MAX_CHARS
    ? `${chars.slice(0, LABEL_MAX_CHARS - 1).join("").trimEnd()}…` : chars.join("");
  return {
    id,
    kind: "chat",
    label: label || choice.key,
    value: choice.key,
    detail: `${choice.provider} thread · ${choice.key}`,
  };
}
