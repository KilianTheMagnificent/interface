import type { TerminalSummary } from "../shared/ipc";

/**
 * The bridges between a Code pane and Threads (review C1). Rust decides who
 * drives a provider session and refuses a second writer; these only read what
 * a pane says about itself, so a menu can offer or explain the move.
 */

/** The provider CLIs whose sessions Threads can drive too. */
const THREAD_ENGINES: ReadonlySet<string> = new Set(["claude", "codex"]);

/**
 * The session key (`engine#id`) a pane's provider session has in the
 * registry, or null when the pane runs no Claude or Codex session it has
 * reported yet.
 */
export function continueAsThreadKey(
    terminal: Pick<TerminalSummary, "launch" | "launchSession"> | undefined): string | null {
  const profileId = terminal?.launch?.profileId;
  const id = terminal?.launchSession?.id?.trim() ?? "";
  if (profileId === undefined || !THREAD_ENGINES.has(profileId) || id === "") return null;
  return `${profileId}#${id}`;
}

/**
 * Why a pane cannot continue as a thread now, or null when it can: only a
 * Claude or Codex pane, and only between turns. Its menu item says this; the
 * click itself ends the pane, and Rust takes the session once it has exited.
 */
export function continueAsThreadRefusal(terminal: {
  readonly profileId?: string;
  readonly activity?: "idle" | "working" | "waiting";
  readonly status: string;
} | undefined): string | null {
  if (terminal === undefined || terminal.profileId === undefined
    || !THREAD_ENGINES.has(terminal.profileId)) {
    return "Only a Claude or Codex pane continues as a thread.";
  }
  if (terminal.status !== "running") return "The pane's agent is no longer running.";
  if (terminal.activity === "working" || terminal.activity === "waiting") {
    return "Wait for the agent to finish, or stop it, first.";
  }
  return null;
}
