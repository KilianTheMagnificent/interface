import { useMemo, useState } from "react";
import type {
  CommitChoice, CommitReview, MergeMethod, ReviewComment, ShipAction, ShipTextKind, ThreadCommand,
  ThreadControlResponse, ThreadShip,
} from "../shared/threads";
import { ipcErrorShape, ipcMessage } from "./ipc-message";
import { pullRequestText } from "./thread-ship";

/** The ship effect (or read) on its way for a thread. */
export type ShipBusy = "read" | "reviewCommit" | "commit" | "branch" | "push" | "fixChecks" | "comments" | "merge";

/** Write with AI's answer (protocol 67): the text to put in the field, or why there is none. */
export type ShipTextResult = { text: string } | { error: string };

/** What the last ship effect said: it happened, nobody could tell, or it was refused. */
export interface ShipNotice { tone: "done" | "unknown" | "error"; message: string }

/** One thread's shipping, as the Changes panel and the changes card draw it. */
export interface ThreadShipState {
  /** Rust's last read of the checkout, or null before the first. */
  ship: ThreadShip | null;
  busy: ShipBusy | null;
  notice: ShipNotice | null;
  /**
   * An outcome nobody could confirm (`shipped.certainty: unknown`): every
   * effect waits until a fresh read says where things stand. Nothing retries.
   */
  needsCheck: boolean;
  /** Which effect's outcome is unknown. */
  unknown: ShipAction | null;
  /** When the last read came back (ms), so a card's read is not repeated at once. */
  readAt: number;
  commitReview: CommitReview | null;
}

export const NO_SHIP: ThreadShipState = {
  ship: null, busy: null, notice: null, needsCheck: false, unknown: null, readAt: 0, commitReview: null,
};

/** How long a read stands in for a new one a card asks for. */
export const SHIP_READ_FRESH_MS = 30_000;

/**
 * Whether a read settles an unknown outcome: a commit's or a branch's needs
 * only git, which every read has; a push's, a pull request's or a merge's
 * needs `gh` to have answered too.
 */
export function readSettles(unknown: ShipAction | null, ship: ThreadShip): boolean {
  if (unknown === null || unknown === "commit" || unknown === "createBranch") return true;
  return ship.remoteRead && ship.gh === "ready";
}

/**
 * The read to show: `next`, keeping what an earlier read learnt through `gh`
 * (its state, the pull request and its checks) when `next` asked git only.
 * Never used to settle an unknown outcome: that takes a read of its own.
 */
export function shownRead(previous: ThreadShip | null, next: ThreadShip): ThreadShip {
  if (next.remoteRead || previous === null || !previous.remoteRead || previous.key !== next.key) return next;
  const { ghFix, pullRequest, checks } = previous;
  return {
    ...next,
    gh: previous.gh,
    remoteRead: true,
    defaultBranch: next.defaultBranch ?? previous.defaultBranch,
    ...(ghFix === undefined ? {} : { ghFix }),
    ...(pullRequest === undefined ? {} : { pullRequest }),
    ...(checks === undefined ? {} : { checks }),
  };
}

export interface ThreadShipPort {
  request: (command: ThreadCommand) => Promise<ThreadControlResponse>;
  /** Sends `text` to the thread as its next prompt, through its pane (as Retry does). */
  send: (key: string, text: string) => Promise<void>;
  /** Adds review comments to the thread's draft, the way a diff line comment goes. */
  quote: (key: string, comments: readonly ReviewComment[]) => void;
  now?: () => number;
}

/**
 * Shipping for every thread, keyed by session key, without React: each
 * effect is one Rust command (`commit`, `createBranch`, `openPullRequest`,
 * `failingChecksPrompt`, `reviewComments`, `mergePullRequest`) whose answer
 * carries a fresh read. One effect at a time per thread; none while an unknown
 * outcome waits for a read that settles it (`readSettles`); a refused effect
 * reads again so the controls show what is true now. Nothing retries.
 */
export class ThreadShipController {
  private states: ReadonlyMap<string, ThreadShipState> = new Map();
  /** Each thread's newest read or effect: an older read that lands later is dropped. */
  private readonly generations = new Map<string, number>();
  /** The message each thread's last commit here was made with: its pull request's words. */
  private readonly committed = new Map<string, string>();

  constructor(private port: ThreadShipPort,
      private readonly changed: (states: ReadonlyMap<string, ThreadShipState>) => void) {}

  setPort(port: ThreadShipPort): void {
    this.port = port;
  }

  stateOf(key: string): ThreadShipState {
    return this.states.get(key) ?? NO_SHIP;
  }

  private now(): number {
    return (this.port.now ?? Date.now)();
  }

  private update(key: string, change: (state: ThreadShipState) => ThreadShipState): void {
    const next = new Map(this.states);
    next.set(key, change(this.stateOf(key)));
    this.states = next;
    this.changed(next);
  }

  private bump(key: string): number {
    const generation = (this.generations.get(key) ?? 0) + 1;
    this.generations.set(key, generation);
    return generation;
  }

  /** Reads the checkout (and, with `remote`, the pull request through `gh`). */
  refresh(key: string, remote = true): Promise<void> {
    // Opening Changes can request a read after Commit has begun its review.
    // A background read must not invalidate that review or an active effect.
    const busy = this.stateOf(key).busy;
    if (busy !== null && busy !== "read") return Promise.resolve();
    const generation = this.bump(key);
    this.update(key, state => ({ ...state, busy: state.busy ?? "read" }));
    return this.port.request({ kind: "shipStatus", key, remote }).then(answer => {
      if (this.generations.get(key) !== generation) return;
      this.update(key, state => {
        const settled = answer.ship !== undefined && readSettles(state.unknown, answer.ship);
        return {
          ...state,
          ship: answer.ship === undefined ? state.ship : shownRead(state.ship, answer.ship),
          busy: state.busy === "read" ? null : state.busy,
          needsCheck: state.needsCheck && !settled,
          unknown: settled ? null : state.unknown,
          readAt: this.now(),
        };
      });
    }, (err: unknown) => {
      if (this.generations.get(key) !== generation) return;
      this.update(key, state => ({
        ...state,
        busy: state.busy === "read" ? null : state.busy,
        notice: { tone: "error", message: ipcMessage(err) },
      }));
    });
  }

  /** A card's read: only when nothing fresh is known and nothing is on its way. */
  ensure(key: string): void {
    const state = this.stateOf(key);
    if (state.busy !== null || (state.ship !== null && this.now() - state.readAt < SHIP_READ_FRESH_MS)) return;
    void this.refresh(key, true);
  }

  /**
   * Runs one effect, unless another runs or an unknown outcome waits to be
   * checked; its answer's read and outcome replace the thread's.
   */
  private async effect(key: string, busy: ShipBusy, command: ThreadCommand,
      after?: (answer: ThreadControlResponse) => Promise<string | null> | string | null): Promise<void> {
    const state = this.stateOf(key);
    if ((state.busy !== null && state.busy !== "read") || state.needsCheck) return;
    this.bump(key);
    this.update(key, current => ({ ...current, busy, notice: null }));
    let answer: ThreadControlResponse;
    try {
      answer = await this.port.request(command);
    } catch (err: unknown) {
      const shape = ipcErrorShape(err);
      const uncertain = command.kind === "commitSelected" && (shape.kind !== "typed" || !shape.replaySafe);
      this.update(key, current => ({ ...current, busy: null,
        commitReview: command.kind === "commitSelected" ? null : current.commitReview,
        needsCheck: uncertain, unknown: uncertain ? "commit" : current.unknown,
        notice: { tone: uncertain ? "unknown" : "error", message: ipcMessage(err) } }));
      if (!uncertain) void this.refresh(key, true);
      return;
    }
    const ship = answer.ship;
    let message: string | null = answer.shipped?.message ?? null;
    let tone: ShipNotice["tone"] = answer.shipped?.certainty === "unknown" ? "unknown" : "done";
    if (after !== undefined) {
      try {
        message = await after(answer);
      } catch (err: unknown) {
        message = ipcMessage(err);
        tone = "error";
      }
    }
    const unknown = tone === "unknown" ? answer.shipped?.action ?? null : null;
    this.update(key, current => ({
      ...current,
      commitReview: command.kind === "commitSelected" ? null : current.commitReview,
      ...(ship === undefined ? {} : { ship: shownRead(current.ship, ship), readAt: this.now() }),
      busy: null,
      needsCheck: tone === "unknown",
      unknown,
      notice: message === null || message === "" ? null : { tone, message },
    }));
  }

  commit(key: string, message: string, paths: readonly string[]): Promise<void> {
    return this.effect(key, "commit", { kind: "commit", key, message, paths: [...paths] }, answer => {
      if (answer.shipped?.certainty === "done") this.committed.set(key, message);
      return answer.shipped?.message ?? null;
    });
  }

  async reviewCommit(key: string): Promise<void> {
    const state = this.stateOf(key);
    if ((state.busy !== null && state.busy !== "read") || state.needsCheck) return;
    const generation = this.bump(key);
    this.update(key, current => ({ ...current, busy: "reviewCommit", commitReview: null }));
    try {
      const answer = await this.port.request({ kind: "reviewCommit", key });
      if (this.generations.get(key) !== generation) return;
      this.update(key, current => ({ ...current, busy: null, commitReview: answer.commitReview ?? null }));
    } catch (err) {
      if (this.generations.get(key) !== generation) return;
      this.update(key, current => ({ ...current, busy: null, notice: { tone: "error", message: ipcMessage(err) } }));
    }
  }

  commitSelected(key: string, message: string, revision: string, choices: readonly CommitChoice[]): Promise<void> {
    return this.effect(key, "commit", { kind: "commitSelected", key, message, revision, choices: [...choices] }, answer => {
      if (answer.shipped?.certainty === "done") this.committed.set(key, message);
      return answer.shipped?.message ?? null;
    });
  }

  createBranch(key: string, branch: string): Promise<void> {
    return this.effect(key, "branch", { kind: "createBranch", key, branch });
  }

  /**
   * Push & open PR, titled and described by the message the owner last
   * committed with here, else by the suggested one.
   */
  openPullRequest(key: string): Promise<void> {
    const { title, body } = this.pullRequestDraft(key);
    return this.openPullRequestWith(key, title, body);
  }

  /** Push & open PR with the title and body the owner has in the pull request form. */
  openPullRequestWith(key: string, title: string, body: string): Promise<void> {
    return this.effect(key, "push", {
      kind: "openPullRequest", key, title: title.trim() === "" ? "Changes" : title.trim(), body,
    });
  }

  /** The pull request form's first words: the message last committed with here, else the suggested one. */
  pullRequestDraft(key: string): { title: string; body: string } {
    return pullRequestText(this.committed.get(key) ?? this.stateOf(key).ship?.message ?? "");
  }

  /**
   * Write with AI (protocol 67): Claude's text for the commit message (from
   * `paths`, the files being committed) or the pull request. Changes nothing;
   * a refusal comes back as words, never thrown.
   */
  async suggestText(key: string, what: ShipTextKind, paths?: readonly string[]): Promise<ShipTextResult> {
    try {
      const answer = await this.port.request({
        kind: "suggestShipText", key, what, ...(paths === undefined ? {} : { paths: [...paths] }),
      });
      return answer.shipText === undefined ? { error: "Claude wrote nothing; try again." } : { text: answer.shipText };
    } catch (err: unknown) {
      return { error: ipcMessage(err) };
    }
  }

  /** Rust's prompt for the failing checks, sent through the thread's pane. */
  fixChecks(key: string): Promise<void> {
    return this.effect(key, "fixChecks", { kind: "failingChecksPrompt", key }, async answer => {
      if (answer.prompt === undefined) return "No prompt came back.";
      await this.port.send(key, answer.prompt);
      return "Asked the thread to fix the failing checks.";
    });
  }

  /** The unresolved review comments, into the thread's draft. */
  bringComments(key: string): Promise<void> {
    return this.effect(key, "comments", { kind: "reviewComments", key }, answer => {
      const comments = answer.reviewComments ?? [];
      if (comments.length === 0) return "No unresolved review comments.";
      this.port.quote(key, comments);
      return `Added ${comments.length} review ${comments.length === 1 ? "comment" : "comments"} to the prompt.`;
    });
  }

  merge(key: string, number: number, method: MergeMethod): Promise<void> {
    return this.effect(key, "merge", { kind: "mergePullRequest", key, number, method });
  }

  dismiss(key: string): void {
    this.update(key, state => ({ ...state, notice: null }));
  }
}

/** The shell's one {@link ThreadShipController}, re-rendering on every change. */
export function useThreadShip(port: ThreadShipPort) {
  const [states, setStates] = useState<ReadonlyMap<string, ThreadShipState>>(() => new Map());
  const [controller] = useState(() => new ThreadShipController(port, setStates));
  controller.setPort(port);
  return useMemo(() => ({
    states,
    stateOf: (key: string): ThreadShipState => states.get(key) ?? NO_SHIP,
    refresh: (key: string, remote = true): void => { void controller.refresh(key, remote); },
    ensure: (key: string): void => { controller.ensure(key); },
    commit: (key: string, message: string, paths: readonly string[]) => controller.commit(key, message, paths),
    reviewCommit: (key: string) => controller.reviewCommit(key),
    commitSelected: (key: string, message: string, revision: string, choices: readonly CommitChoice[]) => controller.commitSelected(key, message, revision, choices),
    createBranch: (key: string, branch: string) => controller.createBranch(key, branch),
    openPullRequest: (key: string) => controller.openPullRequest(key),
    openPullRequestWith: (key: string, title: string, body: string) => controller.openPullRequestWith(key, title, body),
    pullRequestDraft: (key: string) => controller.pullRequestDraft(key),
    suggestText: (key: string, what: ShipTextKind, paths?: readonly string[]) => controller.suggestText(key, what, paths),
    fixChecks: (key: string) => controller.fixChecks(key),
    bringComments: (key: string) => controller.bringComments(key),
    merge: (key: string, number: number, method: MergeMethod) => controller.merge(key, number, method),
    dismiss: (key: string): void => { controller.dismiss(key); },
  }), [controller, states]);
}
