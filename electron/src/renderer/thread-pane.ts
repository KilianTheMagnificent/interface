import type { EngineEvent, EngineId } from "../shared/engine";
import type {
  RestoreResult, ThreadCommand, ThreadControlResponse, ThreadsView, ThreadView,
} from "../shared/threads";
import { providerLabel } from "./provider-labels";
import { threadHandoffNotice } from "./thread-opinion";
import type { ConversationThread, TurnRestorePoint } from "./conversation-types";
import { ipcMessage } from "./ipc-message";
import { equalValue } from "./stable-values";
import type { ThreadWorkspaceChoice } from "./thread-card";
import { threadLineTotals } from "./thread-inbox";
import type { ThreadShipCardView } from "./thread-ship";

/**
 * A Threads pane's rules: what its turn controls may offer, and the commands
 * they send (`editFrom`, `forkAt`, `restoreTurn`). Rust decides whether a fork or a
 * restore is possible and refuses with a sentence when it is not; the pane
 * shows that sentence and never retries on its own.
 */

/** Session key → its thread, keeping each unchanged thread's previous object. */
export function retainThreadIndex(previous: ReadonlyMap<string, ThreadView>,
    view: ThreadsView | null): ReadonlyMap<string, ThreadView> {
  const next = new Map<string, ThreadView>();
  let same = previous.size === (view?.threads.length ?? 0);
  for (const thread of view?.threads ?? []) {
    const before = previous.get(thread.key);
    if (before !== undefined && equalValue(before, thread)) next.set(thread.key, before);
    else { next.set(thread.key, thread); same = false; }
  }
  return same ? previous : next;
}

/** A project thread: one with a folder (local or a worktree), not a plain chat. */
export function isProjectThread(thread: ThreadView): boolean {
  return thread.workspace !== undefined && thread.workspace.mode !== "none";
}

/**
 * The thread's change totals, for the head's Changes button: the inbox row's
 * `threadLineTotals` (a switched thread's every segment), so the two never
 * disagree. With no turn measured the answer is null, never a zero.
 */
export function threadChangeTotals(thread: Pick<ThreadView, "turns" | "chain">):
    { added: number; removed: number } | null {
  return threadLineTotals(thread) ?? null;
}

/**
 * The head's workspace chip: the branch when Rust recorded one, then where
 * the thread works — its project's own checkout or a worktree it works in.
 * Null for a plain chat.
 */
export function threadWorkspaceLabel(workspace: ThreadView["workspace"]): string | null {
  if (workspace === undefined || workspace.mode === "none") return null;
  const place = workspace.mode === "worktree" ? "Worktree" : "Local";
  const branch = workspace.branch?.trim();
  return branch === undefined || branch === "" ? place : `${branch} · ${place}`;
}

export const REVERT_LOCAL_BLOCKED = "This thread works in the project’s own checkout, so its files "
  + "are not restored here. Only a thread that made its own worktree can restore its files.";
export const REVERT_SHARED_BLOCKED = "This thread works in a worktree it did not create, so its "
  + "files are not restored here.";

// Per thread object, one value for an idle pane and one for a busy one.
const CONVERSATION_THREADS = new WeakMap<ThreadView, [ConversationThread?, ConversationThread?]>();

/**
 * What the pane's turn controls offer. Restoring the files is offered only
 * to a thread that owns its worktree; Rust still checks that nothing else
 * uses the folder and may refuse. `busy`: a Fork or an Edit from this pane
 * is on its way. Cached per thread object, so a retained thread hands the
 * pane the same value and its rows skip the render.
 */
export function conversationThread(thread: ThreadView, busy = false): ConversationThread {
  const pair = CONVERSATION_THREADS.get(thread) ?? [];
  const known = pair[busy ? 1 : 0];
  if (known !== undefined) return known;
  const workspace = thread.workspace;
  const revertFilesBlocked = workspace?.mode === "worktree"
    ? workspace.owned ? null : REVERT_SHARED_BLOCKED
    : REVERT_LOCAL_BLOCKED;
  const value: ConversationThread = {
    turns: thread.turns, project: isProjectThread(thread),
    worktree: workspace?.mode === "worktree", revertFilesBlocked,
    ...(busy ? { busy } : {}),
    ...(thread.lastRun === undefined ? {} : { lastRun: thread.lastRun }),
    ...(thread.away === undefined ? {} : { away: thread.away }),
    ...(thread.chain === undefined ? {} : { switched: true }),
  };
  pair[busy ? 1 : 0] = value;
  CONVERSATION_THREADS.set(thread, pair);
  return value;
}

// Per pane value, the one carrying each ship row it was last given.
const WITH_SHIP = new WeakMap<ConversationThread, { ship: ThreadShipCardView; value: ConversationThread }>();

function sameShipCard(left: ThreadShipCardView, right: ThreadShipCardView): boolean {
  return left === right || (
    left.text === right.text
    && left.next === right.next
    && left.label === right.label
    && left.busy === right.busy
  );
}

/**
 * `base` with the latest changes card's ship row. The same row gives back the
 * same object, so a pane whose row did not change skips the render.
 */
export function withShip(base: ConversationThread, ship: ThreadShipCardView | undefined): ConversationThread {
  if (ship === undefined) return base;
  const known = WITH_SHIP.get(base);
  if (known !== undefined && sameShipCard(known.ship, ship)) return known.value;
  const value = { ...base, ship };
  WITH_SHIP.set(base, { ship, value });
  return value;
}

/**
 * The new thread card's project and workspace for "the same workspace" as
 * `thread`: its worktree as an existing one, or the project's own checkout
 * for a local thread. A worktree that is gone is a new worktree from the
 * same base, never the project's checkout: the thread was isolated, and the
 * edit must not start working in the owner's own files.
 */
export function sameWorkspaceAs(thread: ThreadView): { project: string | null;
    workspace: ThreadWorkspaceChoice } {
  const workspace = thread.workspace;
  if (workspace === undefined || workspace.mode === "none") {
    return { project: null, workspace: { mode: "local" } };
  }
  if (workspace.mode === "worktree") {
    if (workspace.worktreePath !== undefined && workspace.missing !== true) {
      return { project: workspace.projectRoot,
        workspace: { mode: "existing", worktreePath: workspace.worktreePath } };
    }
    const baseRef = workspace.baseRef?.trim() ?? "";
    return { project: workspace.projectRoot,
      workspace: baseRef === "" ? { mode: "worktree" } : { mode: "worktree", baseRef } };
  }
  return { project: workspace.projectRoot, workspace: { mode: "local" } };
}

/** What a pane says while a second Fork or Edit is refused. */
export const THREAD_PANE_BUSY = "A fork or an edit from this thread is already starting.";

/**
 * One Fork or Edit at a time per pane. `run` refuses (null) while that
 * pane's last one has not answered, so a double click makes one thread;
 * `changed` hands the panes that are waiting to whoever draws them.
 */
export class ThreadPaneLock {
  private readonly held = new Set<string>();

  constructor(private readonly changed: (held: ReadonlySet<string>) => void) {}

  holds(key: string): boolean {
    return this.held.has(key);
  }

  run<T>(key: string, task: () => Promise<T>): Promise<T> | null {
    if (this.held.has(key)) return null;
    this.held.add(key);
    this.changed(new Set(this.held));
    let running: Promise<T>;
    try {
      running = task();
    } catch (err: unknown) {
      running = Promise.reject(err);
    }
    return running.finally(() => {
      this.held.delete(key);
      this.changed(new Set(this.held));
    });
  }
}

export interface ThreadPanePort {
  request(command: ThreadCommand): Promise<ThreadControlResponse>;
  /** Show this thread (select it, open its pane, focus it). */
  select(key: string): void;
  /** Put text in a thread's composer draft. */
  setDraft(key: string, text: string): void;
  /**
   * Open the New thread card on this project and workspace with `prompt` in
   * it, and `note` shown in the card while it is open.
   */
  openCard(project: string | null, workspace: ThreadWorkspaceChoice, prompt: string, note?: string): void;
  /**
   * The permission mode this session was given here (launched with or
   * changed to), if any. Nothing on the wire carries it, so a fork would
   * otherwise start in the provider's default.
   */
  permissionModeOf(key: string): string | undefined;
  /** Put a new thread in `mode`, as the pane's access picker does. */
  setPermissionMode(key: string, mode: string): void;
  /** The session is working or waiting on the owner right now. */
  working(key: string): boolean;
  /**
   * Close a settled thread's live provider session, as Rust does after an
   * edit, so its handle stops holding the worktree the new thread takes
   * over. Best effort: a session that is busy again, or a close that fails,
   * stays open and changes nothing else.
   */
  closeSettled(key: string): void;
  /** A status line for the owner: what a restore did and where its recovery point is. */
  notify(message: string): void;
}

type ForkPort = Pick<ThreadPanePort, "permissionModeOf" | "setPermissionMode">;

/**
 * A fork or an edit starts in its parent's permission mode. The mode is read
 * before the command, because an edit closes the parent's session and its
 * mode goes with it; the provider default needs nothing applied.
 */
function inheritPermissionMode(port: ForkPort, parentMode: string | undefined,
    child: string): void {
  if (parentMode !== undefined && parentMode !== "" && parentMode !== "default") {
    port.setPermissionMode(child, parentMode);
  }
}

/** Rust's sentence for a send to a thread that is still working (THREADS_BUSY). */
export const THREAD_STILL_WORKING = "The agent is still working. Wait for it to finish, or stop it, then try again.";

/**
 * The New thread card's note after Edit from the first message restored the
 * files: the card covers the pane, so the restore is said in the card too.
 */
export const THREAD_FIRST_EDIT_RESTORED = "Files restored to before this message — recovery point saved";

/** What a restore did, and the recovery point Rust kept of the files it replaced. */
export function threadRecoveryNotice(restore: RestoreResult, ordinal: number): string {
  const files = `${restore.restored} ${restore.restored === 1 ? "file" : "files"}`;
  return `Restored ${files} to before message ${ordinal + 1}. Recovery point saved: ${restore.recoveryRef}`;
}

/**
 * The confirmation a worktree turn's Undo shows. The count is only known once
 * Rust has restored, so it promises only what `restoreTurn` guarantees: the
 * worktree goes back to the turn's start checkpoint, the current files are
 * kept at a recovery point first, and the conversation is untouched.
 */
export function threadFilesRestorePoint(ordinal: number): TurnRestorePoint {
  return {
    description: `Restores this worktree's files to before message ${ordinal + 1}. `
      + "The current files are saved to a recovery point first. The conversation is not changed.",
  };
}

/**
 * Undo turn `ordinal`'s files in a worktree thread: Rust restores the
 * worktree to the turn's start checkpoint (`restoreTurn`), keeping a
 * recovery point of the files it replaces. The thread, its conversation and
 * its turns are untouched — the files-only counterpart of Edit from here.
 * A refusal rejects with Rust's sentence.
 */
export async function restoreThreadFiles(port: Pick<ThreadPanePort, "request" | "notify">,
    thread: ThreadView, ordinal: number): Promise<void> {
  const answer = await ask(port, { kind: "restoreTurn", key: thread.key, ordinal });
  if (answer.restore !== undefined) port.notify(threadRecoveryNotice(answer.restore, ordinal));
}

async function ask(port: Pick<ThreadPanePort, "request">,
    command: ThreadCommand): Promise<ThreadControlResponse> {
  try {
    return await port.request(command);
  } catch (err: unknown) {
    throw new Error(ipcMessage(err));
  }
}

/**
 * Edit from here: a new thread starts from just before turn `ordinal`, with
 * its prompt to change, and this thread is settled.
 *
 * Any later turn than the first asks Rust (`editFrom`), which does it all at
 * once: restores the files first when asked and allowed, forks before that
 * turn, hands the worktree to the new thread, settles this one and closes
 * its session, and answers the new thread's key and the turn's prompt. The
 * new thread is selected with that prompt in its draft, in this thread's
 * permission mode.
 *
 * Turn 0 has nothing before it to fork from, so it goes in steps here: Rust
 * restores the files to before it when asked (`restoreTurn 0`, keeping a
 * recovery point), this thread is settled and its session closed, and the
 * New thread card opens on the same workspace with the prompt in it. The new
 * thread takes the worktree over when it binds to it; a card closed without
 * starting leaves this thread settled, and its menu un-settles it. The card
 * says the files were restored, since it covers the pane's own notice.
 *
 * A refusal rejects with Rust's sentence. A recovery point is announced the
 * moment it exists.
 */
export async function editThreadFrom(port: ThreadPanePort, thread: ThreadView, ordinal: number,
    restoreFiles: boolean, prompt: string): Promise<void> {
  if (ordinal === 0) {
    // Rust refuses a busy edit for every later turn; the first turn's steps
    // keep the same rule, so a running thread is never settled under its agent.
    if (port.working(thread.key)) throw new Error(THREAD_STILL_WORKING);
    let restored = false;
    if (restoreFiles) {
      const answer = await ask(port, { kind: "restoreTurn", key: thread.key, ordinal: 0 });
      if (answer.restore !== undefined) port.notify(threadRecoveryNotice(answer.restore, 0));
      restored = true;
    }
    try {
      await port.request({ kind: "settle", key: thread.key });
    } catch (err: unknown) {
      throw new Error(restored
        ? `The files were restored, but this thread was not settled: ${ipcMessage(err)}`
        : ipcMessage(err));
    }
    port.closeSettled(thread.key);
    const same = sameWorkspaceAs(thread);
    if (restored) port.openCard(same.project, same.workspace, prompt, THREAD_FIRST_EDIT_RESTORED);
    else port.openCard(same.project, same.workspace, prompt);
    return;
  }
  const parentMode = port.permissionModeOf(thread.key);
  const answer = await ask(port, { kind: "editFrom", key: thread.key, ordinal, restoreFiles });
  if (answer.restore !== undefined) port.notify(threadRecoveryNotice(answer.restore, ordinal));
  if (answer.key === undefined) throw new Error("The new thread was not named. Nothing was opened.");
  inheritPermissionMode(port, parentMode, answer.key);
  port.setDraft(answer.key, answer.prompt ?? prompt);
  port.select(answer.key);
}

/**
 * Fork from here: `forkAt`, then the new thread is selected in its parent's
 * permission mode. Rejects with Rust's sentence.
 */
export async function forkThreadAt(port: Pick<ThreadPanePort, "request" | "select"> & ForkPort,
    key: string, keepTurns: number): Promise<string> {
  const parentMode = port.permissionModeOf(key);
  const answer = await ask(port, { kind: "forkAt", key, keepTurns });
  if (answer.key === undefined) throw new Error("The forked thread was not named. Nothing was opened.");
  inheritPermissionMode(port, parentMode, answer.key);
  port.select(answer.key);
  return answer.key;
}

/** What changes in a thread that the open diff should be read again for. */
export function threadDiffSignal(thread: ThreadView | undefined): string {
  if (thread === undefined) return "";
  const last = thread.turns.at(-1);
  return [thread.turns.length, last?.state ?? "", last?.hasEnd ?? "",
    thread.lastRun?.endedAt ?? "", thread.workspace?.missing ?? ""].join(":");
}

/** What switching a thread's provider reaches (protocol 59). */
export interface ThreadSwitchPort {
  request(command: ThreadCommand): Promise<ThreadControlResponse>;
  /**
   * The thread runs on `to` now: its pane, draft and attachments go there.
   * `launched` is the handoff a new ACP session started with, which no send
   * echoed, for the pane to show as its launch prompt.
   */
  carry(from: string, to: string, launched: string | undefined, switchedAt: number): void;
  /** Show the thread on its new session. */
  select(key: string): void;
  /** Milliseconds since the epoch: when the switch was asked for. */
  now(): number;
}

/**
 * Switch to: Rust carries thread `key` on to `engine` and answers the new
 * head; the pane, the draft and the attachments follow it there, and the
 * thread is shown. Resolves the new key and what to tell the owner when the
 * handoff may not have arrived (it is never sent again: the owner types).
 * Rejects with Rust's sentence; then nothing moved.
 */
export async function switchThreadTo(port: ThreadSwitchPort, key: string, engine: EngineId):
    Promise<{ key: string; notice: string | null }> {
  const switchedAt = port.now();
  const answer = await ask(port, { kind: "switchProvider", key, engine });
  const next = answer.key;
  if (next === undefined) throw new Error("The thread's new session was not named. Nothing moved.");
  if (next !== key) port.carry(key, next, answer.prompt, switchedAt);
  port.select(next);
  return { key: next, notice: threadHandoffNotice(answer.handoff, providerLabel(engine)) };
}

/** `map` with `from`'s entry under `to` instead (a switched thread's attachments). */
export function carryKeyed<T>(map: Record<string, T>, from: string, to: string): Record<string, T> {
  if (!(from in map)) return map;
  const { [from]: value, ...rest } = map;
  return { ...rest, [to]: value as T };
}

/** The provider as Rust's switch notice names it ("Claude", "Codex", "Grok"). */
export function switchedProviderWord(engine: string): string {
  if (engine === "claude") return "Claude";
  if (engine === "codex") return "Codex";
  const name = engine.startsWith("acp:") ? engine.slice(4) : engine;
  return name === "" ? name : name[0]!.toUpperCase() + name.slice(1);
}

/**
 * What a switched thread's pane draws before the new head's live stream
 * (protocol 59): the thread's history from before the switch (`before`, ms;
 * the head's own stretch arrives live), then the quiet "Switched to …" row
 * Rust's history puts there. The new head is live before its pane opens, so
 * its history fence is already shut and this is read once, by hand.
 */
export function switchSeed(events: readonly EngineEvent[], before: number, engine: string): EngineEvent[] {
  const cut = new Date(before).toISOString();
  const earlier = events.filter(event => event.at === undefined || event.at < cut);
  return earlier.length === 0 ? [] : [...earlier,
    { at: cut, kind: "notice", message: `Switched to ${switchedProviderWord(engine)}`, tone: "info" }];
}
