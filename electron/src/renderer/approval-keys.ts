import type { ConvItem } from "./conversation-reducer";

/**
 * Approval verdicts the caller holds, which request owns the keyboard chord,
 * and the chord itself: its keys, its reveal-then-answer step and its typing guard.
 */

/**
 * A verdict THIS window has sent for one approval, and whether it landed.
 * "sent", never "settled": what the app knows is that it called `respond`, and
 * only the engine dropping the request settles it.
 */
export interface ApprovalVerdict { verdict: "allow" | "deny"; error?: string }

/**
 * approvalVerdictKey() -> the verdict sent for that approval. ONE map, owned
 * above both surfaces that could answer an approval (the sidebar peek and the
 * conversation pane) and passed to both — a private latch in each let the user
 * allow in one, open the session, be offered Allow/Deny again, click
 * Deny, and have the pane print "Denied" for a command the engine had already
 * run: the bridge drops a verdict for a request that is no longer outstanding
 * with nothing but a console warning.
 */
export type ApprovalVerdicts = Readonly<Record<string, ApprovalVerdict>>;

/**
 * The map key. Session-scoped because an approvalId is only unique within its
 * own session, and NUL-joined because neither half can contain one: a
 * SessionKey is `${engine}#${sessionId}` and an approvalId is a randomUUID.
 */
export function approvalVerdictKey(session: string, approvalId: string): string {
  return `${session}\u0000${approvalId}`;
}

/**
 * The ONE approval card in a pane that may print the chord — the last one still
 * answerable, in document order, which is exactly the card the pane's listener
 * clicks (it takes the last `data-approval-key` button under its root).
 *
 * A pane CAN hold two outstanding requests, and then the tie-break has to be
 * visible: a chip on the upper card while the chord lands on the lower one
 * allows a command the reader was not looking at, and a disabled Allow at the
 * bottom would leave the chord silently dead under a chip that promised it
 * works. Printing the key on precisely one card makes the rule legible instead
 * of hidden.
 *
 * "Answerable" is the same three conditions the card itself renders buttons
 * under: the pane is live, the engine still lists the request, and no verdict
 * has been sent for it. A FAILED send is not a verdict — the card puts its
 * buttons back, so it is still the card the chord must reach.
 *
 * Subagent children are walked because they draw INSIDE their parent's card, so
 * a nested request is earlier in the document than any top-level row after it.
 */
export function keyedApprovalId(
  items: readonly ConvItem[],
  live: boolean,
  activeApprovalIds: ReadonlySet<string>,
  approvals: ApprovalVerdicts,
  sessionKey: string,
): string | null {
  // Every candidate must be in `activeApprovalIds`, so an empty set answers
  // without walking a transcript that holds no answerable request.
  if (!live || activeApprovalIds.size === 0) return null;
  for (let index = items.length - 1; index >= 0; index -= 1) {
    const item = items[index];
    if (item === undefined) continue;
    if (item.kind === "subagent") {
      const nested = keyedApprovalId(
        item.children, live, activeApprovalIds, approvals, sessionKey);
      if (nested !== null) return nested;
      continue;
    }
    if (item.kind !== "approval") continue;
    if (!activeApprovalIds.has(item.id)) continue;
    const answer = approvals[approvalVerdictKey(sessionKey, item.id)];
    const sent = answer?.error === undefined
      ? answer?.verdict ?? item.resolved : undefined;
    if (sent !== undefined) continue;
    return item.id;
  }
  return null;
}

/* ---- the keys a blocking request answers to ------------------------------
 *
 * ONE definition, read by all three surfaces that print or honour them: the
 * transcript's approval card and the Settings
 * shortcut reference. The reference page spreads these same arrays into
 * SETTINGS_SHORTCUTS, so the chip on a button face and the documented copy
 * cannot drift apart.
 *
 * WHY THESE TWO. Cmd/Ctrl+Enter and Cmd/Ctrl+Backspace are free everywhere
 * OUTSIDE a text field and taken inside one — Enter and Cmd+Enter both send a
 * prompt (promptSubmitKeyAction), and Cmd+Enter saves a queued-prompt edit
 * (SteeringQueue) — which is exactly why approvalKeyVerdict refuses to answer
 * anything while the keystroke came from somewhere text is typed. A composer
 * keystroke with a draft in it never answers an approval. The one exception
 * is a Threads pane's EMPTY composer under a docked request (owner,
 * 2026-10-07): an empty draft has nothing to send or queue, so its ⌘↵ and ⌘⌫
 * press the docked card's keyed button. The field decides that itself
 * (ConversationComposer); this guard still refuses every text field.
 * Backspace carries no binding anywhere in the renderer and no menu
 * accelerator; inside a text field macOS gives it "delete to start of line",
 * which the same guard leaves alone (and an empty field has nothing to delete).
 *
 * Alt+Enter is NOT offered as "deny with a note" even though the deny path has
 * one: that chord already steers a running turn (conversationSteerKeyAction).
 */
export const APPROVAL_ALLOW_KEYS = ["⌘", "↩"] as const;
export const APPROVAL_DENY_KEYS = ["⌘", "⌫"] as const;
export const APPROVAL_ALLOW_KEYSHORTCUTS = "Meta+Enter Control+Enter";
export const APPROVAL_DENY_KEYSHORTCUTS = "Meta+Backspace Control+Backspace";

/**
 * Marks the button a key press CLICKS. The keyboard never gets a verdict path
 * of its own: both surfaces resolve the chord to one of these elements and call
 * .click() on it, so the key runs the same handler, the same optimistic latch
 * and the same focus settle the pointer does — and a disabled button (the
 * question card's unanswered "Send answers") refuses both equally.
 */
export const APPROVAL_KEY_ATTR = "data-approval-key";

export type ApprovalChordStep = "reveal" | "answer";

/**
 * REVEAL, THEN ANSWER. The chord is pane-scoped, so it reaches the keyed card
 * wherever that card happens to be — including hundreds of rows above the
 * reader, off the top of a paused transcript, with nothing on screen but an
 * amber capsule to say a request exists at all. Allowing a command you cannot
 * see is the one outcome this whole card is built to prevent, so the first
 * press spends itself bringing the card into view and answers nothing; the
 * second press, with the request on screen, answers it.
 *
 * PARTIAL VISIBILITY COUNTS. A card half off the bottom edge is a card the
 * reader is looking at, and making them press twice for it would teach them to
 * press twice always — which is how a reveal-then-answer rule turns back into
 * an answer-blind one.
 *
 * `top` and `bottom` are the card's edges measured from the scroller's own top
 * edge; `viewportHeight` is the scroller's client height.
 */
export function approvalChordStep(
  top: number,
  bottom: number,
  viewportHeight: number,
): ApprovalChordStep {
  return bottom > 0 && top < viewportHeight ? "answer" : "reveal";
}

export type ApprovalKeyVerdict = "allow" | "deny";

export interface ApprovalKeyEvent {
  key: string;
  metaKey?: boolean;
  ctrlKey?: boolean;
  altKey?: boolean;
  shiftKey?: boolean;
}

/** Anything a keystroke could be TEXT for. Shape, not instanceof: node tests. */
export function approvalKeyTyping(
  target: { tagName?: string; isContentEditable?: boolean } | null | undefined,
): boolean {
  if (target === null || target === undefined) return false;
  if (target.isContentEditable === true) return true;
  const tag = (target.tagName ?? "").toUpperCase();
  return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT";
}

/**
 * The pure rule both surfaces share. `typing` is the guard's answer, passed in
 * rather than sniffed here so the refusal is testable without a DOM.
 */
export function approvalKeyVerdict(
  event: ApprovalKeyEvent,
  typing: boolean,
): ApprovalKeyVerdict | null {
  if (typing) return null;
  if (event.altKey === true || event.shiftKey === true) return null;
  if (event.metaKey !== true && event.ctrlKey !== true) return null;
  if (event.key === "Enter") return "allow";
  if (event.key === "Backspace") return "deny";
  return null;
}
