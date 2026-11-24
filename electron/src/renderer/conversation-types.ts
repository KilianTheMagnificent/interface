import type { FastModeState, ModelOption, PlanEntry } from "../shared/engine";
import type { ThreadAway } from "../shared/away";
import type { ThreadRun, ThreadTurn } from "../shared/threads";
import type { ConvItem } from "./conversation-reducer";
import type { ThreadShipCardView } from "./thread-ship";

/**
 * Everything the model/effort pills render, computed by the caller. The pane
 * never asks the engine anything: `models` is whatever the session's last
 * `models.available` carried, and `model`/`effort` are the caller's derived
 * effective pair — optimistically the pending one while `pending` is true.
 */
export interface SessionConfigView {
  /** Stable routing id and human provider name for the shared selector identity. */
  engine?: string;
  engineLabel?: string;
  /** undefined means the engine's own default. */
  model?: string;
  /** null means no effort flag is set. */
  effort: string | null;
  /** The engine's real list. Empty (none yet) switches the picker to free text. */
  models: ModelOption[];
  /** False when this transport cannot change a running session's model. */
  canSetModel: boolean;
  /** Levels the SELECTED model offers — already filtered by the caller. */
  effortLevels: string[];
  /** False when the selected model reports supportsEffort: false; hides the pill. */
  showEffort: boolean;
  /** False when this transport has no mid-session effort operation. */
  canSetEffort: boolean;
  /** Per field: this one's value above is the optimistic ask, not a confirmation. */
  pendingModel: boolean;
  pendingEffort: boolean;
  /** Current access policy, when this transport can change it mid-session. */
  permissionMode?: string;
  permissionModes?: readonly string[];
  permissionUnknown?: boolean;
  canSetPermission?: boolean;
  pendingPermission?: boolean;
  /** The selected model advertises the provider's fast mode. */
  supportsFastMode?: boolean;
  /** The provider's reported fast-mode lifecycle; null/undefined reads as off. */
  fastMode?: FastModeState | null;
  fastModeReason?: string;
  pendingFastMode?: boolean;
  /** The engine's refusal of the last change, already reverted by the caller. */
  error: string | null;
}

/**
 * The session's PAST transcript — everything it already contained before this
 * window opened — and the state of the read that produced it.
 *
 *  - "idle"    nothing to load and nothing loaded. A session started in THIS app
 *              run is already whole in `items`, so its history view stays idle
 *              and the pane behaves exactly as it did before history existed.
 *              An absent `history` prop means this.
 *  - "loading" the read is in flight. It is not instant: the largest transcript
 *              on the author's machine is 11 MB, so this state is a real screen
 *              the user sees, not a formality.
 *  - "ready"   `items` is the past transcript, oldest first. Empty is a legal,
 *              meaningful answer: the session really has no messages.
 *              `olderMessages` is the engine read's own count of what it left
 *              out (HistoryPage.olderMessages), and is what lets the pane say
 *              "853 older messages are not shown" instead of the far less
 *              useful "older messages are not shown". 0 means nothing was
 *              dropped. `truncated` is the older boolean form of the same
 *              disclosure, kept for a caller that has only the fact and not the
 *              number; it is consulted only when `olderMessages` is absent.
 *              `loadingOlder` keeps this ready page on screen while its strict
 *              superset is being read; `loadOlderError` reports only that page
 *              failure and never discards the transcript already present.
 *              `canLoadOlder` is false at the renderer's hard page ceiling.
 *  - "error"   the read threw. `message` is shown verbatim, so it must already
 *              be a sentence for a human, never a raw "Error invoking remote method".
 */
export type HistoryView =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "ready"; items: ConvItem[]; olderMessages?: number; truncated?: boolean;
      plan?: readonly PlanEntry[]; loadingOlder?: boolean; loadOlderError?: string;
      canLoadOlder?: boolean }
  | { status: "unavailable"; message: string }
  | { status: "error"; message: string };

/**
 * THE HISTORY PROP CONTRACT — what the caller (App.tsx, which owns every
 * window.iface call) must do. The pane fetches nothing; it renders the past
 * transcript, then the live stream, then the messages this window itself sent
 * (which no engine will ever send back), and owns only the states around them.
 *
 *  1. MAP HISTORY THROUGH THE SAME REDUCER. Whatever the IPC read returns must
 *     reach this pane as ConvItem[] built by conversationItems() — the very
 *     function the live stream already goes through. Then a past tool call is
 *     the same 22px row as a live one, a past subagent is the same card, and
 *     this component needs no second renderer. If the main process returns
 *     EngineEvent[], the caller's whole job is
 *     `useMemo(() => conversationItems(pastEvents), [pastEvents])`.
 *  2. SWITCH IT WITH THE SESSION, SYNCHRONOUSLY. `history` must describe the
 *     same session as `items` on every single render. On a selection change it
 *     goes to { status: "loading" } (or "idle") in the SAME state update that
 *     changes the selection — never one commit later, or the pane paints the
 *     previous session's transcript under the new session's title.
 *  3. KEEP THE TWO LISTS DISJOINT. `history` is strictly what PREDATES `items`.
 *     A larger user-requested page must REPLACE the prior page because the
 *     provider returns its strict superset; never concatenate the two. Do not
 *     perform background re-reads after live events start; the one exception
 *     is a session that went live before ANY page of it was read (a send the
 *     host started, at launch), whose single late read is cut to what
 *     predates the stream (history-paging.ts `historyBeforeLive`). mergeTranscript()
 *     drops a history tool/approval/subagent whose engine-minted id also appears
 *     live, and drops a locally echoed send once the transcript itself supplies
 *     it, but those are safety nets over prose it cannot dedupe, not a licence.
 *  4. `onRetryHistory` and `onLoadOlderHistory` are optional. The former appears
 *     on an initial-read failure; the latter appears only while a ready page says
 *     older messages remain. The caller owns both reads and their request fence.
 *
 * The pane never reorders: history renders in the order given, then a labelled
 * seam, then the live items in the order given.
 */

/**
 * ONE RESTORE POINT — a checkpoint the caller can roll the working tree back
 * to, standing at the user turn that came after it.
 *
 * The pane owns no checkpoint machinery and reaches no IPC: it renders a
 * control exactly where the caller says a restore point exists, confirms the
 * action, and hands the turn's key back. Everything else — creating the
 * checkpoint, mapping it to a turn, performing the restore and reporting how it
 * went — belongs to the caller, which is the only place that may call
 * `window.iface`.
 */
export interface TurnRestorePoint {
  /**
   * What the control says. Defaults to "Restore". Keep it short — it sits on
   * the user's own turn card, which is 88% of a pane that can be 383px wide.
   */
  label?: string;
  /**
   * The confirm dialog's body: what this click is about to overwrite, already a
   * sentence for a human ("Restores 14 files in ~/Projects/Interface to the
   * checkpoint taken 6 minutes ago. Later edits to those files are lost.").
   * It is shown verbatim, so it must be true — the pane cannot check it.
   */
  description: string;
}

/**
 * A Threads pane's turn controls: Rust's turn records for this thread and what
 * the thread allows. The pane maps its own user rows to `turns` by prompt
 * digest (thread-turns.ts); a row that maps to none gets no control. Absent,
 * the pane is exactly the conversation it was.
 */
export interface ConversationThread {
  turns: readonly ThreadTurn[];
  /** A project thread: each turn's diff and Edit from here. Plain chat: Fork only. */
  project: boolean;
  /**
   * A worktree thread: Edit from here stands in for the per-turn Undo
   * (restore) control, which is then not drawn. A local thread keeps Undo.
   */
  worktree: boolean;
  /** Why restoring the files is not offered, or null when Rust may allow it. */
  revertFilesBlocked: string | null;
  /**
   * A Fork or an Edit from this pane is on its way: its Fork and Edit
   * controls wait for the answer, so a second click cannot make a second one.
   */
  busy?: boolean;
  /**
   * The thread's newest run, as Rust stamped it. A run that started at the
   * same instant as a turn's `sentAt` is that turn's run (one `begin_turn`
   * writes both), so its `endedAt` is when that turn finished: the Threads
   * layout's "Worked for" and the time under the latest answer.
   */
  lastRun?: ThreadRun;
  /**
   * Shipping, for the latest changes card (protocol 37): Rust's last read in
   * a few words and the one next step. Absent before anything is known.
   */
  ship?: ThreadShipCardView;
  /**
   * The Away loop this thread runs as (protocol 45): its work rounds' turns,
   * their reviews and its report.
   */
  away?: ThreadAway;
  /**
   * The thread's provider was switched (protocol 59): its transcript holds
   * earlier sessions' turns, and only the current session's draw controls.
   */
  switched?: boolean;
}

/** What one matched user row offers. Built per row so the row memo compares values. */
export interface ThreadTurnControl {
  ordinal: number;
  /** Rust's measure of the turn ("+41 −12 · 3 files"), or "" when it has none. */
  change: string;
  /** Rust captured the turn's start: its diff can be shown. */
  diff: boolean;
  /** Edit from here is offered. */
  edit: boolean;
  /** A Fork or an Edit from this pane is on its way: Edit waits. */
  busy: boolean;
}
