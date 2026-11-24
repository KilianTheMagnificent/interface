import "./ThreadOpinion.css";
import { Fragment, memo } from "react";
import { Markdown } from "../markdown";
import { equalValue } from "../stable-values";
import { ACCOUNT_TERMS_NOTE } from "../provider-accounts-view";
import type { FolderTrustRow } from "../folder-trust";
import {
  limitOffersAccount, opinionPacketSize, type ThreadLimitChoice, type ThreadLimitOffer, type ThreadLimitReset,
  type ThreadOpinionCard,
} from "../thread-opinion";
import { ProviderLogo } from "./ProviderLogo";

/**
 * Above a Threads pane's composer (review B4 and E): each second opinion this
 * thread asked for, and the offer to switch the thread to another provider
 * when its own hit its limit. A second opinion is another provider's words,
 * never a turn of this thread's: it says whose they are, and they reach the
 * thread only when the owner sends them on.
 */
export type ThreadOpinionAction = "ask" | "decline" | "cancel" | "send" | "discard" | "dismiss";

export interface ThreadOpinionCardsProps {
  cards: readonly ThreadOpinionCard[];
  limit: ThreadLimitOffer | null;
  /** Cards with an action on its way: their buttons wait. */
  pending: readonly string[];
  onAction: (card: ThreadOpinionCard, action: ThreadOpinionAction) => void;
  /** Switch to the offered provider (protocol 59): the same thread carries on there. */
  onSwitch: (offer: ThreadLimitChoice) => void;
  /**
   * Continue with another account of the thread's own provider (protocol 60):
   * a new thread on that account. Absent: no account is offered.
   */
  onContinue?: (offer: ThreadLimitChoice) => void;
  /**
   * Protocol 66: the limit's known reset, for Resume at reset and Snooze
   * until reset beside the other offers (the line shows with these alone
   * when there is nowhere to switch). Absent or null: neither is offered.
   */
  reset?: ThreadLimitReset | null;
  /** Resume at reset: the thread carries on by itself a minute after the reset. */
  onResume?: () => void;
  /** Snooze until reset: the thread snoozes until `until` (ms). */
  onSnooze?: (until: number) => void;
  /**
   * Protocol 72: the thread's folder is not trusted, so its own settings
   * and instructions are off. Absent or null: no line.
   */
  folderTrust?: FolderTrustRow | null;
  /** Trust folder, from that line. */
  onTrustFolder?: () => void;
}

function stageLine(card: ThreadOpinionCard): string {
  switch (card.stage) {
    case "preparing": return `Preparing what goes to ${card.label}…`;
    case "review": return card.packet === undefined ? ""
      : `${card.label} reads the latest answer, that turn's changes and the recent conversation `
        + `(${opinionPacketSize(card.packet.bytes)}${card.packet.omissions > 0
          ? ", older parts left out" : ""}). It can't change files.`;
    case "running": return `${card.label} is reading the thread…`;
    case "ready": return `Not sent to ${card.sourceLabel}`;
    case "sending": return `Sending to ${card.sourceLabel}…`;
    case "failed": return card.error ?? "The second opinion did not finish.";
  }
}

function OpinionCard({ card, pending, onAction }: {
  card: ThreadOpinionCard;
  pending: boolean;
  onAction: ThreadOpinionCardsProps["onAction"];
}) {
  const act = (action: ThreadOpinionAction) => () => { onAction(card, action); };
  const line = stageLine(card);
  return (
    <article className={`thread-opinion is-${card.stage}`}
      aria-label={`Second opinion from ${card.label}`} aria-busy={pending || undefined}>
      <header className="thread-opinion-head">
        <ProviderLogo engine={card.engine} label={card.label} size={14} />
        <span className="thread-opinion-title">Second opinion from {card.label}</span>
        {(card.stage === "ready" || card.stage === "sending") && (
          <span className="thread-opinion-state">{line}</span>
        )}
      </header>
      {card.body !== undefined && (
        <div className="thread-opinion-body"><Markdown text={card.body} /></div>
      )}
      {card.stage !== "ready" && card.stage !== "sending" && line !== "" && (
        <p className={card.stage === "failed" ? "thread-opinion-line is-error" : "thread-opinion-line"}
          role={card.stage === "failed" ? "alert" : undefined}>
          {line}
        </p>
      )}
      {card.stage === "review" && card.packet !== undefined && (
        <details className="thread-opinion-packet">
          <summary>What is sent</summary>
          <pre>{card.packet.preview}</pre>
        </details>
      )}
      <div className="thread-opinion-actions">
        {card.stage === "review" && <>
          <button type="button" className="thread-ship-act" disabled={pending} onClick={act("ask")}>
            Ask {card.label}
          </button>
          <button type="button" className="thread-act" disabled={pending} onClick={act("decline")}>
            Cancel
          </button>
        </>}
        {(card.stage === "running" || card.stage === "preparing") && card.cancellable && (
          <button type="button" className="thread-act" disabled={pending} onClick={act("cancel")}>
            Cancel
          </button>
        )}
        {card.stage === "ready" && <>
          <button type="button" className="thread-ship-act" disabled={pending} onClick={act("send")}
            title={`${card.sourceLabel} gets this as a message marked as another model's advice`}>
            Send to {card.sourceLabel}
          </button>
          <button type="button" className="thread-act" disabled={pending} onClick={act("discard")}>
            Dismiss
          </button>
        </>}
        {card.stage === "failed" && (
          <button type="button" className="thread-act" onClick={act("dismiss")}>Dismiss</button>
        )}
      </div>
    </article>
  );
}

export const ThreadOpinionCards = memo(function ThreadOpinionCards({ cards, limit, pending,
  onAction, onSwitch, onContinue, reset = null, onResume, onSnooze, folderTrust = null,
  onTrustFolder }: ThreadOpinionCardsProps) {
  if (cards.length === 0 && limit === null && reset === null && folderTrust === null) return null;
  return (
    <div className="thread-opinions">
      {folderTrust !== null && (
        <div className="thread-limit" role="status">
          <span className="thread-limit-text">{folderTrust.text}</span>
          {onTrustFolder !== undefined && (
            <button type="button" className="thread-ship-act thread-limit-continue"
              onClick={onTrustFolder} title={folderTrust.folder}>
              {folderTrust.action}
            </button>
          )}
        </div>
      )}
      {(limit !== null || reset !== null) && (
        <div className="thread-limit" role="status">
          <span className="thread-limit-text">{limit?.message ?? reset?.message}</span>
          {(limit === null ? [] : [limit, ...(limit.also ?? [])]).map(choice => <Fragment key={choice.account ?? choice.engine}>
            {choice.account === undefined ? (
              <button type="button" className="thread-ship-act thread-limit-continue"
                onClick={() => { onSwitch(choice); }}
                title={`Carry on in this thread on ${choice.label}. It is told where things stand first.`}>
                Switch to {choice.label}
              </button>
            ) : onContinue !== undefined && (
              <button type="button" className="thread-ship-act thread-limit-continue"
                onClick={() => { onContinue(choice); }}
                title={`A new thread on ${choice.label} in this folder, carrying this conversation and its changes`}>
                Continue with {choice.label}
              </button>
            )}
            {(choice.account === undefined || onContinue !== undefined) && choice.headroom !== undefined && (
              <span className="thread-limit-headroom">{choice.headroom}</span>
            )}
          </Fragment>)}
          {reset?.resume === true && onResume !== undefined && (
            <button type="button" className="thread-act" onClick={onResume}
              title="This thread carries on by itself a minute after the limit resets. Cancel it on the line above.">
              Resume at reset
            </button>
          )}
          {reset?.snooze === true && onSnooze !== undefined && (
            <button type="button" className="thread-act" onClick={() => { onSnooze(reset.resetsAt); }}
              title="Snooze this thread until the limit resets">
              Snooze until reset
            </button>
          )}
        </div>
      )}
      {/* Under the limit line, not in it, so its buttons keep their room. */}
      {limitOffersAccount(limit, onContinue !== undefined) && (
        <p className="thread-opinion-line">{ACCOUNT_TERMS_NOTE}</p>
      )}
      {cards.map(card => (
        <OpinionCard key={card.id} card={card} pending={pending.includes(card.id)}
          onAction={onAction} />
      ))}
    </div>
  );
}, (previous, next) => previous.onAction === next.onAction && previous.onSwitch === next.onSwitch
  && previous.onContinue === next.onContinue
  && previous.onResume === next.onResume && previous.onSnooze === next.onSnooze
  && previous.onTrustFolder === next.onTrustFolder && equalValue(previous.folderTrust, next.folderTrust)
  && equalValue(previous.reset, next.reset)
  && equalValue(previous.pending, next.pending) && equalValue(previous.limit, next.limit)
  && previous.cards.length === next.cards.length
  && previous.cards.every((card, index) => card.stage === next.cards[index]?.stage
    && card.status === next.cards[index]?.status));
