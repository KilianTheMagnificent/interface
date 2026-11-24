import type { ConvItem } from "./conversation-reducer";
import type { TurnRestorePoint } from "./conversation-types";
import type { PromptSubmitKeyEvent } from "./prompt-submit";

/**
 * The conversation pane's pure rules: who owns a window-level shortcut, how a
 * composer submission dispatches, what a failed send leaves in the draft, what
 * the restore dialog is about and what a finished turn announces. The
 * components in `components/Conversation*.tsx` apply them; the suites prove
 * them without a DOM.
 */

/**
 * Pure ownership rule for the window-level Cmd/Ctrl+F listener. Focus must be
 * inside this conversation (or neutral), and the event must originate inside it
 * (or at window/document/body). This keeps a focused-but-hidden agent from
 * stealing Find from Monaco, sidebars, or modals even for a synthetic event.
 */
export function conversationOwnsFindShortcut(
  targetInside: boolean,
  activeInside: boolean,
  targetNeutral: boolean,
  activeNeutral: boolean,
): boolean {
  if (!activeInside && !activeNeutral) return false;
  return targetInside || targetNeutral;
}

export type ConversationSubmitAction = "ignore" | "queue" | "send";

/**
 * Pure dispatch rule shared by click and Enter. `steer` is the explicit verb: it
 * refuses the queue for THIS submission only, and changes nothing else — a
 * locked composer and an empty draft still win over it.
 */
export function conversationSubmitAction(draft: string, queueWhileBusy: boolean,
    canQueue: boolean, locked = false, steer = false): ConversationSubmitAction {
  if (locked) return "ignore";
  if (draft.trim() === "") return "ignore";
  return queueWhileBusy && canQueue && !steer ? "queue" : "send";
}

/**
 * Alt/Option+Enter delivers into the running turn, and ONLY where the Steer
 * control is offered beside it. Everywhere else the chord stays the newline
 * promptSubmitKeyAction has always made it: a hidden second send on an engine
 * that cannot take mid-turn input would be a keystroke with no visible verb.
 *
 * `offered` must be the SAME predicate the button is enabled by — including the
 * empty draft and the locked composer, not just the engine's capability. The
 * caller preventDefaults on "steer", so a chord admitted on terms the dispatch
 * rule then refuses eats the newline and does nothing with it.
 */
export function conversationSteerKeyAction(event: PromptSubmitKeyEvent,
    offered: boolean): "steer" | "ignore" {
  if (!offered) return "ignore";
  if (event.key !== "Enter" || event.altKey !== true || event.shiftKey) return "ignore";
  if (event.isComposing === true || event.nativeEvent?.isComposing === true) return "ignore";
  if (event.nativeEvent?.keyCode === 229) return "ignore";
  return "steer";
}

/**
 * Escape as a terminal means it: stop what is running.
 *
 * TWO REFUSALS, both deliberate. With no turn in flight the key belongs to
 * whatever else wants it (the find bar's own listener, a picker's popover), and
 * claiming it would break those. With the slash-command menu open, Escape
 * already means "close this menu" and stealing it would make the menu
 * uncloseable while a turn runs.
 */
export function conversationEscapeAction(
  event: PromptSubmitKeyEvent,
  context: { thinking: boolean; commandMenuOpen: boolean },
): "stop" | "pass" {
  if (event.key !== "Escape") return "pass";
  if (context.commandMenuOpen || !context.thinking) return "pass";
  return "stop";
}

/** Only an explicit false is a queue refusal; void preserves fire-and-forget callers. */
export function queueSubmissionAccepted(result: boolean | void): boolean {
  return result !== false;
}

/**
 * What the composer holds after a send that definitely failed. Dispatch cleared
 * it, and the words never left the app, so they go back — but only into a draft
 * still untouched since. Anything the user typed while the send was in flight is
 * newer than the failure and always wins.
 */
export function draftAfterFailedSend(current: string, outbound: string): string {
  return current === "" ? outbound : current;
}

/**
 * What the restore dialog is about, or null for nothing to confirm.
 *
 * The second half is the one that matters: a pending key whose restore point
 * has gone takes the dialog down with it. The map is the caller's and it is
 * rebuilt — a fresh checkpoint list, a restore that just landed, a session that
 * moved on — and a confirm button still pointed at a checkpoint that no longer
 * exists is the exact failure this control was added to prevent.
 */
export function turnRestoreConfirmation(
  pendingKey: string | null,
  points: ReadonlyMap<string, TurnRestorePoint> | undefined,
): { key: string; point: TurnRestorePoint } | null {
  if (pendingKey === null || points === undefined) return null;
  const point = points.get(pendingKey);
  return point === undefined ? null : { key: pendingKey, point };
}

/**
 * What the live region says when a turn ends: the newest non-blank assistant
 * message on one line, capped at 500 characters, or a plain "finished" when
 * the turn said nothing. Read once, on the edge into idle.
 */
export function finishedTurnAnnouncement(items: readonly ConvItem[]): string {
  for (let index = items.length - 1; index >= 0; index -= 1) {
    const item = items[index];
    if (item.kind !== "text" || item.text.trim() === "") continue;
    const compact = item.text.replace(/\s+/g, " ").trim();
    return `Agent response: ${compact.length > 500 ? `${compact.slice(0, 497)}…` : compact}`;
  }
  return "Agent response finished.";
}
