import type { EngineProbe } from "../shared/ipc";
import type {
  ReviewComment, ShipChecks, ShipLeftOut, ThreadShip, ThreadView,
} from "../shared/threads";
import { appendDiffQuote, type ThreadDiffQuote } from "./thread-diff";

/* ============================================================================
   SHIPPING A THREAD'S WORK (protocol 37) — the words and the next step

   Rust reads the checkout (`shipStatus`) and decides everything that matters:
   which files Commit stages, why a push is refused, whether `gh` works and how
   to fix it, what a ship effect's outcome was. These helpers only turn its
   answer into the Changes panel's and the changes card's words, and pick the
   one control a card offers next. Pure: no store, no `window.iface`.
   ========================================================================= */

/** What the changes card offers next, from Rust's last read. */
export type ShipNext =
  | "check"
  | "commit"
  | "branch"
  | "push"
  | "fixChecks"
  | "merge"
  | "none";

/**
 * The one next step: check again after an outcome nobody could confirm; make a
 * branch first when the checkout is on the default branch (Interface never
 * pushes there, and committing first would leave the default branch ahead);
 * commit what Commit would stage; push and open a pull request when there are
 * commits it lacks; fix a pull request's failing checks; merge one whose checks
 * pass. Nothing when the read has not come back, or there is nothing to do.
 */
export function shipNext(ship: ThreadShip | null, needsCheck: boolean): ShipNext {
  if (needsCheck) return "check";
  if (ship === null) return "none";
  if (ship.suggestedBranch !== undefined) return "branch";
  if (ship.files.length > 0 && ship.commitBlocked === undefined) return "commit";
  const pr = ship.pullRequest?.state === "open" ? ship.pullRequest : undefined;
  if (pushOffered(ship) && ship.gh === "ready") return "push";
  if (pr !== undefined && ship.checks?.state === "failing") return "fixChecks";
  if (pr !== undefined && ship.checks?.state !== "pending") return "merge";
  return "none";
}

/**
 * Whether there is something to push: commits the remote branch lacks (or no
 * remote branch yet) on a checkout Rust allows pushing. `gh` is not asked
 * here: without it the control is drawn held, with the way to fix it.
 */
export function pushOffered(ship: ThreadShip): boolean {
  if (ship.pushBlocked !== undefined || ship.suggestedBranch !== undefined) return false;
  return ship.upstream === undefined || (ship.ahead ?? 0) > 0;
}

/** "2 files to commit", "1 file to commit", or "" with nothing to commit. */
export function commitCountText(files: number): string {
  if (files === 0) return "";
  return `${files} ${files === 1 ? "file" : "files"} to commit`;
}

/** "↑2 ↓1" against the branch's upstream, or "" when level or untracked. */
export function aheadBehindText(ship: Pick<ThreadShip, "ahead" | "behind">): string {
  const parts: string[] = [];
  if ((ship.ahead ?? 0) > 0) parts.push(`↑${ship.ahead}`);
  if ((ship.behind ?? 0) > 0) parts.push(`↓${ship.behind}`);
  return parts.join(" ");
}

/** "Not pushed yet", "2 commits to push", "Pushed", or "" without a branch. */
export function pushStateText(ship: ThreadShip): string {
  if (ship.branch === undefined) return "";
  if (ship.upstream === undefined) return "Not pushed yet";
  const ahead = ship.ahead ?? 0;
  if (ahead > 0) return `${ahead} ${ahead === 1 ? "commit" : "commits"} to push`;
  return "Up to date";
}

/** "1 failing · 3 passed · 1 running", "All 4 checks passed", "No checks". */
export function checksText(checks: ShipChecks | undefined): string {
  if (checks === undefined || checks.state === "none") return "No checks";
  const parts: string[] = [];
  if (checks.failed > 0) parts.push(`${checks.failed} failing`);
  if (checks.pending > 0) parts.push(`${checks.pending} running`);
  if (checks.failed === 0 && checks.pending === 0) {
    return checks.passed === 1 ? "1 check passed" : `All ${checks.passed} checks passed`;
  }
  if (checks.passed > 0) parts.push(`${checks.passed} passed`);
  return parts.join(" · ");
}

const LEFT_OUT_WHY: Record<ShipLeftOut["reason"], string> = {
  credential: "credentials are never committed",
  notThisThread: "not this thread's",
};

/**
 * What Commit leaves out, grouped by why: "Left out: .env (credentials are
 * never committed); README.md, notes.txt (not this thread's)". "" when nothing.
 */
export function leftOutText(leftOut: readonly ShipLeftOut[]): string {
  if (leftOut.length === 0) return "";
  const groups = new Map<ShipLeftOut["reason"], string[]>();
  for (const left of leftOut) groups.set(left.reason, [...groups.get(left.reason) ?? [], left.path]);
  const said = [...groups].map(([reason, paths]) => {
    const shown = paths.length > 3 ? [...paths.slice(0, 3), `${paths.length - 3} more`] : paths;
    return `${shown.join(", ")} (${LEFT_OUT_WHY[reason]})`;
  });
  return `Left out: ${said.join("; ")}`;
}

/** A pull request's title and description from a commit message: its first line, then the rest. */
export function pullRequestText(message: string): { title: string; body: string } {
  const [first = "", ...rest] = message.split("\n");
  return { title: first.trim(), body: rest.join("\n").trim() };
}

/**
 * Write with AI's tooltip (protocol 67): clicking it is the owner's consent
 * to spend a little Claude usage, so it says so.
 */
export const WRITE_WITH_AI_TITLE =
  "Claude writes this from the thread's changes and prompts. It uses a little of your Claude usage; nothing is committed or pushed.";

/**
 * Why Write with AI is off, from the providers' last probe: Claude not
 * installed (or not answering) or signed out. Null when it may run, and
 * before any probe has come back (Rust refuses with the reason then).
 */
export function writeWithAiBlocked(health: readonly EngineProbe[]): string | null {
  if (health.length === 0) return null;
  const claude = health.find(probe => probe.engine === "claude");
  if (claude === undefined || !claude.ok || claude.info === undefined) {
    return "Write with AI uses Claude, and Claude is not installed.";
  }
  if (claude.info.auth === "signedOut") return "Write with AI uses Claude, and Claude is signed out. Sign in, then try again.";
  return null;
}

/** A review thread as a diff quote, the way a line comment reaches the draft; null for a file-level one. */
export function reviewCommentQuote(comment: ReviewComment): ThreadDiffQuote | null {
  if (comment.endLine === undefined) return null;
  const outdated = comment.outdated ? " (on lines that have changed since)" : "";
  return {
    path: comment.path,
    startLine: comment.startLine ?? comment.endLine,
    endLine: comment.endLine,
    side: comment.side,
    code: comment.code,
    comment: `${comment.comment}${outdated}`,
  };
}

/**
 * A thread's draft with every review comment added, each after what is already
 * there: a line comment as the diff panel's quotes are (`appendDiffQuote`), a
 * comment on a whole file as its path and the comment.
 */
export function appendReviewComments(draft: string, comments: readonly ReviewComment[]): string {
  let next = draft;
  for (const comment of comments) {
    const quote = reviewCommentQuote(comment);
    if (quote !== null) {
      next = appendDiffQuote(next, quote);
    } else {
      const current = next.replace(/\s+$/u, "");
      const block = `${comment.path}\n${comment.comment}\n\n`;
      next = current === "" ? block : `${current}\n\n${block}`;
    }
  }
  return next;
}

/** What the card says about shipping, in one line: "2 files to commit", "#42 · 1 failing". */
export function shipCardText(ship: ThreadShip | null, thread: Pick<ThreadView, "pullRequest">): string {
  const pr = ship?.pullRequest ?? thread.pullRequest;
  if (ship?.suggestedBranch !== undefined && ship.branch !== undefined) {
    return [`On ${ship.branch}`, commitCountText(ship.files.length)].filter(part => part !== "").join(" · ");
  }
  if (ship !== null && ship.files.length > 0) return commitCountText(ship.files.length);
  if (pr === undefined) return ship === null ? "" : pushStateText(ship);
  const state = pr.state === "open" ? (ship?.checks === undefined ? "Open" : checksText(ship.checks))
    : pr.state === "merged" ? "Merged" : "Closed";
  return `#${pr.number} · ${state}`;
}

/** What the latest changes card says and offers for shipping (plain data, for the row memo). */
export interface ThreadShipCardView {
  text: string;
  next: ShipNext;
  /** The next step's button words, or "" for none. */
  label: string;
  /** An effect or a read is on its way: the button waits. */
  busy: boolean;
}

const NEXT_LABEL: Record<ShipNext, string> = {
  check: "Check again",
  commit: "Commit…",
  branch: "Create branch…",
  push: "Push & open PR",
  fixChecks: "Fix failing checks",
  merge: "Merge…",
  none: "",
};

/**
 * The latest changes card's ship row: Rust's last read in a few words, and the
 * one next step. Undefined before anything is known, so the card draws no row.
 */
export function shipCardView(ship: ThreadShip | null, needsCheck: boolean, busy: boolean,
    thread: Pick<ThreadView, "pullRequest">): ThreadShipCardView | undefined {
  const next = shipNext(ship, needsCheck);
  const text = needsCheck ? "Couldn't confirm the last step" : shipCardText(ship, thread);
  if (text === "" && next === "none") return undefined;
  const open = (ship?.pullRequest ?? thread.pullRequest)?.state === "open";
  return {
    text,
    next,
    label: next === "push" && open ? "Push" : NEXT_LABEL[next],
    busy,
  };
}

