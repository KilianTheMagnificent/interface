/**
 * The quit guard's count (shared/quit-guard.ts): what quitting would cut off,
 * and what keeps the Mac awake while it works (main/keep-awake.ts).
 *
 * Agent panes count while starting, working or waiting for an answer; a plain
 * shell does not, since its output is not an agent's turn. Threads and agent
 * tasks count only when Interface started them (`origin: "app"`): a CLI
 * session found on disk keeps running when Interface quits. Teammate routine
 * runs count while starting, running or waiting. A session a pane is running
 * is that pane, an Away run's sessions are that run and a teammate run's
 * session is that run, so none is counted twice. What is only waiting for an
 * answer is counted again in `waiting`: the quit prompt asks about it, but it
 * does not keep the Mac awake.
 */

import { sessionKey, type SessionSummary } from "../shared/engine";
import type { AgentRun } from "../shared/agents";
import type { TerminalSummary } from "../shared/ipc";
import type { WorkInFlightV1 } from "../shared/quit-guard";
import { isAgentProfile } from "./agent-overview";

export function workInFlight(
  terminals: readonly Pick<TerminalSummary, "state" | "activity" | "launch" | "sessionKey">[],
  sessions: readonly Pick<SessionSummary, "engine" | "sessionId" | "status" | "origin">[],
  awaySessionKeys: readonly string[],
  awayRunning: number,
  agentRuns: readonly Pick<AgentRun, "status" | "sessionKey">[] = [],
): WorkInFlightV1 {
  const elsewhere = new Set<string>(awaySessionKeys);
  let panes = 0;
  let waiting = 0;
  for (const terminal of terminals) {
    if (terminal.state === "exited" || terminal.state === "failed") continue;
    if (terminal.sessionKey !== undefined) elsewhere.add(terminal.sessionKey);
    if (!isAgentProfile(terminal.launch?.profileId)) continue;
    if (terminal.state === "starting" || terminal.activity === "working") panes++;
    else if (terminal.activity === "waiting") { panes++; waiting++; }
  }
  let runs = 0;
  for (const run of agentRuns) {
    if (run.status !== "starting" && run.status !== "running" && run.status !== "waiting") continue;
    if (run.sessionKey !== null) elsewhere.add(run.sessionKey);
    runs++;
    if (run.status === "waiting") waiting++;
  }
  let threads = 0;
  for (const session of sessions) {
    if (session.origin !== "app") continue;
    if (session.status !== "working" && session.status !== "waiting") continue;
    if (elsewhere.has(sessionKey(session.engine, session.sessionId))) continue;
    threads++;
    if (session.status === "waiting") waiting++;
  }
  return { version: 1, terminals: panes, threads, away: Math.max(0, awayRunning), runs, waiting };
}
