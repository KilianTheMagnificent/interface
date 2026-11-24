import "./Conversation.css";
import type { VisualFrameLoader } from "../visual-reply";
import { useCallback, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import { useCommittedCallback, useStableValue } from "../use-stable-value";
import type {
  ContextUsage, PlanEntry, ProviderCommand, ProviderCommandCatalogue, ProviderCommandScope,
} from "../../shared/engine";
import type { PromptTurnImage } from "../../shared/prompt-images";
import {
  TRANSCRIPT_DENSITY_KEY, nextTranscriptDensities, parseTranscriptDensities,
  serializeTranscriptDensities, transcriptDensityFor, transcriptHasFoldableRows,
} from "../transcript-density";
import { echoPosition, echoSeen, mergeTranscript, newPendingEcho } from "../transcript-merge";
import { appliedDiffs, editedFilesStat } from "../tool-activity";
import { expandImportedRows } from "../transcript-import";
import { settleDetachedSubagents } from "../conversation-reducer";
import {
  APPROVAL_KEY_ATTR, keyedApprovalId, type ApprovalKeyVerdict, type ApprovalVerdicts,
} from "../approval-keys";
import type { ConvItem } from "../conversation-reducer";
import type { UsageHeadroom } from "../usage-view";
import type { PendingEcho } from "../transcript-merge";
import type { TranscriptDensity, TranscriptDensityMap } from "../transcript-density";
import type {
  ConversationThread, HistoryView, SessionConfigView, TurnRestorePoint,
} from "../conversation-types";
import type { DraftStore } from "../draft-store";
import type { DraftStashSummary } from "../../shared/draft-stash";
import type {
  PromptContextCapabilities, PromptContextItem, PromptContextKind, PromptTurnAttachment,
} from "../prompt-context";
import type { SteeringQueueItem, SteeringQueueMove } from "../steering-queue";
import { promptDropZone, type PromptDropPath } from "../prompt-drop";
import type { PastedFile } from "../prompt-paste";
import { dirtyFilesLabel } from "../diff-stats";
import { threadApprovalPending, threadDockApprovals, type ThreadApproval } from "../thread-frame";
import type { AwayCardAction } from "../away-thread";
import type { ShipNext } from "../thread-ship";
import { ConversationComposer } from "./ConversationComposer";
import {
  ConversationTranscript, type ConversationTranscriptControls,
} from "./ConversationTranscript";
import { ipcMessage } from "../ipc-message";
import { DropIndicator } from "./DropIndicator";
import { providerKindFromSession } from "./ProviderLogo";
import { ThreadApprovalDock } from "./ThreadApprovalDock";
import "../theme.css";

/**
 * REQUIRED OF THE CONSUMER — one of these routes, or a half-typed draft leaks across
 * sessions. The composer draft, the echoes of this window's own sends and the
 * auto-scroll pin are component-local and the pane cannot see a session switch on
 * its own: `onSend` is simply rebound to the newly selected session, so text typed
 * for session A would be sent to session B, and A's sent messages would go on
 * standing in B's transcript.
 *
 *   1. `<Conversation key={sessionKey} … />` — remounts the pane on every switch and
 *      drops the stale draft/scroll state (drafts do not survive a tab-back), or
 *   2. pass `sessionKey` below — the pane keeps its identity but clears the draft and
 *      re-pins the scroll itself whenever the value changes; or
 *   3. pass `sessionKey` plus a `drafts` store and the echo props — the shell keeps
 *      those values per session while this pane resets only view-local state.
 *
 * Passing both is fine (the key wins; the reset is then a no-op). Passing neither is
 * the bug. Item-level state (tool expansion, subagent folds) is keyed on
 * engine-minted ids and is safe either way; approval verdicts are not held here
 * at all — they belong to the caller's map, keyed by session AND approvalId.
 */
export interface ConversationProps {
  /** The LIVE stream only: events that arrived while this window has been open. */
  items: ConvItem[];
  /**
   * The session's past transcript and the state of the read for it — see the
   * contract above. Optional so the pane keeps working for a caller that has not
   * wired the read yet; omitted behaves as { status: "idle" }, which is exactly
   * the pre-history behaviour. A caller that CAN read history must pass it, or
   * selecting an existing session shows "No messages yet." until it is resumed.
   */
  history?: HistoryView;
  /** The provider's latest complete-replacement task plan, when it exposes one. */
  plan?: readonly PlanEntry[];
  /** Latest session-window occupancy. Omitted when the provider has not said. */
  context?: ContextUsage;
  /** Prior complete provider report, used only for an honest report-to-report change. */
  previousContext?: ContextUsage;
  /** Commands advertised by this exact live provider session. */
  commands?: readonly ProviderCommand[];
  /**
   * Pre-launch discovery, for the chat that exists but has never been resumed.
   * After a relaunch every session is in that state — the registry knows it,
   * no adapter is running, and nothing has advertised anything — so typing "/"
   * would otherwise face an empty menu until the first send goes out.
   *
   * Optional, and the pane asks nothing without it: a caller that omits this
   * (or `cwd`) keeps exactly the live-only behaviour. What comes back can only
   * WIDEN the menu — see effectiveProviderCommands for why it never narrows
   * what this pane is willing to send.
   */
  loadProviderCommands?: (scope: ProviderCommandScope) => Promise<ProviderCommandCatalogue>;
  /**
   * The session's own working directory, used only to scope that discovery —
   * a provider resolves its project settings from here, so the repo root
   * `pathRoot` shows paths against is the wrong directory to ask from.
   */
  cwd?: string;
  /**
   * The engine binary's reported version. ABSENT means the probe has not
   * answered or the engine did not say — the welcome block prints the engine's
   * name alone rather than a version it is guessing at.
   */
  engineVersion?: string;
  /**
   * The account plan this engine reports ("max", "pro"). ABSENT is the ordinary
   * case: the usage read is lazy and has usually not run.
   */
  planName?: string;
  /**
   * The provider's plan headroom (review E), said quietly on the model chip;
   * absent when nothing was measured.
   */
  headroom?: UsageHeadroom;
  /** Optional: renders a Retry button on the failure row. Re-runs the read. */
  onRetryHistory?: () => void;
  /** Optional: requests the next strict-superset page while older messages remain. */
  onLoadOlderHistory?: () => void;
  thinking: boolean;
  /** What the busy strip should say: "Thinking", a live tool, or "Working". */
  workingLabel?: string;
  /**
   * Epoch ms when this session's current turn started, owned by the shell so a
   * remount or pane switch cannot reset the elapsed clock.
   */
  turnStartedAt?: number;
  /**
   * True only for the workspace tile that owns keyboard focus. While true,
   * Cmd/Ctrl+F opens this pane's transcript find bar instead of browser find.
   * Omitted is false so multiple mounted conversations cannot all claim it.
   */
  focused?: boolean;
  /**
   * Identifies the session these `items`/callbacks belong to. Optional only so the
   * `key={sessionKey}` remount route stays valid — see the note above; omit it *only*
   * if the parent takes that route.
   */
  sessionKey?: string;
  /**
   * The shell's own strip, drawn directly above the composer on the transcript's
   * reading measure — the app's return recap is the one that uses it today.
   *
   * A SLOT RATHER THAN A SIBLING because the two positions are not equivalent:
   * anything the shell stacks BELOW this pane sits under the composer, which
   * reads as a footnote to the controls rather than as the last thing said
   * about the transcript. This pane owns the measure, so it is the only place
   * that can put a caller's line on it.
   */
  beforeComposer?: ReactNode;
  /**
   * Protocol 67: a card the shell docks with the approval dock, directly
   * above the composer (the private secret card).
   */
  dockCard?: ReactNode;
  /**
   * A line the shell puts inside the composer's top edge (owner, 2026-11-15):
   * the kept prompt. Pass a stable node (memoized by the caller), so a
   * streamed event does not redraw the composer.
   */
  composerTop?: ReactNode;
  /** A transport-owned composer, such as the input for an already running PTY.
   * Transcript rendering stays shared; the default session controls are omitted. */
  composer?: ReactNode;
  /**
   * Optional shell-owned drafts, one per session key. Supplying a store
   * preserves a half-written prompt when the workspace switches this pane to
   * another session, and lets a keystroke re-render the composer alone: the
   * draft is read from the store, never passed down as a prop. Without one the
   * draft is component-local and a session switch clears it.
   */
  drafts?: DraftStore;
  /**
   * Optional shell-owned local echoes. Engines do not stream user messages
   * back live, so these must survive a switch just like the draft does.
   */
  pendingEchoes?: readonly PendingEcho[];
  onPendingEcho?: (echo: PendingEcho) => void;
  onPendingEchoError?: (id: string, error: string) => void;
  /**
   * The absolute directory this session's paths are SHOWN relative to — its
   * repo root when it has one, otherwise its cwd. Purely cosmetic: it decides
   * whether a row prints `src/main/bridge.ts` or the 69-character absolute path
   * it came from, and the absolute string stays on every row's title and in the
   * find index regardless.
   *
   * Optional, and safe to omit: with no root every path renders exactly as it
   * did before this prop existed. It is per-session on purpose — two tiles show
   * two different sessions, so this can never be read from a global.
   */
  pathRoot?: string;
  /**
   * The agentKey of the subagent a caller-owned detail surface is currently
   * showing, or absent when it is showing none. Only the cell whose `item.id`
   * matches is painted selected, so a pane that is not the one being inspected
   * paints nothing — this is a per-session prop for the same reason `pathRoot`
   * is, and the shell must never derive it from a global.
   */
  selectedSubagentKey?: string;
  /**
   * Open a subagent in that surface. `viaKeyboard` is true when the activation
   * came from a key rather than a pointer, which is the caller's cue to move
   * focus into what it opened; a mouse user stays in the transcript they are
   * reading.
   *
   * OPTIONAL, and its absence is a working configuration rather than a broken
   * one: with no handler the cell keeps the expand-in-place behaviour it has
   * always had, so a partly wired build degrades to what shipped.
   */
  onSelectSubagent?: (agentKey: string, viaKeyboard: boolean) => void;
  /** A turn failed because the provider's CLI is not signed in: open its sign-in. */
  onSignIn?: () => void;
  /** False when the selected session has no live handle. */
  live: boolean;
  /** Model/effort state. The model control also renders while paused. */
  config: SessionConfigView;
  /** Typed context selected for the next direct or queued prompt. */
  contextItems?: readonly PromptContextItem[];
  /** Context captured by a send is temporarily immutable until validation settles. */
  contextLocked?: boolean;
  contextCapabilities?: PromptContextCapabilities;
  contextOpen?: boolean;
  onContextOpenChange?: (open: boolean) => void;
  onContextRequest?: (kind: PromptContextKind) => void;
  onContextRemove?: (id: string) => void;
  onContextPreview?: (item: PromptContextItem) => void;
  onPreviewAttachment?: (attachment: PromptTurnAttachment) => void;
  /** Reads and frames visual replies' pages (T3 section 8). App builds the
   * one stable instance; absent, a visual card says it could not be shown. */
  visualFrames?: VisualFrameLoader;
  /** Clear caller-owned context after, and only after, a prompt was accepted. */
  onContextConsumed?: (items: readonly PromptContextItem[]) => void;
  /** Paths dropped onto this conversation; the caller grants and attaches them. */
  onDroppedPaths?: (paths: readonly PromptDropPath[]) => void;
  /** Images pasted into this composer; the caller stores and attaches the bytes.
   *  Saved paths come back so a non-empty draft can insert them at the end. */
  onPastedImages?: (files: readonly PastedFile[]) =>
    void | Promise<readonly string[] | void>;
  /** Resolve a dropped File to a local path. Empty when the File has none. */
  pathForFile?: (file: { name: string }) => string;
  /** Caller-owned durable prompts waiting behind the active provider turn. */
  queueItems?: readonly SteeringQueueItem[];
  queueSendingId?: string | null;
  queueError?: string | null;
  /** A provider turn is still settling; queue edits remain available but Send now waits. */
  queueControlsLocked?: boolean;
  /** Provider configuration is changing; no prompt may leave under stale policy. */
  sendLocked?: boolean;
  /** Short visible/accessible explanation for a locked composer. */
  sendLockReason?: string;
  /** Defaults to `thinking`; callers may include approval-waiting provider states. */
  queueWhileBusy?: boolean;
  /**
   * This engine's adapter proved it can take text INTO a running turn
   * (EngineCapabilities.steer). Absent or false means no Steer control and no
   * Alt+Enter chord — the busy composer offers only the durable queue, which is
   * the truth for every ACP provider.
   */
  canSteer?: boolean;
  /** Explicit false means the bounded queue refused the prompt and the draft stays put. */
  onQueue?: (text: string) => boolean | void;
  onQueueMove?: (id: string, move: SteeringQueueMove) => boolean | void;
  onQueueEdit?: (id: string, text: string) => boolean | void;
  onQueueRemove?: (id: string) => boolean | void;
  onQueueSendNow?: (id: string) => void;
  onQueueClear?: () => boolean | void;
  /** A queued row's editor opened or closed. The caller holds delivery at an open one. */
  onQueueEditing?: (id: string, editing: boolean) => void;
  /**
   * Deliver the text. The returned promise is how a REFUSED send is told apart
   * from a delivered one: reject it (with an Error whose message is already a
   * sentence for a human) and the pane marks its own echo undelivered instead
   * of drawing a user turn that looks exactly like one that landed. A paused
   * known session may reconnect inside `iface.send`; deleted or otherwise
   * invalid sessions still reject before provider bytes leave Interface.
   * Returning nothing keeps the old fire-and-forget behaviour.
   */
  onSend: (text: string) => void | Promise<unknown>;
  /** Runs a provider command without serializing prompt-context attachments. */
  onRunCommand?: (text: string) => void | Promise<unknown>;
  onStop: () => void;
  /** A concrete provider model id; the UI never offers a default sentinel. */
  onSetModel: (model?: string) => void;
  /** null clears the effort flag. */
  onSetEffort: (effort: string | null) => void;
  /** Present only when the session's engine can flip fast mode mid-session. */
  onSetFastMode?: (enabled: boolean) => void;
  /** An approval item's `id` *is* its approvalId (see conversation-reducer.ts). */
  onApprove: (approvalId: string, answers?: Record<string, string>) => void;
  onDeny: (approvalId: string, note?: string) => void;
  /**
   * VERDICTS ALREADY SENT, from either surface that can answer one —
   * keyed by approvalVerdictKey(sessionKey, approvalId). The caller owns it,
   * and passing the SAME map to both is the whole contract: an approval card
   * with a private latch let the user allow in one, open the session, be
   * offered Allow/Deny again, click Deny, and read "Denied" for a command the
   * engine had already run — the bridge drops a verdict for a request that is
   * no longer outstanding with nothing but a console warning.
   *
   * Omitted, every card is unanswered and clicking says nothing, which is the
   * pre-map behaviour and is only correct for a caller that cannot answer.
   */
  approvals: ApprovalVerdicts;
  /** Approval ids the bridge may still answer; empty makes history read-only. */
  activeApprovalIds: ReadonlySet<string>;
  /**
   * An agent session's approvals are Interface's approval policy's to answer
   * first. `"checking"` draws the request with no verdict controls and no
   * chord, because the host refuses a verdict it has not handed to the owner;
   * `"owner"` and `null` (not an agent session) keep today's card.
   */
  approvalGate?: (approvalId: string) => "owner" | "checking" | null;
  /**
   * PER-TURN CHECKPOINT RESTORE — the caller's restore points, keyed by the
   * TRANSCRIPT ROW KEY of the user turn each one precedes. Those keys are
   * mergeTranscript's own (`h:user-12`, `l:user-3`, `e:7`); the caller already
   * holds the three lists it builds them from, so it can produce the same key
   * with one call to mergeTranscript.
   *
   * Omitted, or missing a turn's key, means NO CONTROL IS DRAWN for that turn —
   * which is the whole safety property here. The pane cannot tell whether a
   * checkpoint exists, so it never offers a rollback that would fail or, worse,
   * roll back to the wrong moment. Every user turn looks exactly as it did
   * before this prop existed until the caller says otherwise.
   *
   * Both this and `onRestoreTurn` are required for a control to appear. See
   * TurnRestorePoint for what the caller has to describe.
   */
  turnRestorePoints?: ReadonlyMap<string, TurnRestorePoint>;
  /**
   * Perform the restore for that row key. Called only after the user confirmed
   * in the app's ConfirmDialog, and only for a key the map above carried.
   *
   * The CALLER owns the outcome. This pane deliberately renders no success or
   * failure of its own: a restore rewrites files outside this transcript, so
   * what happened belongs on the app's own notice surface (the same one the
   * Review Center's restore already reports through), not in a chat bubble.
   */
  onRestoreTurn?: (turnKey: string) => void;
  /**
   * A restore already in flight, by the same key. Its control says so and stops
   * accepting clicks; every other turn's control is disabled too, because two
   * concurrent rollbacks of one working tree is not a state to offer.
   */
  restoringTurnKey?: string | null;
  /**
   * THREADS — this pane's Rust turn records and what its thread allows. The
   * pane maps its own user rows to them by prompt digest; a row that maps to
   * no turn gets no control. All four are optional: omitted (the phone, the
   * Agent surface, any non-thread pane), the pane is what it always was.
   */
  thread?: ConversationThread;
  /** Open this turn's diff (the change summary on a user turn). */
  onThreadTurnDiff?: (ordinal: number) => void;
  /** Preview a file the thread changed, beside it (review B5). */
  onThreadPreview?: (path: string) => void;
  /**
   * Edit from here: a new thread from just before turn `ordinal`, with
   * `prompt` in its composer. Rejects with the message to show (Rust refused).
   */
  onThreadEditFrom?: (ordinal: number, restoreFiles: boolean, prompt: string) => Promise<void>;
  /** Fork from here: a new thread keeping the first `keepTurns` prompts and their answers. */
  onThreadForkAt?: (keepTurns: number) => void;
  /**
   * Shipping (protocol 37): the latest changes card's next step, or "read"
   * when its ship row first shows. Absent, the card has no ship row.
   */
  onThreadShip?: (step: ShipNext | "read") => void;
  /** An Away loop's thread cards (protocol 45): stop, open a review, review the changes. */
  onThreadAway?: (action: AwayCardAction) => void;
  /**
   * A prompt the shell asks this pane to send, once per `id` (the Changes
   * panel's Fix failing checks): sent as Retry sends, with the pane's own echo
   * and refusal, or queued behind the running turn when the pane cannot send now.
   */
  threadSend?: { id: number; text: string };
  /**
   * Which row each matched turn's prompt is (ordinal → row key), reported
   * whenever that changes, so the shell can offer this pane's Undo for a turn
   * from outside the transcript (the diff panel).
   */
  onThreadTurnRows?: (rowKeys: ReadonlyMap<number, string>) => void;
  /**
   * Retry: send a failed turn's prompt again, text only, as a new send in this
   * thread. Rejects only on a definite refusal (the outbound lane's rule), and
   * the pane then marks its echo undelivered as it does for the composer.
   */
  onThreadRetry?: (prompt: string) => Promise<void>;
  onThreadRecover?: (prompt: string) => Promise<void>;
  /**
   * THE THREADS LAYOUT (owner, 2026-10-06 redesign): a pane in the Threads
   * grid draws its turns as folded work + answer + changes card, controls
   * once under the latest answer, and the composer's run settings as chips.
   * Absent (the Agent surface, the phone), the pane is what it always was.
   */
  threadLayout?: boolean;
  /**
   * THREADS — the thread's project, said in a docked request's footnote
   * ("in website · waiting 0:42"). Absent (a plain chat, or a caller that has
   * not passed it), the footnote is the waiting clock alone.
   */
  projectLabel?: string;
  /** Change this session's access mode from the composer's access chip. */
  onSetPermissionMode?: (mode: string) => void;
  /**
   * T3 section 9: this thread's stashed drafts (Rust's `draft-stash.v1.json`).
   * Present, the composer offers ⌘S and the + menu's "Stash draft" and draws
   * "Stashed (n)"; absent (not a thread, or no store), it offers none.
   */
  stash?: ConversationStash;
  /**
   * Protocol 67: `/btw <question>` in this thread's composer asks a side
   * question; answers why it was refused, or null. Absent: `/btw` is sent as
   * typed (not a thread).
   */
  onSideQuestion?: (question: string) => Promise<string | null>;
}

/** A thread's stashed drafts as its composer shows them (T3 section 9). */
export interface ConversationStash {
  /** Newest first; empty until read. */
  entries: readonly DraftStashSummary[];
  /** A stash command is on its way. */
  busy: boolean;
  /** Park the composer's draft and chips; the caller clears them once Rust has them. */
  onStash: () => void;
  /** Put a stash back; a non-empty composer is stashed first, in the same commit. */
  onRestore: (id: string) => void;
  onRemove: (id: string) => void;
}


// Conversation-only rules. They live here rather than in theme.css because that
// file belongs to the shell; everything colour-ish still comes from its tokens,
// so the pane re-themes with the rest of the app. Cursor's transcript language:
// flat assistant prose on the pane background, 22px borderless tool rows, one
// bordered card for the thing that is actually blocking (an approval), and a
// single composer box holding the textarea and its controls inside one border.

// Module-level so their identity is stable: a fresh `{}` or `[]` per render
// would make the memos below recompute forever.
const IDLE_HISTORY: HistoryView = { status: "idle" };
const NO_ITEMS: ConvItem[] = [];
const NO_ECHOES: readonly PendingEcho[] = [];
const NO_DOCK: readonly ThreadApproval[] = [];

/* localStorage never throws here: a blocked or full store just means the dial
   does not survive a reload, which is not worth a message. Same rule the
   sidebar's collapse memory follows. */
function readTranscriptDensities(): TranscriptDensityMap {
  try {
    return parseTranscriptDensities(window.localStorage.getItem(TRANSCRIPT_DENSITY_KEY));
  } catch {
    return {};
  }
}

function writeTranscriptDensities(densities: TranscriptDensityMap): void {
  try {
    window.localStorage.setItem(TRANSCRIPT_DENSITY_KEY,
      serializeTranscriptDensities(densities));
  } catch {
    // Quota, privacy mode, or no window at all (the project's node tests).
  }
}

/**
 * Pairs each open queue-editor `id` with the `onQueueEditing` callback of the
 * session that opened it, so unmounting `<SteeringQueue key={sessionKey}>` when
 * `Conversation` switches `A -> B` clears `A`'s edit lock after `committed`
 * has already moved to `B` in `useLayoutEffect`.
 */
export function createQueueEditingForwarder(
  latest: () => ((id: string, editing: boolean) => void) | undefined,
): (id: string, editing: boolean) => void {
  const openQueueEdits: Array<{
    id: string;
    close: ((id: string, editing: boolean) => void) | undefined;
  }> = [];
  return (id: string, editing: boolean): void => {
    if (editing) {
      const close = latest();
      openQueueEdits.push({ id, close });
      close?.(id, true);
      return;
    }
    const index = openQueueEdits.findIndex(entry => entry.id === id);
    const close = index >= 0
      ? openQueueEdits.splice(index, 1)[0]?.close
      : latest();
    close?.(id, false);
  };
}

/**
 * Stable stand-ins for the caller's callbacks. Each forwards to the prop the
 * pane last committed, which is what a handler bound during that render would
 * have called, so the two memoized halves keep equal props across a caller
 * that mints new callbacks on every render (App's tiles do). Where a callback's
 * PRESENCE changes what is drawn, the shell passes the stand-in only while the
 * caller's own is present.
 */
function usePaneCallbacks(latest: () => ConversationProps) {
  const [forward] = useState(() => {
    const forwardQueueEditing = createQueueEditingForwarder(() => latest().onQueueEditing);
    return {
    onStop: (): void => { latest().onStop(); },
    onSetModel: (model?: string): void => { latest().onSetModel(model); },
    onSetEffort: (effort: string | null): void => { latest().onSetEffort(effort); },
    onSetFastMode: (enabled: boolean): void => { latest().onSetFastMode?.(enabled); },
    onApprove: (approvalId: string, answers?: Record<string, string>): void => {
      latest().onApprove(approvalId, answers);
    },
    onDeny: (approvalId: string, note?: string): void => { latest().onDeny(approvalId, note); },
    onSelectSubagent: (agentKey: string, viaKeyboard: boolean): void => {
      latest().onSelectSubagent?.(agentKey, viaKeyboard);
    },
    onSignIn: (): void => { latest().onSignIn?.(); },
    onRestoreTurn: (turnKey: string): void => { latest().onRestoreTurn?.(turnKey); },
    onThreadTurnDiff: (ordinal: number): void => { latest().onThreadTurnDiff?.(ordinal); },
    onThreadPreview: (path: string): void => { latest().onThreadPreview?.(path); },
    onThreadEditFrom: (ordinal: number, restoreFiles: boolean, prompt: string): Promise<void> =>
      latest().onThreadEditFrom?.(ordinal, restoreFiles, prompt) ?? Promise.resolve(),
    onThreadForkAt: (keepTurns: number): void => { latest().onThreadForkAt?.(keepTurns); },
    onThreadShip: (step: ShipNext | "read"): void => { latest().onThreadShip?.(step); },
    onThreadAway: (action: AwayCardAction): void => { latest().onThreadAway?.(action); },
    onThreadTurnRows: (rowKeys: ReadonlyMap<number, string>): void => {
      latest().onThreadTurnRows?.(rowKeys);
    },
    onSetPermissionMode: (mode: string): void => { latest().onSetPermissionMode?.(mode); },
    onRetryHistory: (): void => { latest().onRetryHistory?.(); },
    onLoadOlderHistory: (): void => { latest().onLoadOlderHistory?.(); },
    onQueue: (text: string): boolean | void => latest().onQueue?.(text),
    onQueueMove: (id: string, move: SteeringQueueMove): boolean | void =>
      latest().onQueueMove?.(id, move),
    onQueueEdit: (id: string, text: string): boolean | void => latest().onQueueEdit?.(id, text),
    onQueueRemove: (id: string): boolean | void => latest().onQueueRemove?.(id),
    onQueueSendNow: (id: string): void => { latest().onQueueSendNow?.(id); },
    onQueueClear: (): boolean | void => latest().onQueueClear?.(),
    onQueueEditing: forwardQueueEditing,
    onRunCommand: (text: string): void | Promise<unknown> => latest().onRunCommand?.(text),
    onContextOpenChange: (open: boolean): void => { latest().onContextOpenChange?.(open); },
    onContextRequest: (kind: PromptContextKind): void => { latest().onContextRequest?.(kind); },
    onContextRemove: (id: string): void => { latest().onContextRemove?.(id); },
    onContextPreview: (item: PromptContextItem): void => { latest().onContextPreview?.(item); },
    onPreviewAttachment: (attachment: PromptTurnAttachment): void => { latest().onPreviewAttachment?.(attachment); },
    onPastedImages: (files: readonly PastedFile[]): void | Promise<readonly string[] | void> =>
      latest().onPastedImages?.(files),
    loadProviderCommands: (scope: ProviderCommandScope): Promise<ProviderCommandCatalogue> =>
      latest().loadProviderCommands?.(scope)
        ?? Promise.reject(new Error("Provider command discovery is not available.")),
    onSideQuestion: (question: string): Promise<string | null> =>
      latest().onSideQuestion?.(question)
        ?? Promise.resolve("Side questions are not available in this pane."),
  };
  });
  return forward;
}

// Presentational only: no window.iface, no store. Every effect leaves through a
// callback prop, so the caller owns IPC and the pane stays readable by inspection.
//
// THE SHELL. It holds what both halves read — the merged transcript rows, this
// window's own echoes, the density dial — and the drop zone over the whole
// pane, and draws two memoized halves: ConversationTranscript and
// ConversationComposer. A keystroke reaches only the composer (its draft comes
// from `drafts`), and a streamed token reaches this shell and the transcript
// but not the composer, whose every prop is stable across one.
export function Conversation(props: ConversationProps) {
  const { items, live, sessionKey } = props;
  // The props this pane last committed, for the stand-ins above and for a send
  // that must keep its callbacks past its own event.
  const committed = useRef(props);
  useLayoutEffect(() => { committed.current = props; });
  const latest = useCallback((): ConversationProps => committed.current, []);
  const forward = usePaneCallbacks(latest);
  const rootRef = useRef<HTMLDivElement>(null);
  const transcript = useRef<ConversationTranscriptControls>(null);
  // Quote (T3 section 9): a reply's selection goes into this pane's draft
  // through the composer, which owns the field and the caret.
  const quoteInto = useRef<((text: string) => void) | null>(null);
  const [quoteReply] = useState(() => (text: string): void => { quoteInto.current?.(text); });
  // Through the subagent cards, not just across the top level: a spawned agent's
  // edits are this session's edits, which is the same rule turnChangeSummaries
  // applies to the per-turn chip drawn a few rows above this line. A declined
  // or failed edit changed nothing, so only `appliedDiffs` count here too.
  // Kept by identity while the diffs are the same ones, so a streamed token does
  // not recount the pill.
  const dirtyItems = useStableValue(useMemo(() => appliedDiffs(items), [items]));
  const dirtyLabel = useMemo(() => dirtyFilesLabel(
    new Set(dirtyItems.map(item => item.path)).size,
    editedFilesStat(dirtyItems),
  ), [dirtyItems]);
  const history = props.history ?? IDLE_HISTORY;
  const past = history.status === "ready" ? history.items : NO_ITEMS;
  // Is `past` the WHOLE past, or just what has arrived so far? "idle" means
  // there is nothing to read (the session was started in this app run and is
  // already whole in `items`), so empty is the complete answer there. "loading"
  // and "error" both leave `past` empty over a transcript that exists, and an
  // echo frozen against that must not be cancelled by it later — see
  // PendingEcho.historyKnown.
  const pastKnown = history.status === "ready" || history.status === "idle"
    || history.status === "unavailable";
  // Messages THIS window sent. No engine ever sends them back — `user.message`
  // exists only in a replayed transcript — so without this the same sentence is
  // visible after a reload and invisible at the moment it is typed.
  // mergeTranscript drops any that the transcript itself has since supplied.
  //
  // KEYED BY SESSION, and deliberately not cleared on a switch: the agent's
  // reply survives a tab away and back (the caller's store keeps `items`), so
  // the question that prompted it has to survive as well, or returning to a
  // session shows an answer to nothing. A caller taking the `key={sessionKey}`
  // remount route drops the whole map instead, exactly as it drops the draft.
  const [recoveryError, setRecoveryError] = useState<string | null>(null);
  const [sent, setSent] = useState<Record<string, readonly PendingEcho[]>>({});
  const echoKey = sessionKey ?? "";
  const echoes = props.pendingEchoes ?? sent[echoKey] ?? NO_ECHOES;
  const nextEcho = useRef(0);
  // The density dial, per session. Read at mount for what this pane draws, and
  // re-read at every write — see chooseDensity. `echoKey` is the same ""
  // fallback the echo list uses, so a pane on the remount route keeps a working
  // control whose choice is deliberately not persisted.
  const [densities, setDensities] = useState<TranscriptDensityMap>(readTranscriptDensities);
  const density = transcriptDensityFor(densities, echoKey);
  // Bumped by every press, INCLUDING a press on the step already showing. The
  // dial is a mode AND an action, and "fold all of this back up" is the action
  // the strip exists for: on the shipped default the step the user needs is
  // already the pressed one, so a control that only acts when the VALUE changes
  // offers no collapse-all at all from the state every session starts in.
  const [densityEpoch, setDensityEpoch] = useState(0);
  const chooseDensity = useCommittedCallback((next: TranscriptDensity): void => {
    // Merged over what is on disk AT THE CLICK rather than over this pane's
    // mount-time copy. The tiled workspace is the case this dial exists for,
    // and every open tile is a separate instance of this component holding its
    // own snapshot of one shared key: writing that snapshot back erases
    // whatever the tile beside it chose after this one mounted.
    const updated = nextTranscriptDensities(readTranscriptDensities(), echoKey, next);
    setDensities(updated);
    setDensityEpoch(epoch => epoch + 1);
    writeTranscriptDensities(updated);
  });
  // expandImportedRows runs over the MERGED list, not over either half: an
  // imported session's markers are prose, and splitting them before the merge
  // would mint tool ids that isEngineId cannot tell from an engine's own.
  const rows = useMemo(() => expandImportedRows(mergeTranscript(
    live ? past : settleDetachedSubagents(past),
    live ? items : settleDetachedSubagents(items),
    echoes,
  )), [past, items, echoes, live]);
  const foldable = useMemo(() => transcriptHasFoldableRows(rows), [rows]);
  // Split once per render: the checking cards, and the requests the owner may
  // actually answer — the only ones that may hold the chord or the capsule.
  const checkingApprovalIds = new Set<string>();
  let ownerApprovalIds = props.activeApprovalIds;
  if (props.approvalGate !== undefined) {
    for (const id of props.activeApprovalIds) {
      if (props.approvalGate(id) === "checking") checkingApprovalIds.add(id);
    }
    if (checkingApprovalIds.size > 0) {
      ownerApprovalIds = new Set([...props.activeApprovalIds].filter(id => !checkingApprovalIds.has(id)));
    }
  }
  // By content: the shipped caller hands over a fresh Set (and a fresh
  // capability object, and a fresh previous context report) every render, and
  // a caller that rebuilds its context list must not redraw the composer.
  const activeApprovalIds = useStableValue(props.activeApprovalIds);
  const stableCheckingApprovalIds = useStableValue(checkingApprovalIds);
  const stableOwnerApprovalIds = useStableValue(ownerApprovalIds);
  // THE DOCK (Threads only, owner 2026-10-07): every request this pane holds
  // open, in transcript order, drawn above the composer and never in the
  // transcript. Walked only while the engine lists a request at all.
  const threadLayout = props.threadLayout === true;
  const dockItems = useStableValue(useMemo(() => threadLayout && live && activeApprovalIds.size > 0
    ? threadDockApprovals(rows.flatMap(row => (row.kind === "item" ? [row.item] : [])),
      activeApprovalIds)
    : NO_DOCK, [threadLayout, live, rows, activeApprovalIds]));
  // Blocked on the owner: the composer and the dock wear amber on this alone.
  const approvalPending = dockItems.length > 0 && threadApprovalPending(dockItems,
    stableOwnerApprovalIds, props.approvals, sessionKey ?? "");
  // The card the chord answers — the transcript's rule (keyedApprovalId) over
  // the same live items, so the docked card wears the keys exactly where the
  // pane's listener will click.
  const dockKeyedId = dockItems.length > 0 && props.focused === true
    ? keyedApprovalId(items, live, stableOwnerApprovalIds, props.approvals, sessionKey ?? "")
    : null;
  // An empty composer's ⌘↵ / ⌘⌫ press the docked card's own keyed button, the
  // one the chord clicks, so the key runs the click's handler, latch and note
  // flow. False when no keyed button took it, and the key does what it did.
  const answerDockedApproval = useCallback((verdict: ApprovalKeyVerdict): boolean => {
    const buttons = rootRef.current?.querySelectorAll<HTMLButtonElement>(
      `.thread-dock button[${APPROVAL_KEY_ATTR}="${verdict}"]`);
    const button = buttons?.[buttons.length - 1];
    if (button === undefined || button.disabled) return false;
    button.click();
    return true;
  }, []);
  const contextItems = useStableValue(props.contextItems);
  const contextCapabilities = useStableValue(props.contextCapabilities);
  const context = useStableValue(props.context);
  const previousContext = useStableValue(props.previousContext);
  // A resume mounts on history "loading" / "error" / "unavailable" before any
  // items exist. Those are not brand-new chats — locking them reopens the
  // "Choose a model" banner on every restore. Idle + empty is the only new session.
  const sessionHasTranscript = (history.status === "ready" && history.items.length > 0)
    || history.status === "loading"
    || history.status === "error"
    || history.status === "unavailable"
    || props.items.length > 0
    || echoes.length > 0;
  const awayRunning = props.thread?.away?.state === "running"
    || props.thread?.away?.state === "stopping";
  const modelRequired = !sessionHasTranscript
    && props.config.canSetModel
    && (props.config.model === undefined || props.config.model.trim() === ""
      || props.config.model === "default");
  const composerLocked = props.sendLocked === true || awayRunning || modelRequired;

  // One echo per direct send, minted here because the echo list is the
  // shell's. `seen` freezes how many identical user turns were ALREADY
  // visible, `at` freezes WHERE the message goes (above the reply it is about
  // to get), and `historyKnown` says whether `seen` counted a real transcript
  // or an empty stand-in. The function handed back marks this echo undelivered
  // on the session it was sent from, through the callback that session had.
  const postEcho = useCommittedCallback((text: string,
      images: readonly PromptTurnImage[],
      attachments?: readonly PromptTurnAttachment[]): ((why: string) => void) => {
    // A shell-owned echo list can outlive this component instance. Advance
    // past ids restored after a pane switch before minting the next one.
    while (echoes.some(e => e.id === String(nextEcho.current))) nextEcho.current += 1;
    const id = String(nextEcho.current++);
    const echo = newPendingEcho({
      id, text, seen: echoSeen(text, past, items, echoes),
      at: echoPosition(items), historyKnown: pastKnown, images, attachments,
    });
    const key = echoKey;
    const onPendingEchoError = props.onPendingEchoError;
    if (props.onPendingEcho !== undefined) props.onPendingEcho(echo);
    else setSent(map => ({ ...map, [echoKey]: [...(map[echoKey] ?? NO_ECHOES), echo] }));
    transcript.current?.followLatest();
    return why => {
      if (onPendingEchoError !== undefined) {
        onPendingEchoError(id, why);
      } else {
        setSent(map => ({
          ...map,
          [key]: (map[key] ?? NO_ECHOES).map(
            e => (e.id === id ? { ...e, error: why } : e)),
        }));
      }
    };
  });
  // Recovery is receipt-bound. The owner sees a new continuation instruction;
  // never echo or replay the old transcript text as a fresh prompt here.
  const retryThreadTurn = useCommittedCallback((prompt: string): Promise<boolean> => {
    setRecoveryError(null);
    const send = props.onThreadRecover;
    if (send === undefined) return Promise.resolve(false);
    return Promise.resolve(send(prompt)).then(() => true, (err: unknown) => {
      const said = ipcMessage(err);
      setRecoveryError(said !== "" ? said : "Recovery could not be prepared.");
      return false;
    });
  });
  const canRetry = props.onThreadRecover !== undefined && !composerLocked
    && props.queueWhileBusy !== true && props.thinking !== true;
  // The shell's send request: each new id once, now or behind the running turn.
  const threadSend = props.threadSend;
  const threadSendSeen = useRef(threadSend?.id ?? 0);
  const sendShellPrompt = useCommittedCallback((text: string): void => {
    if (props.onThreadRetry !== undefined && !composerLocked && props.queueWhileBusy !== true && props.thinking !== true) {
      const refuseEcho = postEcho(text, []);
      void props.onThreadRetry(text).catch((failure: unknown) => { refuseEcho(ipcMessage(failure)); });
    } else props.onQueue?.(text);
  });
  useLayoutEffect(() => {
    if (threadSend === undefined || threadSend.id === threadSendSeen.current) return;
    threadSendSeen.current = threadSend.id;
    sendShellPrompt(threadSend.text);
  }, [threadSend, sendShellPrompt]);
  // The transcript's pin, after a commit only this shell or the composer made.
  const keepPinned = useCallback((): void => { transcript.current?.pin(); }, []);
  useLayoutEffect(() => { keepPinned(); });
  // Files dropped anywhere on the pane attach to the next prompt, the same
  // drop zone the launch card has; see promptDropZone.
  const [dropHover, setDropHover] = useState(0);
  const dropZone = promptDropZone({
    enabled: !composerLocked && props.contextLocked !== true,
    pathForFile: props.pathForFile,
    onDroppedPaths: props.onDroppedPaths,
    onPastedImages: props.onPastedImages,
    setHover: setDropHover,
  });

  const dock = dockItems.length === 0 ? null : (
    <ThreadApprovalDock
      items={dockItems}
      live={live}
      activeApprovalIds={activeApprovalIds}
      checkingApprovalIds={stableCheckingApprovalIds}
      approvals={props.approvals}
      sessionKey={sessionKey ?? ""}
      keyedApprovalId={dockKeyedId}
      engineLabel={props.config.engineLabel}
      pathRoot={props.pathRoot}
      projectLabel={props.projectLabel}
      onApprove={forward.onApprove}
      onDeny={forward.onDeny}
    />
  );

  return (
    <div className={`conv${props.threadLayout === true ? " is-thread-layout" : ""}${
      dropHover > 0 ? " is-drop-target" : ""}`}
      data-provider={providerKindFromSession(props.config.engine, sessionKey)}
      ref={rootRef} tabIndex={-1}
      aria-keyshortcuts={props.focused === true ? "Meta+F Control+F" : undefined}
      {...dropZone}>
      {dropHover > 0 && (
        <DropIndicator label="Drop files to attach" />
      )}
      <ConversationTranscript
        items={items}
        history={props.history}
        plan={props.plan}
        thinking={props.thinking}
        workingLabel={props.workingLabel}
        turnStartedAt={props.turnStartedAt}
        focused={props.focused}
        sessionKey={sessionKey}
        live={live}
        config={props.config}
        engineVersion={props.engineVersion}
        planName={props.planName}
        cwd={props.cwd}
        pathRoot={props.pathRoot}
        selectedSubagentKey={props.selectedSubagentKey}
        onSelectSubagent={props.onSelectSubagent === undefined
          ? undefined : forward.onSelectSubagent}
        onSignIn={props.onSignIn === undefined ? undefined : forward.onSignIn}
        approvals={props.approvals}
        activeApprovalIds={activeApprovalIds}
        checkingApprovalIds={stableCheckingApprovalIds}
        ownerApprovalIds={stableOwnerApprovalIds}
        turnRestorePoints={props.turnRestorePoints}
        onRestoreTurn={props.onRestoreTurn === undefined ? undefined : forward.onRestoreTurn}
        restoringTurnKey={props.restoringTurnKey}
        thread={props.thread}
        onThreadTurnDiff={props.onThreadTurnDiff === undefined ? undefined : forward.onThreadTurnDiff}
        onThreadPreview={props.onThreadPreview === undefined ? undefined : forward.onThreadPreview}
        onPreviewAttachment={props.onPreviewAttachment === undefined ? undefined : forward.onPreviewAttachment}
        visualFrames={props.visualFrames}
        onThreadEditFrom={props.onThreadEditFrom === undefined ? undefined : forward.onThreadEditFrom}
        onThreadForkAt={props.onThreadForkAt === undefined ? undefined : forward.onThreadForkAt}
        onThreadShip={props.onThreadShip === undefined ? undefined : forward.onThreadShip}
        onThreadAway={props.onThreadAway === undefined ? undefined : forward.onThreadAway}
        onThreadTurnRows={props.onThreadTurnRows === undefined ? undefined : forward.onThreadTurnRows}
        retryThreadTurn={canRetry ? retryThreadTurn : undefined}
        threadLayout={props.threadLayout === true}
        onRetryHistory={props.onRetryHistory === undefined ? undefined : forward.onRetryHistory}
        onLoadOlderHistory={props.onLoadOlderHistory === undefined
          ? undefined : forward.onLoadOlderHistory}
        onApprove={forward.onApprove}
        onDeny={forward.onDeny}
        onStop={forward.onStop}
        queueItems={props.queueItems}
        queueSendingId={props.queueSendingId}
        queueError={props.queueError}
        queueControlsLocked={props.queueControlsLocked}
        onQueueMove={props.onQueueMove === undefined ? undefined : forward.onQueueMove}
        onQueueEdit={props.onQueueEdit === undefined ? undefined : forward.onQueueEdit}
        onQueueRemove={props.onQueueRemove === undefined ? undefined : forward.onQueueRemove}
        onQueueSendNow={props.onQueueSendNow === undefined ? undefined : forward.onQueueSendNow}
        onQueueClear={props.onQueueClear === undefined ? undefined : forward.onQueueClear}
        onQueueEditing={forward.onQueueEditing}
        rows={rows}
        density={density}
        densityEpoch={densityEpoch}
        rootRef={rootRef}
        controls={transcript}
        onQuoteReply={props.composer === undefined ? quoteReply : undefined}
      />

      {props.composer !== undefined ? <>{dock}{props.dockCard}{props.composer}</> : (
        <div className="conv-strip">
          {/* The shell's line, on the transcript's own measure — see
              ConversationProps.beforeComposer. Above the dirty pill because
              both are about what just happened, and this one is the older
              news of the two. */}
          {props.beforeComposer}
          {recoveryError && <p role="alert">{recoveryError}</p>}
          {/* What blocks the turn, directly above the hands that answer it. */}
          {dock}
          {props.dockCard}
          <ConversationComposer
            drafts={props.drafts}
            sessionKey={sessionKey}
            thinking={props.thinking}
            live={live}
            config={props.config}
            commands={props.commands}
            loadProviderCommands={props.loadProviderCommands === undefined
              ? undefined : forward.loadProviderCommands}
            cwd={props.cwd}
            planName={props.planName}
            headroom={props.headroom}
            context={context}
            previousContext={previousContext}
            contextItems={contextItems}
            contextLocked={props.contextLocked}
            contextCapabilities={contextCapabilities}
            contextOpen={props.contextOpen}
            onContextOpenChange={props.onContextOpenChange === undefined
              ? undefined : forward.onContextOpenChange}
            onContextRequest={props.onContextRequest === undefined
              ? undefined : forward.onContextRequest}
            onContextRemove={props.onContextRemove === undefined
              ? undefined : forward.onContextRemove}
            onContextPreview={props.onContextPreview === undefined
              ? undefined : forward.onContextPreview}
            onPastedImages={props.onPastedImages === undefined
              ? undefined : forward.onPastedImages}
            queueSendingId={props.queueSendingId}
            queueControlsLocked={props.queueControlsLocked}
            sendLocked={props.sendLocked === true || awayRunning}
            sendLockReason={awayRunning
              ? "Away loop is running — stop Away before sending." : props.sendLockReason}
            queueWhileBusy={awayRunning ? false : props.queueWhileBusy}
            canSteer={props.canSteer}
            onQueue={awayRunning || props.onQueue === undefined ? undefined : forward.onQueue}
            onRunCommand={props.onRunCommand === undefined ? undefined : forward.onRunCommand}
            onStop={forward.onStop}
            onSetModel={forward.onSetModel}
            onSetEffort={forward.onSetEffort}
            onSetFastMode={props.onSetFastMode === undefined ? undefined : forward.onSetFastMode}
            threadLayout={props.threadLayout === true}
            approvalPending={approvalPending}
            onAnswerApproval={dockKeyedId === null ? undefined : answerDockedApproval}
            onSetPermissionMode={props.onSetPermissionMode === undefined
              ? undefined : forward.onSetPermissionMode}
            latest={latest}
            dirtyLabel={dirtyLabel}
            modelRequired={modelRequired}
            foldable={foldable}
            density={density}
            onChooseDensity={chooseDensity}
            postEcho={postEcho}
            afterCommit={keepPinned}
            quoteInto={quoteInto}
            stash={props.stash}
            composerTop={props.composerTop}
            onSideQuestion={props.onSideQuestion === undefined ? undefined : forward.onSideQuestion}
          />
        </div>
      )}
    </div>
  );
}

// mergeTranscript, TranscriptRow and the echo rule live in ../transcript-merge
// beside the reducer whose output they order. They are pure functions over
// ConvItem with no DOM in them, and what is on screen is decided there — which
// is where the tests for it are (conversation.test.ts). Nothing else imported
// them from here.
