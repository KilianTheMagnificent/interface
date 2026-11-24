/**
 * AN AWAY LOOP'S THREAD (review B3, protocol 45). Rust runs each loop as a
 * Threads thread and joins the loop's projection to the thread's view
 * (`ThreadView.away`). This module turns that projection into what the
 * thread's transcript draws: each work round's turn, the fresh review of that
 * round under it, and the loop's report at the end. Rust decided every
 * verdict; nothing here reads a provider's text.
 */
import type { AwayState, AwayVerdict, ThreadAway, ThreadAwayRound } from "../shared/away";
import type { ThreadTurn } from "../shared/threads";
import { providerLabel } from "./provider-labels";
import { AWAY_STATE_LABELS } from "./away-view";

/** One work round's turn and the review of that round. */
export interface AwayTurnCard {
  iteration: number;
  maxIterations: number;
  work: ThreadAwayRound;
  /** Absent until the round's review starts. */
  review?: ThreadAwayRound;
}

/**
 * Each thread turn a work round ran as, by the turn's ordinal. Rust names a
 * round's turn once the round has its answer; the round still working is the
 * thread's running turn, which no finished round has claimed.
 */
export function awayTurnCards(away: ThreadAway | undefined,
    turns: readonly Pick<ThreadTurn, "ordinal" | "state">[] = []): ReadonlyMap<number, AwayTurnCard> {
  const cards = new Map<number, AwayTurnCard>();
  if (away === undefined) return cards;
  const claimed = new Set(away.rounds.flatMap(round => round.turn === undefined ? [] : [round.turn]));
  const running = turns.filter(turn => turn.state === "running" && !claimed.has(turn.ordinal)).at(-1);
  for (const round of away.rounds) {
    const work = round.role === "work" && round.turn === undefined && round.state === "started"
      && running !== undefined ? { ...round, turn: running.ordinal } : round;
    if (work.role !== "work" || work.turn === undefined) continue;
    const review = away.rounds.find(round => round.role === "review"
      && round.iteration === work.iteration);
    cards.set(work.turn, {
      iteration: work.iteration, maxIterations: away.maxIterations, work,
      ...(review === undefined ? {} : { review }),
    });
  }
  return cards;
}

/** The provider whose sessions review the loop's rounds (protocol 47; the worker's before it). */
export function awayReviewerOf(away: Pick<ThreadAway, "provider" | "reviewer">): string {
  return away.reviewer ?? away.provider;
}

/** A verdict as the cards say it. */
export const AWAY_VERDICT_LABELS: Readonly<Record<AwayVerdict, string>> = {
  continue: "Approved, more to do",
  complete: "Approved, done",
  revise: "Changes requested",
  blocked: "Blocked",
};

/** What a review card shows. */
export interface AwayReviewView {
  /** "Review · Codex". */
  title: string;
  /** The verdict's words, or where the review stands without one. */
  verdict: string;
  tone: "ok" | "revise" | "blocked" | "pending";
  summary: string;
  checks: readonly string[];
  sessionKey?: string;
  /** Reviews only read: they never change a file. */
  note: string;
}

/** The review card under a work round's turn. */
export function awayReviewView(review: ThreadAwayRound, provider: string): AwayReviewView {
  const verdict = review.verdict;
  const pending = review.state === "started";
  return {
    title: `Review · ${providerLabel(provider)}`,
    verdict: verdict !== undefined ? AWAY_VERDICT_LABELS[verdict]
      : pending ? "Reviewing…"
      : review.state === "cancelled" ? "Stopped before a verdict"
      : "No verdict",
    tone: verdict === "revise" ? "revise"
      : verdict === "blocked" || (!pending && verdict === undefined) ? "blocked"
      : pending ? "pending" : "ok",
    summary: review.summary ?? "",
    checks: review.checks ?? [],
    ...(review.sessionKey === undefined ? {} : { sessionKey: review.sessionKey }),
    note: "A fresh session read the changes; reviews change no files.",
  };
}

/** The line a work round's prompt row shows instead of its instructions. */
export function awayRoundLine(card: Pick<AwayTurnCard, "iteration" | "maxIterations">): string {
  return `Work round ${card.iteration} of ${card.maxIterations}`;
}

/** The loop's card at the end of its thread. */
export interface AwayReportView {
  runId: string;
  live: boolean;
  state: AwayState;
  title: string;
  detail: string;
  objective: string;
  /** "2 rounds reviewed · Codex · ends 9:40 PM". */
  meta: string;
  /** The last verdict a review gave, when one did. */
  verdict?: { label: string; summary: string; checks: readonly string[] };
}

const clock = (at: number): string =>
  new Date(at).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });

/** The report (or, while the loop runs, its status) at the end of the thread. */
export function awayReport(away: ThreadAway | undefined): AwayReportView | null {
  if (away === undefined) return null;
  const live = away.state === "running" || away.state === "stopping";
  const reviewed = away.rounds.filter(round => round.role === "review" && round.verdict !== undefined);
  const last = reviewed.at(-1);
  const rounds = `${reviewed.length} ${reviewed.length === 1 ? "round" : "rounds"} reviewed`;
  const model = away.model === undefined ? providerLabel(away.provider)
    : `${providerLabel(away.provider)} · ${away.model}`;
  const reviewer = awayReviewerOf(away);
  const worker = reviewer === away.provider ? model : `${model}, reviewed by ${providerLabel(reviewer)}`;
  const title = live ? (away.state === "stopping" ? "Away is stopping" : "Away is working")
    : `Away · ${AWAY_STATE_LABELS[away.state]}`;
  return {
    runId: away.runId, live, state: away.state, title, detail: away.detail,
    objective: away.objective,
    meta: [rounds, worker, live ? `ends ${clock(away.deadlineAt)}` : `started ${clock(away.startedAt)}`]
      .join(" · "),
    ...(last?.verdict === undefined ? {} : { verdict: {
      label: AWAY_VERDICT_LABELS[last.verdict], summary: last.summary ?? "", checks: last.checks ?? [],
    } }),
  };
}

/** What the owner asks of an Away thread's cards. */
export type AwayCardAction =
  | { kind: "stop"; runId: string }
  | { kind: "session"; key: string }
  | { kind: "review" };
