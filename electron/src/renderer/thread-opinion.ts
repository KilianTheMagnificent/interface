import type { EngineEvent, EngineId } from "../shared/engine";
import type { CollaborationEngine, CollaborationStatusDto } from "../shared/ipc";
import type { HandoffDelivery, ThreadChain, ThreadLimit, ThreadSchedules, ThreadView } from "../shared/threads";
import { providerLabel } from "./provider-labels";
import { formatReset, threadUsageHeadroom, type UsageHeadroom } from "./usage-view";
import { accountGroupFor, DEFAULT_ACCOUNT_ID, type ProviderAccountsView } from "../shared/provider-accounts";
import { accountDisplayLabel } from "./provider-accounts-view";
import type { SteeringQueueItem } from "./steering-queue";

/**
 * Second opinion and Switch to in a thread (review B4; Switch to replaced
 * Continue with in protocol 59, T3 section 5), and the offer to switch when a
 * provider hits its limit (review E). Rust decides what is sent and refuses
 * what it must; this module only words what the collaboration channels and
 * the provider probes already said.
 */

/** What the provider probes say about one engine, for these offers. */
export interface ThreadProviderFacts {
  engine: string;
  /** The provider's own display name, when it gave one. */
  label?: string;
  /** Answering and signed in. */
  ready: boolean;
  /** It advertises supervised, read-only consultation (a consult target). */
  consult: boolean;
}

export interface ThreadProviderChoice<E extends string = string> {
  engine: E;
  label: string;
}

export interface ThreadProviderOffers {
  /** Second opinion from …: ready consult targets other than the thread's own. */
  opinion: readonly ThreadProviderChoice<CollaborationEngine>[];
  /** Switch to …: ready providers other than the thread's own (project threads only). */
  switchTo: readonly ThreadProviderChoice<EngineId>[];
}

const CONSULT_ENGINES: readonly CollaborationEngine[] = ["claude", "codex", "acp:grok"];

function isConsultEngine(engine: string): engine is CollaborationEngine {
  return (CONSULT_ENGINES as readonly string[]).includes(engine);
}

function isEngineId(engine: string): engine is EngineId {
  return engine === "claude" || engine === "codex" || /^acp:.+/u.test(engine);
}

export const NO_THREAD_OFFERS: ThreadProviderOffers = { opinion: [], switchTo: [] };

/**
 * Which providers a thread on `threadEngine` can ask for a second opinion and
 * switch to, in the probes' order. A plain chat ("No project") switches
 * nowhere: it has no folder to carry.
 */
export function threadProviderOffers(threadEngine: string, providers: readonly ThreadProviderFacts[],
    project: boolean): ThreadProviderOffers {
  const others = providers.filter(provider => provider.ready && provider.engine !== threadEngine);
  const opinion = others.flatMap(provider => isConsultEngine(provider.engine) && provider.consult
    ? [{ engine: provider.engine, label: providerLabel(provider.engine, provider.label) }] : []);
  const switchTo = project ? others.flatMap(provider => isEngineId(provider.engine)
    ? [{ engine: provider.engine, label: providerLabel(provider.engine, provider.label) }] : []) : [];
  return opinion.length === 0 && switchTo.length === 0
    ? NO_THREAD_OFFERS : { opinion, switchTo };
}

export type ThreadOpinionStage =
  /** Rust is building the packet. */
  | "preparing"
  /** The owner reviews what goes to the other provider: Ask or Cancel. */
  | "review"
  /** The other provider is reading. */
  | "running"
  /** Its reply is here: Send to the thread's provider, or Dismiss. */
  | "ready"
  /** The reply is on its way into the thread. */
  | "sending"
  /** It did not finish; the reason is said. */
  | "failed";

export interface ThreadOpinionCard {
  id: string;
  stage: ThreadOpinionStage;
  /** The provider giving the opinion ("Codex"), and its engine for the mark. */
  engine: string;
  label: string;
  /** The thread's own provider ("Claude"), which Send hands the reply to. */
  sourceLabel: string;
  /** The reply (ready / sending), as untrusted text. */
  body?: string;
  /** What goes out (review): its size and how much was left out. */
  packet?: { bytes: number; omissions: number; preview: string };
  /** Why it failed, in Rust's words. */
  error?: string;
  /** A cancel the coordinator says is safe now. */
  cancellable: boolean;
  status: CollaborationStatusDto;
}

/** A failed second opinion is shown this long after it last changed, then let go. */
export const FAILED_OPINION_SHOWN_MS = 60 * 60 * 1000;

function stageOf(status: CollaborationStatusDto): ThreadOpinionStage | null {
  switch (status.receipt.state) {
    case "requested": return "preparing";
    case "awaiting_approval": return status.approval === undefined ? "preparing" : "review";
    case "approved": case "preflight": case "preparing_target": case "running_target":
    case "result_ready":
      return "running";
    case "awaiting_peer_review": return status.peer === undefined ? "running" : "ready";
    case "delivery_pending": return "sending";
    case "failed": case "unknown": return "failed";
    // Sent on, discarded, denied or cancelled: the thread's transcript, or
    // nothing, is what is left to show.
    case "completed": case "denied": case "cancelled":
    case "parking_source": case "ownership_transfer": case "activating_target":
      return null;
  }
}

/**
 * The second opinions to draw in thread `key`, oldest first: every consult
 * whose source is the thread and that is still the owner's business (being
 * reviewed, running, a reply waiting, being sent), and a failed one for an
 * hour unless dismissed here.
 */
export function threadOpinionCards(statuses: readonly CollaborationStatusDto[], key: string,
    dismissed: ReadonlySet<string>, now: number): ThreadOpinionCard[] {
  const cards: ThreadOpinionCard[] = [];
  for (const status of statuses) {
    if (status.sourceSessionKey !== key || status.receipt.mode !== "consult") continue;
    const id = status.receipt.collaborationId;
    if (dismissed.has(id)) continue;
    const stage = stageOf(status);
    if (stage === null) continue;
    if (stage === "failed") {
      const at = Date.parse(status.receipt.updatedAt);
      if (Number.isNaN(at) || now - at > FAILED_OPINION_SHOWN_MS) continue;
    }
    const target = status.receipt.targetEngine;
    cards.push({
      id, stage, engine: target, label: providerLabel(target),
      sourceLabel: providerLabel(status.receipt.sourceEngine),
      ...(status.peer !== undefined && (stage === "ready" || stage === "sending")
        ? { body: status.peer.body } : {}),
      ...(stage === "review" && status.approval !== undefined ? { packet: {
        bytes: status.approval.packetBytes, omissions: status.approval.omissions,
        preview: status.approval.packetPreview,
      } } : {}),
      ...(stage === "failed" && status.receipt.error !== undefined
        ? { error: status.receipt.error.safeNextAction } : {}),
      cancellable: status.receipt.cancellationSafe,
      status,
    });
  }
  return cards.sort((left, right) =>
    left.status.receipt.createdAt.localeCompare(right.status.receipt.createdAt));
}

/** "9.4 KB", for what a second opinion sends. */
export function opinionPacketSize(bytes: number): string {
  return bytes < 1024 ? `${bytes} bytes` : `${(bytes / 1024).toFixed(1)} KB`;
}

/** The new thread's name: the thread's title and the account it continues on. */
export function continuationName(title: string, label: string): string {
  const head = [...title.replace(/\s+/gu, " ").trim()].slice(0, 80).join("").trimEnd();
  return head === "" ? `Continued on ${label}` : `${head} · ${label}`;
}

/**
 * One place a thread at its limit can go: another provider (Switch to, the
 * same thread) or, with `account`, another account of its own provider
 * (Continue with, a new thread on that account; protocol 60).
 */
export interface ThreadLimitChoice {
  engine: EngineId;
  /** "Codex", or "Claude (Work)" for another account of the same provider. */
  label: string;
  /** Its own headroom, when it reported one. */
  headroom?: string;
  /** Protocol 60: the account a continuation runs on; absent for a provider switch. */
  account?: string;
}

/** What a thread at its provider's limit offers: one provider to switch to, or another account. */
export interface ThreadLimitOffer extends ThreadLimitChoice {
  /** "Codex hit its usage limit; it resets in 2h 5m." */
  message: string;
  /** Protocol 60: the same provider's other accounts that have headroom, offered too. */
  also?: readonly ThreadLimitChoice[];
}

/** Whether the limit line offers another account (Continue with), so the
 *  account-terms note goes under it. `canContinue`: the pane draws that
 *  button at all. */
export function limitOffersAccount(limit: ThreadLimitOffer | null, canContinue: boolean): boolean {
  return canContinue && limit !== null
    && [limit, ...(limit.also ?? [])].some(choice => choice.account !== undefined);
}

/** Where a thread's own account stands, for the other-account offer. */
export interface ThreadLimitAccounts {
  view: ProviderAccountsView | null;
  /** The thread's account; absent = the inherited Default. */
  account?: string;
}

/** "38% left" → 38, "At limit" → -1, unknown → 0. */
function percentLeft(text: string): number {
  const match = /(\d+)% left/u.exec(text);
  return match === null ? (text === "At limit" ? -1 : 0) : Number(match[1]);
}

/**
 * The thread's provider's other accounts whose own usage row shows headroom
 * (a measured "N% left" above zero), most headroom first.
 */
export function otherAccountChoices(threadEngine: string, accounts: ThreadLimitAccounts,
    headrooms: Readonly<Record<string, UsageHeadroom>>): ThreadLimitChoice[] {
  const group = accountGroupFor(accounts.view, threadEngine);
  if (group === undefined || group.accounts.length <= 1) return [];
  const own = accounts.account ?? DEFAULT_ACCOUNT_ID;
  const provider = providerLabel(threadEngine);
  return group.accounts.flatMap(account => {
    if (account.id === own) return [];
    const headroom = threadUsageHeadroom(headrooms, threadEngine,
      account.id === DEFAULT_ACCOUNT_ID ? undefined : account.id)?.text;
    if (headroom === undefined || percentLeft(headroom) <= 0) return [];
    return [{ engine: group.engine, label: accountDisplayLabel(provider, account.label), headroom,
      account: account.id }];
  }).sort((left, right) => percentLeft(right.headroom ?? "") - percentLeft(left.headroom ?? ""));
}

/**
 * When the thread's provider reported that it hit its limit (`limit`, the
 * session's latest rate-limit notice), the provider to switch to: the one
 * with the most headroom among those offered, else the first offered. Null
 * when nothing was reported or there is nowhere to go. Never switches by
 * itself: the owner presses it. `resetsAt` (protocol 66, the thread's limit
 * as Rust recorded it) says when it resets; without it the notice's own
 * words are read.
 */
export function threadLimitOffer(limit: string | undefined, threadEngine: string,
    offers: ThreadProviderOffers,
    headrooms: Readonly<Record<string, UsageHeadroom>>, now: number,
    accounts?: ThreadLimitAccounts, resetsAt?: string): ThreadLimitOffer | null {
  if (limit === undefined || limit.trim() === "") return null;
  const leftOf = (engine: string): number => percentLeft(headrooms[engine]?.text ?? "");
  const best = [...offers.switchTo].sort((left, right) =>
    leftOf(right.engine) - leftOf(left.engine))[0];
  const provider: ThreadLimitChoice | null = best === undefined || leftOf(best.engine) < 0 ? null : {
    engine: best.engine, label: best.label,
    ...(headrooms[best.engine]?.text === undefined ? {} : { headroom: headrooms[best.engine]?.text }),
  };
  const others = accounts === undefined ? [] : otherAccountChoices(threadEngine, accounts, headrooms);
  const [primary, ...also] = provider === null ? others : [provider, ...others];
  if (primary === undefined) return null;
  return {
    message: limitMessage(threadEngine, resetsAt ?? noticeReset(limit), now),
    ...primary,
    ...(also.length === 0 ? {} : { also }),
  };
}

/**
 * Latest rate-limit notice in one session's event log, for the sidebar badge
 * and the limit line. Read from the event log rather than the registry on
 * purpose: a rate limit is the engine reporting a quota it is waiting on, not
 * a change of session state, so it never became a SessionSummary field and
 * never reorders a row (registry.test.ts, "rate-limit signals never change
 * status or order").
 */
export function latestRateLimitNotice(log: readonly EngineEvent[]): string | undefined {
  // The most recent usage.updated is the whole answer, including when it
  // carries no rateLimited: a later turn that reported no limit is the engine
  // saying the limit has lifted, so the badge clears.
  for (let i = log.length - 1; i >= 0; i--) {
    const evt = log[i];
    if (evt.kind !== "usage.updated") continue;
    return evt.rateLimited?.message;
  }
  return undefined;
}

/**
 * Which threads' limit lines may offer somewhere to go, keyed by thread: each
 * session's latest notice from its event log (`rateLimits`), and every thread
 * Rust still records at its limit (`ThreadView.limit`, protocol 66, cleared
 * once a run completes). Claude reports its limit and then ends the turn with
 * a result whose token usage names none, so its event log forgets the limit
 * at once; without Rust's record its line never offered Switch to. A record
 * whose reset has passed (`now`) no longer counts.
 */
export function threadLimitNotices(rateLimits: Readonly<Record<string, string>>,
    threads: readonly Pick<ThreadView, "key" | "limit">[], now: number): Record<string, string> {
  const notices: Record<string, string> = { ...rateLimits };
  for (const thread of threads) {
    if (thread.limit === undefined || notices[thread.key] !== undefined) continue;
    const resetsAt = thread.limit.resetsAt === undefined ? Number.NaN : Date.parse(thread.limit.resetsAt);
    if (Number.isFinite(resetsAt) && resetsAt <= now) continue;
    notices[thread.key] = "Rate limit reached";
  }
  return notices;
}

/** Codex's notice says when the window resets ("…; resets 2026-10-14T18:00:00Z"). */
function noticeReset(limit: string): string | null {
  return /resets\s+(\S+)/u.exec(limit)?.[1] ?? null;
}

/** "Codex hit its usage limit; it resets in 2h 5m." */
function limitMessage(threadEngine: string, resetsAt: string | null, now: number): string {
  const reset = formatReset(resetsAt, now);
  return `${providerLabel(threadEngine)} hit its usage limit${reset === "" ? "" : `; it ${reset}`}.`;
}

/**
 * Resume at reset and Snooze until reset (T3 section 10, protocol 66): what
 * the limit line offers once Rust knows when the thread's limit resets.
 */
export interface ThreadLimitReset {
  /** When the limit resets, ms since the epoch. */
  resetsAt: number;
  /** "Claude hit its usage limit; it resets in 2h 5m.", for a line that offers nowhere to switch. */
  message: string;
  /** Resume at reset: false while a resume for this reset is already set (the schedule line shows it). */
  resume: boolean;
  /** Snooze until reset: false once the thread is snoozed until then. */
  snooze: boolean;
}

/**
 * The thread's Resume at reset and Snooze until reset, from its limit and
 * its schedules. Null when its provider reported no limit, or did not say
 * when it resets (never a guessed time), or the reset has passed.
 */
export function threadLimitReset(limit: ThreadLimit | undefined, threadEngine: string,
    schedules: ThreadSchedules | undefined, snoozedUntil: number | undefined,
    now: number): ThreadLimitReset | null {
  const resetsAt = limit?.resetsAt === undefined ? Number.NaN : Date.parse(limit.resetsAt);
  if (!Number.isFinite(resetsAt) || resetsAt <= now) return null;
  const resumeSet = (schedules?.schedules ?? []).some(schedule => schedule.kind === "resume"
    && schedule.enabled && (schedule.waiting !== undefined || (schedule.nextAt ?? 0) >= resetsAt));
  return {
    resetsAt,
    message: limitMessage(threadEngine, limit?.resetsAt ?? null, now),
    resume: !resumeSet,
    snooze: snoozedUntil === undefined || snoozedUntil < resetsAt,
  };
}

/**
 * The owner's queued follow-up a Resume at reset carries (protocol 66): the
 * queue's first row, when it never left (not in flight: its delivery would be
 * unknown) and carries no attachments (those are checked again only when the
 * owner sends). Null otherwise; Rust then picks the prompt. Never skips past
 * the first row, so the queue's order holds.
 */
export function resumeQueuedPrompt(queue: readonly SteeringQueueItem[]): { id: string; text: string } | null {
  const first = queue[0];
  if (first === undefined || first.deliveryState === "inflight" || (first.context?.length ?? 0) > 0) return null;
  if (first.text.trim() === "" || new TextEncoder().encode(first.text).length > 16 * 1024) return null;
  return { id: first.id, text: first.text };
}

/** Why Second opinion and Switch to wait: they run between turns. Null when they may run. */
export function threadBridgeWait(status: string): string | null {
  return status === "working" || status === "waiting"
    ? "Let the turn finish or stop it first." : null;
}

/**
 * A switched thread (protocol 59): the provider it ran on before its current
 * one, for the pane head's "Switched from …". Null for a thread that was never
 * switched.
 */
export function threadSwitchedFrom(chain: ThreadChain | undefined): string | null {
  const segments = chain?.segments ?? [];
  const previous = segments.at(-2);
  return segments.length < 2 || previous === undefined ? null : providerLabel(previous.engine);
}

/**
 * A switch's handoff prompt (Rust's `switch_prompt`), which the transcript
 * folds into one line; null for any other prompt. A handoff to a new session
 * names its provider; one that switches back names the provider in brackets.
 */
export function threadHandoffLine(text: string): string | null {
  const taken = /^You are taking over a thread from .+? in Interface; the owner switched it to (.+?)\.\n/u
    .exec(text);
  if (taken !== null) return `Handed the thread to ${taken[1]}`;
  const back = /^The owner switched this thread back to you \((.+?)\) in Interface\./u.exec(text);
  return back === null ? null : `Handed the thread back to ${back[1]}`;
}

/** What a switch's answer says about its handoff, for the owner; null when it arrived. */
export function threadHandoffNotice(delivery: HandoffDelivery | undefined, label: string): string | null {
  switch (delivery) {
    case "unknown":
      return `The thread is on ${label} now, but its handoff may not have arrived. Type to carry on.`;
    case "unsent":
      return `The thread is on ${label} now, but its handoff was not sent. Type to carry on.`;
    case "sent": case undefined:
      return null;
  }
}
