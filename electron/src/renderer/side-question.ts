import type { EngineEvent } from "../shared/engine";
import type { ThreadSide } from "../shared/threads";
import { appendReplyQuote, type DraftWrite } from "./composer-quote";

/* ============================================================================
   SIDE QUESTIONS (protocol 67, T3 section 12) — the panel's words

   Rust forks the thread into a hidden side thread and asks it the question;
   the side thread's events stream to the renderer like any session's. These
   helpers turn them into what the right column shows (the answer so far and
   where it stands), parse `/btw`, and quote the answer into the main thread's
   draft for Bring back. Pure: no store, no `window.iface`.
   ========================================================================= */

/** Whether a thread on `engine` can ask a side question: Rust forks only Claude and Codex threads. */
export function sideQuestionOffered(engine: string): boolean {
  return engine === "claude" || engine === "codex";
}

/** Where a side question stands. */
export type SideState = "asking" | "answering" | "answered" | "failed";

export interface SideAnswer {
  state: SideState;
  /** The side thread's answer so far (its main agent's text). */
  text: string;
  /** Why it failed, when it did. */
  error?: string;
}

/**
 * The answer in `events` (the side thread's, oldest first): the main agent's
 * text after the question, and whether it is still coming. A fork's copied
 * history is not in its live events, so everything here is the answer.
 */
export function sideAnswer(events: readonly EngineEvent[]): SideAnswer {
  let text = "";
  let state: SideState = "asking";
  let error: string | undefined;
  for (const event of events) {
    switch (event.kind) {
      case "message.delta":
        if (event.agentKey === undefined) {
          text += event.text;
          if (state === "asking") state = "answering";
        }
        break;
      case "status.changed":
        if (event.status === "working" && state === "asking") state = "answering";
        if (event.status === "idle" && state !== "failed") state = text.trim() === "" ? state : "answered";
        if (event.status === "failed") state = "failed";
        break;
      case "error":
        if (!event.retryable) {
          state = "failed";
          error = event.message;
        }
        break;
      default:
        break;
    }
  }
  return { state, text: text.trim(), ...(error === undefined ? {} : { error }) };
}

/** The panel's status line. */
export function sideStatusText(answer: SideAnswer): string {
  switch (answer.state) {
    case "asking": return "Asking…";
    case "answering": return "Answering…";
    case "answered": return "Answered. Nothing was added to the thread.";
    case "failed": return answer.error === undefined ? "The side question failed." : `Failed: ${answer.error}`;
  }
}

/**
 * `/btw <question>` in a thread's composer: the question, "" for a bare
 * `/btw`, or null when the draft is not one.
 */
export function sideQuestionCommand(draft: string): string | null {
  const match = /^\s*\/btw(?:\s+([\s\S]*))?$/iu.exec(draft);
  if (match === null) return null;
  return (match[1] ?? "").trim();
}

/**
 * Bring back: the main thread's draft with the side answer quoted after it,
 * led by the question it answers. Null when there is no answer yet.
 */
export function bringBackSide(draft: string, side: Pick<ThreadSide, "question">, answer: string): DraftWrite | null {
  if (answer.trim() === "") return null;
  const question = side.question.replace(/\s+/gu, " ").trim();
  return appendReplyQuote(draft, `Side question: ${question}\n\n${answer}`);
}
