import type { EngineEvent } from "../shared/engine";
import { sessionDiffStat } from "./diff-stats";

/**
 * WHAT HAPPENED WHILE YOU WERE LOOKING SOMEWHERE ELSE, in one line.
 *
 * A briefing you can read in the time it takes to put your hands back on the
 * keyboard. It costs no model call and asks the main process for nothing: every
 * clause is a difference between two readings of state this window already
 * held, taken at the moment attention left the pane and at the moment it came
 * back.
 *
 * The rule the whole module answers to is that it may only say what the event
 * log PROVES. A clause with no evidence behind it is dropped, not softened into
 * a dash or a zero: "+0 −0" over an agent that rewrote a file is a false
 * measurement, and "checks green" over a run nobody started is a false
 * reassurance. A recap that says less than it could is correct; a recap that
 * says one thing it cannot support is worthless, because after the first one
 * the user has to go and check anyway — which is the cost this line exists to
 * remove.
 */

/** How long attention must have been elsewhere before a return is worth a line. */
export const RECAP_MIN_AWAY_MS = 5 * 60_000;

/** Below this many completed turns a session has no history to miss. */
export const RECAP_MIN_TURNS = 3;

/** The line takes itself away; it is a briefing, not a notice to be managed. */
export const RECAP_AUTO_DISMISS_MS = 30_000;

/**
 * Everything one session's log can prove at a single instant. Two of these,
 * subtracted, are the whole recap.
 */
export interface RecapSnapshot {
  /** Turns that reached a terminal status, failed ones included. */
  readonly turns: number;
  /** The subset of `turns` that ended failed. */
  readonly failedTurns: number;
  readonly added: number;
  readonly removed: number;
  /**
   * False when this window has watched no diff at all. The counts are then 0
   * because nothing is known, NOT because nothing changed, and the two must
   * never render the same way (see ./diff-stats).
   */
  readonly diffKnown: boolean;
}

/** A finished Review check run, counted by outcome. */
export interface RecapChecks {
  readonly passed: number;
  readonly failed: number;
}

export interface RecapInput {
  readonly before: RecapSnapshot;
  readonly after: RecapSnapshot;
  readonly leftAt: number;
  readonly returnedAt: number;
  /** The session is still mid-turn at the moment of return. */
  readonly working: boolean;
  /** A permission request is open. Its words stay out of this line on purpose. */
  readonly approvalWaiting: boolean;
  /** Only a run that FINISHED during the absence; null for every other case. */
  readonly checks: RecapChecks | null;
}

export interface RecapLine {
  /** The clauses in reading order; `text` is these joined. */
  readonly parts: readonly string[];
  readonly text: string;
}

const LEAD = "While you were away";
const SEPARATOR = " · ";

/**
 * Read one session's log the way the shell itself reads it.
 *
 * A turn is counted exactly where App fires its turn-done notification: a
 * status that was `working` or `waiting` reaching `idle` or `failed`. Borrowing
 * that rule rather than inventing a second one is the point — a recap that
 * counted turns differently from the notification the user already got would be
 * two answers to one question.
 *
 * A log replayed from disk usually carries no status events, so it proves no
 * turns and the recap stays quiet over it. That is the honest reading: this
 * window did not watch those turns happen.
 */
export function recapSnapshot(events: readonly EngineEvent[]): RecapSnapshot {
  let turns = 0;
  let failedTurns = 0;
  let previous: string | undefined;
  for (const event of events) {
    if (event.kind !== "status.changed") continue;
    const ending = (previous === "working" || previous === "waiting")
      && (event.status === "idle" || event.status === "failed");
    if (ending) {
      turns += 1;
      if (event.status === "failed") failedTurns += 1;
    }
    previous = event.status;
  }
  const diff = sessionDiffStat(events);
  return {
    turns,
    failedTurns,
    added: diff?.added ?? 0,
    removed: diff?.removed ?? 0,
    diffKnown: diff !== null,
  };
}

/**
 * The clauses for one return, or null when there is nothing provable to say.
 *
 * Null is the common answer and deliberately so: a short absence, a young
 * session, or a quiet one all produce no line rather than a line saying
 * nothing happened.
 */
export function recapLine(input: RecapInput): RecapLine | null {
  const { after, before } = input;
  if (input.returnedAt - input.leftAt < RECAP_MIN_AWAY_MS) return null;
  if (after.turns < RECAP_MIN_TURNS) return null;

  // A snapshot that shrank is a log this window replaced, not an agent that
  // un-did work: a prepended history page or a reopened session. Nothing about
  // the absence can be read out of the pair, so nothing is claimed.
  if (after.turns < before.turns) return null;

  const failed = Math.max(0, after.failedTurns - before.failedTurns);
  const finished = Math.max(0, after.turns - before.turns - failed);
  const parts: string[] = [LEAD];

  if (finished > 0) parts.push(`${finished} ${plural(finished, "turn")} finished`);
  if (failed > 0) parts.push(`${failed} ${plural(failed, "turn")} failed`);

  // Both sides must be known at both ends. Subtracting a real total from an
  // unknown one would report the session's whole history as this absence's work.
  const lines = after.diffKnown && after.added >= before.added && after.removed >= before.removed
    ? diffClause(after.added - before.added, after.removed - before.removed)
    : "";
  if (lines !== "") parts.push(lines);

  const checks = input.checks;
  if (checks !== null && checks.failed > 0) {
    parts.push(`${checks.failed} ${plural(checks.failed, "check")} failed`);
  } else if (checks !== null && checks.passed > 0) {
    parts.push("checks passed");
  }

  if (input.approvalWaiting) parts.push("approval waiting");
  if (input.working) parts.push("still working");

  if (parts.length === 1) return null;
  return { parts, text: parts.join(SEPARATOR) };
}

/** `+62 −11`, either half dropped when it did not move; "" when neither did. */
function diffClause(added: number, removed: number): string {
  const halves: string[] = [];
  if (added > 0) halves.push(`+${added}`);
  if (removed > 0) halves.push(`−${removed}`);
  return halves.join(" ");
}

function plural(n: number, word: string): string {
  return n === 1 ? word : `${word}s`;
}
