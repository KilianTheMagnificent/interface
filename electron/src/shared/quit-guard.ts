/**
 * The quit guard: what is still working when the owner quits.
 *
 * The renderer is where pane activity, thread status, Away loops and teammate
 * runs meet, so it reports a count to main whenever that changes. Main asks
 * before a quit that would end them (the way Terminal and iTerm do), and
 * keeps the Mac awake while any of them is working (main/keep-awake.ts). A
 * main-only channel: the Rust host never sees it, and nothing is persisted.
 */

export const WORK_IN_FLIGHT_CHANNEL = "iface:workInFlight";

export interface WorkInFlightV1 {
  readonly version: 1;
  /** Agent panes working, waiting for an answer, or starting. */
  readonly terminals: number;
  /** Threads (and agent tasks) Interface started whose turn is running or waiting. */
  readonly threads: number;
  /** Away loops running or stopping. */
  readonly away: number;
  /** Teammate routine runs starting, running or waiting. */
  readonly runs: number;
  /** Of the panes, threads and runs above, those only waiting for an answer. */
  readonly waiting: number;
}

export const NO_WORK_IN_FLIGHT: WorkInFlightV1 =
  { version: 1, terminals: 0, threads: 0, away: 0, runs: 0, waiting: 0 };

const MAX_COUNT = 10_000;
const count = (value: unknown): number | null =>
  typeof value === "number" && Number.isInteger(value) && value >= 0 && value <= MAX_COUNT ? value : null;

/** A renderer report, or null for anything that is not exactly one. */
export function parseWorkInFlight(value: unknown): WorkInFlightV1 | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
  const record = value as Record<string, unknown>;
  if (record.version !== 1) return null;
  const terminals = count(record.terminals);
  const threads = count(record.threads);
  const away = count(record.away);
  const runs = count(record.runs);
  const waiting = count(record.waiting);
  if (terminals === null || threads === null || away === null || runs === null
      || waiting === null || waiting > terminals + threads + runs) return null;
  return { version: 1, terminals, threads, away, runs, waiting };
}

/** Something is making progress: the Mac should not idle-sleep under it. */
export function workingNow(work: WorkInFlightV1 | null): boolean {
  return work !== null && work.terminals + work.threads + work.away + work.runs - work.waiting > 0;
}

const plural = (n: number, one: string, many: string): string => `${n} ${n === 1 ? one : many}`;

function list(parts: readonly string[]): string {
  if (parts.length <= 1) return parts.join("");
  return `${parts.slice(0, -1).join(", ")} and ${parts[parts.length - 1]}`;
}

/** The question to ask before quitting, or null when nothing would be lost. */
export function quitPrompt(work: WorkInFlightV1 | null): { message: string; detail: string } | null {
  if (work === null) return null;
  const total = work.terminals + work.threads + work.away + work.runs;
  if (total === 0) return null;
  const parts = [
    ...work.terminals > 0 ? [plural(work.terminals, "agent pane", "agent panes")] : [],
    ...work.threads > 0 ? [plural(work.threads, "thread turn", "thread turns")] : [],
    ...work.runs > 0 ? [plural(work.runs, "teammate run", "teammate runs")] : [],
    ...work.away > 0 ? [plural(work.away, "Away loop", "Away loops")] : [],
  ];
  return {
    message: `${plural(total, "agent is", "agents are")} still working. Quit anyway?`,
    detail: `Quitting stops ${list(parts)}.${work.away > 0
      ? " A stopped Away loop does not restart on its own." : ""}`,
  };
}
