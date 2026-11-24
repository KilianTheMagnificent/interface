import { memo } from "react";
import type { ReactNode } from "react";
import { approvalVerdictKey, type ApprovalVerdicts } from "../approval-keys";
import type { ThreadApproval } from "../thread-frame";
import type { ConversationProps } from "./Conversation";
import {
  ApprovalCard, CheckingApprovalCard, QuestionCard, type ApprovalDock,
} from "./ConversationRows";

/**
 * THE DOCK (owner, 2026-10-07, "a live turn"): every request a Threads pane
 * holds open, drawn directly above its composer instead of in the transcript,
 * so what blocks the turn sits where the owner's hands already are. The cards
 * are the transcript's own (ApprovalCard, QuestionCard, CheckingApprovalCard)
 * in their docked form: the same verdicts, handlers, keys, alert role and
 * voice guard. The chord reaches them exactly as it reaches a transcript card
 * — the pane's listener clicks the last keyed button under the pane, and a
 * card here is last in the document — and an empty composer's ⌘↵ / ⌘⌫ click
 * the same buttons.
 */

/**
 * When the dock first drew each request, for its waiting clock. Keyed by the
 * item itself, which the transcript keeps by identity while it is unchanged,
 * so a pane that remounts (a workspace switch) keeps the count it had.
 */
const dockedAt = new WeakMap<object, number>();
function dockedSince(item: ThreadApproval): number {
  const seen = dockedAt.get(item);
  if (seen !== undefined) return seen;
  const now = Date.now();
  dockedAt.set(item, now);
  return now;
}

export interface ThreadApprovalDockProps {
  /** The pane's open requests in transcript order (threadDockApprovals). */
  items: readonly ThreadApproval[];
  live: boolean;
  activeApprovalIds: ReadonlySet<string>;
  /** Requests Interface's approval policy is still deciding: drawn, not answerable. */
  checkingApprovalIds: ReadonlySet<string>;
  approvals: ApprovalVerdicts;
  sessionKey: string;
  /** The one request whose buttons carry the chord; null when this pane owns none. */
  keyedApprovalId: string | null;
  engineLabel?: string;
  pathRoot?: string;
  projectLabel?: string;
  onApprove: ConversationProps["onApprove"];
  onDeny: ConversationProps["onDeny"];
}

export const ThreadApprovalDock = memo(function ThreadApprovalDock(
  props: ThreadApprovalDockProps,
): ReactNode {
  if (props.items.length === 0) return null;
  return (
    <div className="thread-dock">
      {props.items.map(item => {
        const answer = props.approvals[approvalVerdictKey(props.sessionKey, item.id)];
        const dock: ApprovalDock = {
          pathRoot: props.pathRoot, projectLabel: props.projectLabel, since: dockedSince(item),
        };
        // A verdict already sent keeps its card, as in the transcript: the
        // host drops an answered request from the owner's list, which must not
        // read as "checking".
        if (props.live && props.checkingApprovalIds.has(item.id)
            && (answer === undefined || answer.error !== undefined) && item.resolved === undefined) {
          return <CheckingApprovalCard key={item.id} item={item} dock={dock} />;
        }
        const active = props.live && props.activeApprovalIds.has(item.id);
        const keyed = props.keyedApprovalId === item.id;
        return item.questions !== undefined
          ? <QuestionCard key={item.id} item={item} onApprove={props.onApprove}
              onDeny={props.onDeny} keyed={keyed} active={active} answer={answer} dock={dock} />
          : <ApprovalCard key={item.id} item={item} onApprove={props.onApprove}
              onDeny={props.onDeny} keyed={keyed} engineLabel={props.engineLabel}
              threadLayout active={active} answer={answer} dock={dock} />;
      })}
    </div>
  );
});
