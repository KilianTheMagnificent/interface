import { parseAgentQuestions, questionSummary } from "../shared/questions";
import type { EngineEvent, SessionStatus } from "../shared/engine";
// One line of activity, at most, clipped to the same width the transcript's own
// activity lines use — see conversation-reducer.ts's `clip`.
import { clip as clipLine } from "./conversation-reducer";

/**
 * Per-session derivations off the event log: the turn clock and progress, the
 * live activity line, the pending approval, and the one "blocked on you"
 * predicate the attention tray and the dock badge share.
 */

/* ---- per-session derivations off the event log ----------------------------
   What each agent is DOING and what it is blocked on lives in the per-session
   event log, so the derivation is here: pure, and testable in a repo with no
   DOM environment. */

// Enough characters to fill the two clamped lines on a card many times over,
// with room for an answer that opens with blank lines. Without it, a session
// whose last turn is a 40 KB answer concatenates the whole thing to throw all
// but 80 characters away, on every render.
const PROSE_SCAN_MAX = 2_000;

/**
 * What this agent is doing right now, in one line.
 *
 * Read BACKWARDS, and prose beats tools when it is newer: an agent that ran a
 * command and is now explaining itself is doing the explaining. Within one
 * turn the newest tool summary is the answer, because "Bash: npm test" says
 * more about a working agent than the sentence it wrote before starting.
 *
 * "" means the log has nothing to say — a session this app run has not streamed
 * (every resumed row) or one that has only thought so far. The card prints
 * "Working…" in that case rather than an empty hole, which is the component's
 * job and not this function's.
 *
 * Deliberately NOT conversation-reducer.ts's `streamActivity`: that one is private to
 * the subagent reducer, sums EVERY delta in the log (the whole transcript for a
 * long session) and clips the result from the FRONT, so it reports the agent's
 * FIRST words. On a session-length log that is the wrong end.
 */
export interface TurnProgress {
  active: boolean;
  /** Empty when idle. "Thinking", a live tool summary, or "Working". */
  label: string;
}

/** Whole seconds, then minutes/hours — the same clock Claude Code and Codex print. */
export function formatTurnElapsed(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  if (hours > 0) return minutes > 0 ? `${hours}h ${minutes}m` : `${hours}h`;
  if (minutes > 0) return seconds > 0 ? `${minutes}m ${seconds}s` : `${minutes}m`;
  return `${seconds}s`;
}

/** Ignore a flicker that never became a real turn. */
export const TURN_CLOCK_MIN_MS = 500;

/** Keep one start time per session so remounting a pane does not reset the clock. */
export function rememberTurnStart(
  clocks: Readonly<Record<string, number>>,
  key: string,
  running: boolean,
  now: number,
): Record<string, number> {
  const existing = clocks[key];
  if (running) {
    if (existing !== undefined) return clocks as Record<string, number>;
    return { ...clocks, [key]: now };
  }
  if (existing === undefined) return clocks as Record<string, number>;
  const next = { ...clocks };
  delete next[key];
  return next;
}

/** Attach a finished prompt's duration to the last transcript row of that turn. */
export function stampTurnClock(startedAt: number, endedAt: number,
    lastRowKey: string | null): { key: string; ms: number } | null {
  if (lastRowKey === null) return null;
  const ms = endedAt - startedAt;
  if (ms < TURN_CLOCK_MIN_MS) return null;
  return { key: lastRowKey, ms };
}

/**
 * What the composer status strip should say right now. Claude Code and Codex
 * keep a turn visible through tools, not only while a thinking pulse is the
 * last event — `thinking.signal` drops as soon as the first tool.call lands.
 */
export function turnProgress(events: readonly EngineEvent[],
    status: SessionStatus, live: boolean): TurnProgress {
  let thinking = false;
  let lastStatus = status;
  const open = new Map<string, string>();
  for (const evt of events) {
    if (evt.kind === "session.closed") {
      thinking = false;
      lastStatus = "idle";
      open.clear();
    } else if (evt.kind === "thinking.signal") thinking = evt.active;
    else if (evt.kind === "error" && !evt.retryable) {
      thinking = false;
      lastStatus = "failed";
      open.clear();
    } else if (evt.kind === "status.changed") {
      lastStatus = evt.status;
      if (evt.status === "idle" || evt.status === "failed") {
        thinking = false;
        open.clear();
      }
    } else if (evt.kind === "tool.call") open.set(evt.callId, evt.summary);
    else if (evt.kind === "tool.result") open.delete(evt.callId);
  }
  const waiting = lastStatus === "waiting";
  const running = lastStatus === "working" || thinking || open.size > 0;
  const active = live && (waiting || running);
  if (!active) return { active: false, label: "" };
  if (waiting) return { active: true, label: "Waiting for approval" };
  if (open.size > 0) {
    const last = [...open.values()].at(-1) ?? "";
    return { active: true, label: last === "" ? "Working" : last };
  }
  if (thinking) return { active: true, label: "Thinking" };
  return { active: true, label: "Working" };
}

export function sessionActivity(events: readonly EngineEvent[]): string {
  let prose = "";
  for (let i = events.length - 1; i >= 0; i--) {
    const evt = events[i];
    if (evt.kind === "message.delta") {
      prose = evt.text + prose;
      // The store coalesces a streamed answer into one delta, so this is the
      // only return that can see more than PROSE_SCAN_MAX characters; hand
      // the clipper a prefix rather than regex-scanning the whole answer.
      if (prose.length >= PROSE_SCAN_MAX) return clipLine(prose.slice(0, PROSE_SCAN_MAX));
      continue;
    }
    // A boundary. Whatever prose has been collected since it belongs to a later
    // turn than this event, so it wins.
    if (prose.trim() !== "") return clipLine(prose);
    if ((evt.kind === "tool.call" || evt.kind === "tool.result") && evt.summary !== "") {
      return clipLine(evt.summary);
    }
  }
  return clipLine(prose);
}

/**
 * The request holding this agent up: the newest `approval.requested` in its log.
 *
 * No "already answered" bookkeeping HERE on purpose — the event log does not
 * record what this window sent. Whether a verdict has gone out comes from the
 * caller's ApprovalVerdicts map, and a session that has answered is moved out
 * of "waiting" by the engine's next event — the same signal the sidebar dot
 * already uses.
 */
export function pendingApproval(events: readonly EngineEvent[]):
    { id: string; tool: string; summary: string; asksQuestion?: boolean } | undefined {
  for (let i = events.length - 1; i >= 0; i--) {
    const evt = events[i];
    if (evt.kind !== "approval.requested") continue;
    const questions = parseAgentQuestions(evt.tool, evt.input);
    return {
      id: evt.approvalId, tool: evt.tool,
      summary: questions !== undefined ? questionSummary(questions) : evt.summary,
      ...(questions !== undefined ? { asksQuestion: true } : {}),
    };
  }
  return undefined;
}

/* ---- a live thread's step, ask and failure ---------------------------------
   The Threads inbox's second line (owner, 2026-10-07, "Inbox under load") says
   what a live thread is doing, what it is asking, or why it failed. These are
   the log's own facts, read in one forward pass; thread-inbox.ts chooses which
   one a row says, and thread-live.ts words it. */

/** A tool call or an approval, as much of it as thread-live.ts's words read. */
export interface LiveStepCall {
  readonly tool: string;
  readonly summary: string;
  readonly input: unknown;
}

export interface ThreadLiveFacts {
  /**
   * A turn is in flight by the log's own reading: the registry's status rule
   * (interface-contracts `next_status`: a delta or a call means working unless
   * an approval is parked), a thinking pulse, or a call still open.
   */
  readonly running: boolean;
  /** The newest open call: the main thread's, else a subagent's while no main-thread call is open. */
  readonly step?: LiveStepCall;
  /**
   * The request it is parked on: the newest approval, until the status moves
   * on from waiting or the turn ends. `question` is a question approval's own
   * summary.
   */
  readonly ask?: LiveStepCall & { readonly question?: string };
  /** The newest failing error's first line, kept until a later turn starts working. */
  readonly error?: string;
}

type ToolCallEvent = Extract<EngineEvent, { kind: "tool.call" }>;
type ApprovalEvent = Extract<EngineEvent, { kind: "approval.requested" }>;

function stepCall(evt: ToolCallEvent | ApprovalEvent): LiveStepCall {
  return { tool: evt.tool, summary: evt.summary, input: evt.input };
}

/**
 * What a session's log says for a Threads row's second line. null for an
 * empty log: a session restored but never resumed, or one only a CLI runs,
 * has nothing live to say and keeps the row's project line.
 */
export function threadLiveFacts(events: readonly EngineEvent[]): ThreadLiveFacts | null {
  if (events.length === 0) return null;
  // The log's own status, by the registry's rule; not the listing's.
  let phase: SessionStatus = "idle";
  let thinking = false;
  const open = new Map<string, ToolCallEvent>();
  let ask: ApprovalEvent | undefined;
  let error: string | undefined;
  const settle = (next: SessionStatus): void => {
    phase = next;
    thinking = false;
    open.clear();
  };
  for (const evt of events) {
    switch (evt.kind) {
      case "session.closed":
        settle("idle");
        ask = undefined;
        break;
      case "approval.requested":
        phase = "waiting";
        ask = evt;
        break;
      case "status.changed":
        if (evt.status !== "waiting") ask = undefined;
        if (evt.status === "working") error = undefined;
        if (evt.status === "idle" || evt.status === "failed") settle(evt.status);
        else phase = evt.status;
        break;
      case "error":
        if (evt.retryable) break;
        settle("failed");
        ask = undefined;
        error = clipLine(evt.message.split(/\r?\n/u).find(line => line.trim() !== "") ?? "") || error;
        break;
      case "thinking.signal":
        thinking = evt.active;
        break;
      case "tool.call":
        open.set(evt.callId, evt);
        if (phase !== "waiting") phase = "working";
        break;
      case "tool.result":
        open.delete(evt.callId);
        if (phase !== "waiting") phase = "working";
        break;
      case "message.delta":
        if (phase !== "waiting") phase = "working";
        break;
      default:
        break;
    }
  }
  let main: ToolCallEvent | undefined;
  let child: ToolCallEvent | undefined;
  for (const call of open.values()) {
    if (call.agentKey === undefined) main = call;
    else child = call;
  }
  const step = main ?? child;
  const questions = ask === undefined ? undefined : parseAgentQuestions(ask.tool, ask.input);
  return {
    running: phase === "working" || thinking || open.size > 0,
    ...(step === undefined ? {} : { step: stepCall(step) }),
    ...(ask === undefined ? {} : {
      ask: { ...stepCall(ask), ...(questions === undefined ? {} : { question: questionSummary(questions) }) },
    }),
    ...(error === undefined ? {} : { error }),
  };
}

function sameCall(a: LiveStepCall | undefined, b: LiveStepCall | undefined): boolean {
  return a === b || (a !== undefined && b !== undefined && a.tool === b.tool
    && a.summary === b.summary && Object.is(a.input, b.input));
}

/** Whether two readings say the same thing, so a row keeps last frame's facts by identity. */
export function sameThreadLiveFacts(a: ThreadLiveFacts, b: ThreadLiveFacts): boolean {
  return a.running === b.running && a.error === b.error && sameCall(a.step, b.step)
    && sameCall(a.ask, b.ask) && a.ask?.question === b.ask?.question;
}

/* ---- the one "blocked on you" derivation -----------------------------------
   The most expensive state in the app is an agent waiting on the user who does
   not know it is waiting, and until now the only signal that survived leaving
   the window is the macOS dock badge.

   THERE IS EXACTLY ONE PREDICATE BELOW, and everything that counts, lists,
   announces or navigates to "blocked on you" goes through it: the toolbar pill,
   the toolbar's live region, and — by the parity test in viewmodels.test.ts —
   the dock badge in src/main/badge.ts. The defect this shape exists to make
   impossible is two counters with different definitions visible in the same
   window at the same instant. A number that can be derived twice will
   eventually be derived twice differently. */

/**
 * THE attention predicate. Every count of "blocked on you" in this module goes
 * through it — the pill's and each project group's own `counts.waiting` — so
 * `status === "waiting"` is written down exactly once.
 *
 * Deliberately the same rule as the dock badge's `waitingCount`
 * (src/main/badge.ts), which cannot import this module (it is main-process
 * code). `viewmodels.test.ts` runs both over the same listings and asserts they
 * agree, so changing either one alone turns the suite red — that assertion is
 * what makes "one fact, two renderings" enforceable rather than aspirational.
 *
 * Structural, not nominal: it takes anything carrying a `status`, so a row, a
 * card or a summary are all judged by the same line of code.
 */
export function needsAttention(session: { status: SessionStatus }): boolean {
  return session.status === "waiting";
}

/**
 * Every session blocked on the user, in listing order.
 *
 * Callers take `.length` for the count and the array itself for navigation, so
 * the number the pill prints and the session it opens are the SAME selection
 * rather than two filters that happen to agree today.
 *
 * Pass the registry LISTING, not `SidebarView.visible`. The two differ in
 * exactly one state — while the sidebar's search box has text in it, which is
 * the one filter that drops rows the user has not archived — and in that state
 * under-reporting is the dangerous direction: a search must never quieten an
 * alarm about an agent it happens not to match.
 */
export function attentionSessions<T extends { status: SessionStatus }>(
    sessions: readonly T[]): T[] {
  return sessions.filter(needsAttention);
}

/**
 * What a screen reader hears when the count changes, from the toolbar's live
 * region. Spelled out in a sentence: "3 need you" read aloud with no
 * surrounding context does not say what needs anything, and the zero case has
 * to be announceable — going back to nothing blocked is the clearing of an
 * alarm, not the absence of one.
 */
export function attentionAnnouncement(count: number): string {
  if (count === 0) return "No agents need you";
  return count === 1 ? "1 agent needs you" : `${count} agents need you`;
}
