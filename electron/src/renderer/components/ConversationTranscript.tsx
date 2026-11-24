import {
  memo, useCallback, useEffect, useImperativeHandle, useLayoutEffect, useMemo, useRef, useState,
} from "react";
import type { ReactNode, RefObject } from "react";
import {
  collapseSuccessfulToolRuns, displayRowKeys, groupSubagentGrids, retainDisplayRows,
  type ConversationDisplayRow,
} from "../conversation-display";
import { useCommittedCallback, useStableValue } from "../use-stable-value";
import {
  APPROVAL_KEY_ATTR, approvalChordStep, approvalKeyTyping, approvalKeyVerdict, approvalVerdictKey,
  keyedApprovalId,
} from "../approval-keys";
import {
  buildTranscriptSearchCorpus, moveTranscriptSearchIndex, searchTranscriptCorpus,
  TRANSCRIPT_SEARCH_MAX_QUERY_CHARS, transcriptSearchStatus,
} from "../transcript-search";
import { collectRunningSubagents, type ConvItem } from "../conversation-reducer";
import {
  diffPathsByToolRow, findSubagentItem, turnChangeSummaries, type TurnChangeSummary,
} from "../tool-activity";
import {
  jumpLatestBacklog, jumpLatestLabel, jumpScrollBehavior, prefersReducedMotion, promptAnchorKey,
  promptAnchorView, transcriptPrompts, truncationNotice, type PromptAnchorSpot,
} from "../transcript-navigation";
import {
  toolRunMinimum, transcriptDensityOpensItems, type TranscriptDensity,
} from "../transcript-density";
import type { TranscriptRow } from "../transcript-merge";
import type { HistoryView, ThreadTurnControl } from "../conversation-types";
import type { ShipNext } from "../thread-ship";
import {
  headStretchPromptRows, matchThreadTurns, threadPromptRows, threadRetryKey, threadTurnChangeText,
  threadTurnEnds, threadTurnRowKeys, transcriptWasCompacted,
} from "../thread-turns";
import { threadHandoffLine } from "../thread-opinion";
import type { ThreadTurn } from "../../shared/threads";
import {
  rememberThreadTurnTiming, THREAD_RUN_MATCH_MS, threadChangesView, threadDisplayKey,
  threadDisplayPlan, threadDrawnKeys, threadRunFor, threadStampKeys, threadTranscriptPlan,
  threadTurnTiming, threadWorkLead, type ThreadDisplayPlan, type ThreadTurnMatch,
} from "../thread-transcript";
import { threadLiveTurn, type ThreadTrailRow } from "../thread-live-turn";
import {
  conversationOwnsFindShortcut, finishedTurnAnnouncement, turnRestoreConfirmation,
} from "../conversation-pane-rules";
import {
  normalizeSteeringQueue, type SteeringQueueItem, type SteeringQueueMove,
} from "../steering-queue";
import { stampTurnClock } from "../session-activity";
import { sessionStatusModel, welcomeLines } from "../session-status";
import { ConfirmDialog } from "./ConfirmDialog";
import { ThreadEditDialog } from "./ThreadEditDialog";
import {
  EditedFilesRow, FileChangeRow, LiveTurnElapsed, mediaQuery, SearchableItem, SubagentGrid,
  ToolRun, TranscriptSkeleton, TurnClock, useDocumentChrome, type ConversationItemContext,
  type FindRenderContext,
} from "./ConversationRows";
import { Icon } from "./glyphs";
import type { ConversationProps } from "./Conversation";
import { ProductName } from "./ProductName";
import { ProviderLogo, providerKindFromSession } from "./ProviderLogo";
import { SteeringQueue } from "./SteeringQueue";
import { WorkingIndicator } from "./WorkingIndicator";
import {
  ThreadChangesCard, ThreadStamp, ThreadTurnFoot, ThreadUserMeta, ThreadWorkFold,
} from "./ThreadTurnParts";
import { AwayReportCard, AwayReviewCard, AwayRoundPrompt, ThreadHandoffPrompt } from "./ThreadAwayParts";
import { scheduledPromptFold } from "../thread-schedules";
import { awayReport, awayReviewerOf, awayTurnCards, type AwayCardAction, type AwayTurnCard } from "../away-thread";
import {
  ThreadLivePill, ThreadLiveTurn, ThreadStepList, useThreadFinishBeat, useThreadFoldMotion,
} from "./ThreadLiveTurn";
// After ConversationRows, whose import brings Conversation.css and theme.css:
// the Threads layout restyles their rules at equal or higher specificity, so
// it has to come later in the cascade.
import "./ThreadTranscript.css";

/**
 * THE TRANSCRIPT HALF OF THE PANE, memoized. It owns everything the reader
 * scrolls and searches: the display-row pipeline, the find bar and its
 * Cmd/Ctrl+F listener, the approval chord, the render-phase session-switch
 * reset and the layout-effect pin it feeds, the jump capsule, the prompt
 * anchor, the running-subagent and working strips, the queue slot, the turn
 * clock and the restore dialog.
 *
 * Every prop is identity-stable across a keystroke (the composer reads its
 * draft from a store) and across anything only the composer shows, so neither
 * redraws a single row. Callbacks arrive as the shell's stable forwarders.
 *
 * The composer below it changes the transcript in exactly two ways, both
 * through `controls`: a send pulls the view back to the newest line
 * (`followLatest`), and any commit that may have resized the composer re-runs
 * the pin (`pin`), exactly as the one-component pane re-ran it on every commit.
 */

// Module-level so their identity is stable: a fresh `{}` or `[]` per render
// would make the memo below recompute forever.
const IDLE_HISTORY: HistoryView = { status: "idle" };
const NO_ITEMS: ConvItem[] = [];
const NO_QUEUE: readonly SteeringQueueItem[] = [];
const NO_APPROVAL_IDS: ReadonlySet<string> = new Set<string>();
const NO_DISPLAY_ROWS: readonly ConversationDisplayRow[] = [];
const NO_DRAWN_KEYS: readonly (readonly string[])[] = [];
const NO_QUEUE_MOVE = (_id: string, _move: SteeringQueueMove): void => undefined;
const NO_QUEUE_TEXT = (_id: string, _text: string): void => undefined;
const NO_QUEUE_ID = (_id: string): void => undefined;
const NO_QUEUE_CLEAR = (): void => undefined;
const NO_THREAD_FOLDS: ReadonlyMap<string, boolean> = new Map();
const NO_ROWS: readonly TranscriptRow[] = [];

type TranscriptFindState = { open: boolean; query: string; index: number };
const CLOSED_FIND: TranscriptFindState = { open: false, query: "", index: 0 };
const EMPTY_FIND_CORPUS = { entries: [], truncated: false } as const;

/**
 * The DOM half of conversationOwnsFindShortcut, read once per window-level keydown: does this
 * pane own this press? Both listeners that bind at the window — Find and the
 * approval chord — ask it, so the two cannot drift into scoping themselves
 * differently.
 */
function paneOwnsShortcut(root: HTMLElement, event: KeyboardEvent): boolean {
  const target = event.target;
  const active = document.activeElement;
  const targetNeutral = target === window || target === document
    || target === document.body || target === document.documentElement;
  const activeNeutral = active === null || active === document.body
    || active === document.documentElement;
  return conversationOwnsFindShortcut(
    target instanceof Node && root.contains(target),
    active instanceof Node && root.contains(active),
    targetNeutral, activeNeutral);
}

/** What the composer may ask of the transcript. */
export interface ConversationTranscriptControls {
  /** Your own message always pulls the view back down; see followLatest. */
  followLatest: () => void;
  /** Re-run the pin after a commit that only the composer or the shell made. */
  pin: () => void;
}

export type ConversationTranscriptProps = Pick<ConversationProps,
  | "items" | "history" | "plan" | "thinking" | "workingLabel" | "turnStartedAt" | "focused"
  | "sessionKey" | "live" | "config" | "engineVersion" | "planName" | "cwd" | "pathRoot"
  | "selectedSubagentKey" | "onSelectSubagent" | "onSignIn" | "approvals" | "activeApprovalIds"
  | "turnRestorePoints" | "onRestoreTurn" | "restoringTurnKey" | "onRetryHistory"
  | "thread" | "onThreadTurnDiff" | "onThreadEditFrom" | "onThreadForkAt" | "onThreadTurnRows"
  | "onThreadShip" | "onThreadAway" | "onThreadPreview" | "onPreviewAttachment" | "visualFrames"
  | "threadLayout"
  | "onLoadOlderHistory" | "onApprove" | "onDeny" | "onStop" | "queueItems" | "queueSendingId"
  | "queueError" | "queueControlsLocked" | "onQueueMove" | "onQueueEdit" | "onQueueRemove"
  | "onQueueSendNow" | "onQueueClear" | "onQueueEditing"> & {
  /** The merged, expanded transcript the shell built from history, items and echoes. */
  rows: readonly TranscriptRow[];
  /** The density dial the composer's control sets, and which press set it. */
  density: TranscriptDensity;
  densityEpoch: number;
  /** activeApprovalIds split by the caller's approval gate. */
  checkingApprovalIds: ReadonlySet<string>;
  ownerApprovalIds: ReadonlySet<string>;
  /** The pane's root: the window-level shortcuts are scoped to it. */
  rootRef: RefObject<HTMLDivElement | null>;
  /** Quote a reply's selection into the draft (T3 section 9); absent offers no Quote. */
  onQuoteReply?: (text: string) => void;
  controls: RefObject<ConversationTranscriptControls | null>;
  /**
   * Send a failed turn's prompt again, through the pane's own echo and send.
   * Resolves false only on a definite refusal. Passed only while the pane is
   * free to send; absent, no turn offers Retry.
   */
  retryThreadTurn?: (prompt: string) => Promise<boolean>;
};

export const ConversationTranscript = memo(function ConversationTranscript(
  props: ConversationTranscriptProps,
): ReactNode {
  const { items, thinking, live, sessionKey, rows, density, densityEpoch } = props;
  const cliChrome = useDocumentChrome() === "cli";
  // Only CLI mounts the opening block, so only CLI pays for it: refined would
  // recompute five lines it never draws on every render of every pane.
  const welcome = cliChrome
    ? welcomeLines({
      engineLabel: props.config.engineLabel,
      version: props.engineVersion,
      model: sessionStatusModel(props.config.model, props.config.models,
        props.config.engineLabel, props.config.engine),
      effort: props.config.effort,
      plan: props.planName,
      path: props.pathRoot ?? props.cwd,
    })
    : null;
  const history = props.history ?? IDLE_HISTORY;
  const past = history.status === "ready" ? history.items : NO_ITEMS;
  const plan = props.plan ?? (history.status === "ready" ? history.plan : undefined);
  const [find, setFind] = useState<TranscriptFindState>(CLOSED_FIND);
  const [awayFromLatest, setAwayFromLatest] = useState(false);
  // The row the reader was last looking at the bottom of, captured on the edge
  // into paused and cleared on every route back. Separate from `awayFromLatest`
  // on purpose: that one answers "is the capsule drawn", this one answers "what
  // has landed since", and a transcript with no rows at all has an honest answer
  // to the first and none to the second.
  const [jumpAnchor, setJumpAnchor] = useState<string | null>(null);
  // The user prompt the visible region is the answer to — the KEY only, because
  // its ordinal and the run's total are facts about the transcript this render
  // and would go stale the moment another prompt landed. See promptAnchorView.
  const [anchorKey, setAnchorKey] = useState<string | null>(null);
  // The row key whose restore is being confirmed. ONE dialog for the whole
  // pane rather than one per user turn: a transcript can hold hundreds of
  // turns, and a modal per turn is hundreds of hook-bearing components
  // rendering null to hold a boolean that is false everywhere but once.
  const [pendingRestore, setPendingRestore] = useState<string | null>(null);
  const [turnAnnouncement, setTurnAnnouncement] = useState("");
  const priorThinking = useRef(thinking);
  // DELIBERATELY [thinking] alone. The sentence is read once, on the edge into
  // idle, from the items of the render that committed that edge — which is
  // exactly what this closure holds. Keying it on `items` as well only re-ran
  // the whitespace pass over the streaming message on every token.
  useEffect(() => {
    if (priorThinking.current && !thinking) setTurnAnnouncement(finishedTurnAnnouncement(items));
    else if (!priorThinking.current && thinking) setTurnAnnouncement("Agent started working.");
    priorThinking.current = thinking;
  }, [thinking]);
  // Computed over the MERGED rows rather than inside collapseSuccessfulToolRuns,
  // because "which diffs follow this call" is a fact about the transcript and
  // not about how it happens to be folded this second.
  const diffPaths = useStableValue(useMemo(() => diffPathsByToolRow(rows), [rows]));
  // Same reason, one level up: which diffs a PROMPT caused is a fact about the
  // transcript, so it is derived over the merged rows and not over whichever
  // fold the density dial happens to be showing. Only the Chat layout draws
  // the chip, so a Threads pane skips the pass on every streamed frame.
  const threadLayout = props.threadLayout === true;
  const turnChanges = useStableValue(useMemo(() => threadLayout ? NO_TURN_CHANGES
    : turnChangeSummaries(rows), [threadLayout, rows]));
  // A Threads pane: which user row is which Rust turn, and where each turn's
  // answer ends. Joined by prompt digest only; a row that maps to no turn gets
  // no control (see thread-turns.ts). Both halves read the merged rows, for
  // the same reason as the two maps above.
  const thread = props.thread;
  const threadRowsFromStart = (history.status === "idle"
      || (history.status === "ready"
        && truncationNotice(history.olderMessages, history.truncated) === null))
    && !transcriptWasCompacted(rows);
  // A switched thread (protocol 59): only its current session's stretch,
  // matched from the newest end, carries controls.
  const threadTurns = useMemo(() => thread === undefined ? null
    : thread.switched === true
      ? matchThreadTurns(headStretchPromptRows(rows), thread.turns, false)
      : matchThreadTurns(threadPromptRows(rows), thread.turns, threadRowsFromStart),
  [thread, rows, threadRowsFromStart]);
  // The prompts that wear a centred stamp: the first, then one an hour on.
  const stampKeys = useMemo(() => threadTurns === null ? NO_STAMP_KEYS : threadStampKeys(threadTurns),
    [threadTurns]);
  const threadBusy = thread?.busy === true
    || thread?.away?.state === "running"
    || thread?.away?.state === "stopping";
  const threadControls = useStableValue(useMemo(() => {
    const controls = new Map<string, ThreadTurnControl>();
    if (thread === undefined || threadTurns === null) return controls;
    for (const [key, turn] of threadTurns) {
      controls.set(key, {
        ordinal: turn.ordinal,
        change: threadTurnChangeText(turn),
        diff: thread.project && turn.hasStart && props.onThreadTurnDiff !== undefined,
        edit: thread.project && props.onThreadEditFrom !== undefined,
        busy: threadBusy,
      });
    }
    return controls;
  }, [thread, threadBusy, threadTurns, props.onThreadTurnDiff, props.onThreadEditFrom]));
  // The same join turned around for the shell, which keys restore points by
  // row: reported only when an ordinal moves to another row (a send's echo
  // settling, a new turn matched), never per streamed delta.
  const threadRowKeys = useStableValue(useMemo(() => threadTurns === null ? NO_THREAD_ROW_KEYS
    : threadTurnRowKeys(threadTurns), [threadTurns]));
  const reportThreadRows = props.onThreadTurnRows;
  useEffect(() => { reportThreadRows?.(threadRowKeys); }, [reportThreadRows, threadRowKeys]);
  // Read only by the Chat layout's Fork; a Threads pane forks from its turn foot.
  const threadEnds = useMemo(() => threadLayout || threadTurns === null
    || props.onThreadForkAt === undefined ? NO_THREAD_ENDS : threadTurnEnds(rows, threadTurns),
  [threadLayout, rows, threadTurns, props.onThreadForkAt]);
  // Retry: the newest Rust turn, when it failed, while the pane may send and no
  // Fork or Edit holds it. A click holds the button until the turn it named
  // stops being the one offered (the new send recorded, or the pane busy).
  const retryThreadTurn = props.retryThreadTurn;
  const retryKey = useMemo(() => thread === undefined || threadTurns === null
    || retryThreadTurn === undefined || threadBusy
    ? null : threadRetryKey(rows, threadTurns, thread.turns),
  [thread, threadBusy, threadTurns, rows, retryThreadTurn]);
  const [retrying, setRetrying] = useState<string | null>(null);
  const retryingRef = useRef<string | null>(null);
  useEffect(() => {
    if (retryingRef.current !== null && retryingRef.current !== retryKey) {
      retryingRef.current = null;
      setRetrying(null);
    }
  }, [retryKey]);
  const onThreadRetry = useCommittedCallback((turnKey: string): void => {
    if (retryThreadTurn === undefined || turnKey !== retryKey || retryingRef.current !== null) return;
    const row = rows.find(candidate => candidate.key === turnKey);
    const prompt = row?.kind === "item" && row.item.kind === "user" ? row.item.text : "";
    if (prompt === "") return;
    retryingRef.current = turnKey;
    setRetrying(turnKey);
    void retryThreadTurn(prompt).then(sent => {
      // A definite refusal sent nothing: the same Retry is offered again.
      if (!sent && retryingRef.current === turnKey) {
        retryingRef.current = null;
        setRetrying(null);
      }
    });
  });
  const retryFor = (turnKey: string): "ready" | "sending" | undefined => turnKey !== retryKey
    ? undefined : retrying === turnKey ? "sending" : "ready";
  const [pendingEdit, setPendingEdit] = useState<{ ordinal: number; prompt: string } | null>(null);
  const onThreadTurn = useCommittedCallback((action: "diff" | "edit", turnKey: string) => {
    const turn = threadTurns?.get(turnKey);
    if (turn === undefined) return;
    if (action === "diff") {
      props.onThreadTurnDiff?.(turn.ordinal);
      return;
    }
    const row = rows.find(candidate => candidate.key === turnKey);
    const prompt = row?.kind === "item" && row.item.kind === "user" ? row.item.text : "";
    // Every turn asks first, turn 0 included: it has nothing before it to fork
    // from, so the caller starts a new thread in the same workspace instead,
    // but reverting the files is still the owner's choice to make.
    setPendingEdit({ ordinal: turn.ordinal, prompt });
  });
  // The Threads layout's controls, each a stable forwarder so their rows' memo holds.
  const onThreadEdit = useCommittedCallback((turnKey: string): void => { onThreadTurn("edit", turnKey); });
  const onThreadShip = useCommittedCallback((step: ShipNext | "read"): void => {
    props.onThreadShip?.(step);
  });
  const onThreadAway = useCommittedCallback((action: AwayCardAction): void => {
    props.onThreadAway?.(action);
  });
  // An Away loop's thread: each work round's turn, the review under it, the report.
  const awayCards = useMemo(() => awayTurnCards(thread?.away, thread?.turns),
    [thread?.away, thread?.turns]);
  const awayReportView = useMemo(() => awayReport(thread?.away), [thread?.away]);
  const onThreadReview = useCommittedCallback((ordinal: number): void => {
    props.onThreadTurnDiff?.(ordinal);
  });
  const onThreadPreview = useCommittedCallback((path: string): void => {
    props.onThreadPreview?.(path);
  });
  // Fork from here keeps that turn and every one before it (`keepTurns` counts from 1).
  const onThreadFork = useCommittedCallback((ordinal: number): void => {
    props.onThreadForkAt?.(ordinal + 1);
  });
  const onThreadUndo = useCommittedCallback((turnKey: string): void => { setPendingRestore(turnKey); });
  const toggleThreadFold = useCommittedCallback((turnKey: string): void => {
    // The find hit or the selected subagent holds its fold open, as it holds a folded run.
    if (turnKey === findFoldKey || turnKey === selectedFoldKey) return;
    const toggle = (): void => {
      setThreadFolds(current => {
        const next = new Map(current);
        next.set(turnKey, !(current.get(turnKey) ?? threadFoldsOpen));
        return next;
      });
    };
    // Opening shows the work at once and plays it in; closing plays it out,
    // then folds it. Reduced motion folds and unfolds in place.
    if (reducedMotion || !liveDrawn) {
      toggle();
      return;
    }
    // A press while this line is still moving waits for it: 340ms is shorter than a second thought.
    if (foldMotion !== null && foldMotion.key === turnKey) return;
    if (threadFolds.get(turnKey) ?? threadFoldsOpen) {
      startFoldMotion({ key: turnKey, dir: "close" }, toggle);
    } else {
      toggle();
      startFoldMotion({ key: turnKey, dir: "open" }, () => undefined);
    }
  });
  // Every display row that did not change keeps last frame's object, so the
  // memoized run, file-change and grid rows below skip a streamed token that
  // only touched the tail. An identity-only cache, like useStableValue.
  const retainedDisplay = useRef(NO_DISPLAY_ROWS);
  const displayRows = useMemo(() => {
    retainedDisplay.current = retainDisplayRows(retainedDisplay.current,
      groupSubagentGrids(collapseSuccessfulToolRuns(rows, toolRunMinimum(density))));
    return retainedDisplay.current;
  }, [rows, density]);
  // THE THREADS LAYOUT (owner, 2026-10-06 redesign): each turn's work line,
  // answer, changes card and controls, decided once per rows change in
  // thread-transcript.ts and mapped onto what the dial draws. Null on every
  // other pane, which then draws exactly what it always drew. An approval the
  // owner can still answer never folds, whichever turn it is in.
  const waitingApprovals = live ? props.activeApprovalIds : NO_APPROVAL_IDS;
  // Rust's turns tell a steer from a turn: a steer has no Rust turn of its own
  // and belongs to the run it arrived in (see threadSteerKeys).
  const threadMatch = useMemo<ThreadTurnMatch | undefined>(() =>
    threadTurns === null || thread === undefined ? undefined : {
      matched: threadTurns,
      newest: thread.turns.reduce((newest, turn) => Math.max(newest, turn.ordinal), -1),
    }, [threadTurns, thread]);
  const threadPlan = useMemo(() => threadLayout
    ? threadTranscriptPlan(rows, waitingApprovals, threadMatch) : null,
  [threadLayout, rows, waitingApprovals, threadMatch]);
  const threadView = useMemo(() => threadPlan === null
    ? null : threadDisplayPlan(displayRows, threadPlan), [threadPlan, displayRows]);
  // The turn still running never folds and has no end block yet: the pane's
  // own stream, a turn this pane is not the live surface for, or Rust's record.
  const lastThreadTurn = threadPlan?.turns[threadPlan.turns.length - 1];
  const liveThreadKey = lastThreadTurn !== undefined && (thinking || props.turnStartedAt !== undefined
    || threadTurns?.get(lastThreadTurn.userKey)?.state === "running") ? lastThreadTurn.userKey : null;
  const latestFinishedKey = threadPlan?.turns[threadPlan.turns.length
    - (liveThreadKey === null ? 1 : 2)]?.userKey ?? null;
  // THE LIVE TURN (owner, 2026-10-07): in the refined Threads layout the run
  // draws itself under its prompt — a header, a trail, its prose — and plays
  // a short finish into the finished turn's shape. Bare-CLI chrome keeps its
  // own running rows and working line.
  const liveDrawn = threadLayout && !cliChrome;
  const reducedMotion = prefersReducedMotion(mediaQuery());
  // The finish collapses the trail into a closed work line; at Detailed the
  // line opens anyway, so the trail is simply replaced.
  const beat = useThreadFinishBeat(liveDrawn ? liveThreadKey : null, sessionKey,
    liveDrawn && !reducedMotion, !transcriptDensityOpensItems(density));
  const beatKey = beat !== null && beat.phase !== "arrive" && beat.key === lastThreadTurn?.userKey
    ? beat.key : null;
  const trailKey = !liveDrawn ? null : liveThreadKey ?? beatKey;
  const trailPhase = liveThreadKey !== null || beat === null || beat.phase === "arrive"
    ? "live" as const : beat.phase;
  const trailRows = useMemo(() => {
    if (trailKey === null) return NO_ROWS;
    for (let index = rows.length - 1; index >= 0; index -= 1) {
      if (rows[index].key === trailKey) return rows.slice(index + 1);
    }
    return NO_ROWS;
  }, [rows, trailKey]);
  const approvalVerdicts = props.approvals;
  const ownerApprovals = live ? props.ownerApprovalIds : NO_APPROVAL_IDS;
  const checkingApprovals = live ? props.checkingApprovalIds : NO_APPROVAL_IDS;
  const liveModel = useMemo(() => {
    if (trailKey === null || threadPlan === null) return null;
    const foldedIn = threadPlan.foldedIn;
    return threadLiveTurn(trailRows, {
      waiting: ownerApprovals,
      checking: checkingApprovals,
      verdict: id => {
        const sent = approvalVerdicts[approvalVerdictKey(sessionKey ?? "", id)];
        return sent === undefined || sent.error !== undefined ? undefined : sent.verdict;
      },
      steers: threadPlan.steers,
      root: props.pathRoot,
      // The collapse draws only what folds: the rest is drawn below the work line.
      only: trailPhase === "collapse" ? key => foldedIn.get(key) === trailKey : undefined,
    });
  }, [trailKey, threadPlan, trailRows, ownerApprovals, checkingApprovals, approvalVerdicts,
    sessionKey, props.pathRoot, trailPhase]);
  // When the run began: the shell's clock, Rust's own run, this pane's clock, the send.
  const trailMeasured = trailKey === null ? undefined : threadTurns?.get(trailKey);
  const trailRun = trailMeasured === undefined ? undefined : threadRunFor(trailMeasured, thread);
  const rustRunStart = trailRun !== undefined && trailMeasured !== undefined
    && Math.abs(trailRun.startedAt - trailMeasured.sentAt) <= THREAD_RUN_MATCH_MS
    ? trailRun.startedAt : undefined;
  // Where the live turn's own rows begin in what the pane draws: everything
  // after its prompt is the live turn's to draw (it is always the last turn).
  const liveFrom = useMemo(() => {
    if (trailKey === null || trailPhase === "collapse") return -1;
    for (let index = displayRows.length - 1; index >= 0; index -= 1) {
      const display = displayRows[index];
      if (display.kind === "row" && display.row.key === trailKey) return index;
    }
    return -1;
  }, [displayRows, trailKey, trailPhase]);
  const [foldMotion, startFoldMotion] = useThreadFoldMotion();
  // YOUR MESSAGE RISES INTO PLACE (owner, 2026-10-07): a prompt this pane
  // sends, the moment it appears — never one loaded from history, nor any
  // already there when the pane mounted. followLatest arms it at the send;
  // the first new user row after that consumes it.
  const sendArmed = useRef(false);
  const newestUserKey = useMemo(() => {
    for (let index = rows.length - 1; index >= 0; index -= 1) {
      const row = rows[index];
      if (row.kind === "item" && row.item.kind === "user") return row.key;
    }
    return null;
  }, [rows]);
  const seenUserKey = useRef(newestUserKey);
  const [risingKey, setRisingKey] = useState<string | null>(null);
  useLayoutEffect(() => {
    if (seenUserKey.current === newestUserKey) return;
    seenUserKey.current = newestUserKey;
    const armed = sendArmed.current;
    sendArmed.current = false;
    if (armed && liveDrawn && !reducedMotion && newestUserKey !== null
        && threadPlan?.byUser.has(newestUserKey) === true) setRisingKey(newestUserKey);
  }, [newestUserKey, liveDrawn, reducedMotion, threadPlan]);
  useEffect(() => {
    if (risingKey === null) return;
    const id = window.setTimeout(() => { setRisingKey(null); }, 400);
    return () => { window.clearTimeout(id); };
  }, [risingKey]);
  // Each work line's fold. The dial seeds every one of them — Detailed opens
  // the work, Balanced and Compact fold it — and a press of the dial re-seeds
  // them all, exactly as it re-seeds every card (useDensityFold). A line the
  // reader toggles afterwards keeps their choice until the dial is pressed.
  const [threadFolds, setThreadFolds] = useState<ReadonlyMap<string, boolean>>(NO_THREAD_FOLDS);
  const threadFoldSeed = `${density}#${String(densityEpoch)}`;
  const [seenThreadFoldSeed, setSeenThreadFoldSeed] = useState(threadFoldSeed);
  if (seenThreadFoldSeed !== threadFoldSeed) {
    setSeenThreadFoldSeed(threadFoldSeed);
    setThreadFolds(NO_THREAD_FOLDS);
  }
  const threadFoldsOpen = transcriptDensityOpensItems(density);
  // The subagent the detail panel shows opens the fold it is in, so Reveal
  // always has a row to land on. Scanned only while one is selected.
  const selectedSubagent = props.selectedSubagentKey;
  const selectedFoldKey = useMemo(() => {
    if (threadPlan === null || selectedSubagent === undefined) return undefined;
    for (const row of rows) {
      if (row.kind !== "item" || row.item.kind !== "subagent") continue;
      if (row.item.id === selectedSubagent
          || findSubagentItem(row.item.children, selectedSubagent) !== undefined) {
        return threadPlan.foldedIn.get(row.key);
      }
    }
    return undefined;
  }, [threadPlan, rows, selectedSubagent]);
  // ONE object for every row, rebuilt only when a field a row reads changes.
  // Passing `props` itself handed each row a fresh object per render — React
  // mints one every time — so the row memo below could never hit and a streamed
  // token re-rendered the whole transcript. See ConversationItemContext.
  const onApprove = useCommittedCallback(props.onApprove);
  const onDeny = useCommittedCallback(props.onDeny);
  // Split by the shell, once per render of the pane: the checking cards, and
  // the requests the owner may actually answer — the only ones that may hold
  // the chord or the capsule. Both are kept by content there, because the
  // shipped caller hands over a fresh Set every render.
  const { checkingApprovalIds, ownerApprovalIds } = props;
  const onSelectSubagent = useCommittedCallback((key: string, viaKeyboard: boolean) => props.onSelectSubagent?.(key, viaKeyboard));
  const onPreviewAttachment = useCommittedCallback((attachment: Parameters<NonNullable<ConversationProps["onPreviewAttachment"]>>[0]) => {
    props.onPreviewAttachment?.(attachment);
  });
  const keyedApproval = useMemo(() => keyedApprovalId(items, live,
    ownerApprovalIds, props.approvals, sessionKey ?? ""),
  [items, live, ownerApprovalIds, props.approvals, sessionKey]);
  const itemContext = useStableValue<ConversationItemContext>({
    activeApprovalIds: props.activeApprovalIds,
    approvals: props.approvals,
    checkingApprovalIds,
    config: { engineLabel: props.config.engineLabel },
    focused: props.focused,
    keyedApprovalId: keyedApproval,
    live: props.live,
    onApprove,
    onDeny,
    onPreviewAttachment: props.onPreviewAttachment === undefined ? undefined : onPreviewAttachment,
    onSelectSubagent: props.onSelectSubagent === undefined ? undefined : onSelectSubagent,
    onSignIn: props.onSignIn,
    pathRoot: props.pathRoot,
    restoringTurnKey: props.restoringTurnKey,
    selectedSubagentKey: props.selectedSubagentKey,
    sessionKey: props.sessionKey,
    threadLayout,
    turnRestorePoints: props.turnRestorePoints,
    onQuoteReply: props.onQuoteReply,
    visualFrames: props.visualFrames,
  });
  const runningAgents = useMemo(() => {
    const cards: ConvItem[] = [];
    for (const row of rows) {
      if (row.kind === "item" && row.item.kind === "subagent") cards.push(row.item);
    }
    return collectRunningSubagents(cards);
  }, [rows]);
  const lastRowKey = useMemo(() => {
    for (let i = rows.length - 1; i >= 0; i--) {
      const row = rows[i];
      if (row.kind === "item") return row.key;
    }
    return null;
  }, [rows]);
  // What the pane DRAWS, one entry per painted line. The count is taken over
  // this rather than over `rows` because it is a claim about the screen: the
  // dial folds a dozen tool rows into one line at every density it offers, and
  // "12 new" above one folded line is the capsule contradicting the transcript
  // beside it.
  // Only while the reader is away from the latest: following, the backlog is
  // empty and this would be one array per row per streamed frame for nothing.
  const awayFromLatestRows = jumpAnchor !== null;
  // A Threads pane draws a folded turn's work as its one work line.
  const drawnKeys = useMemo(() => !awayFromLatestRows ? NO_DRAWN_KEYS
    : threadView === null ? displayRows.map(displayRowKeys)
      : threadDrawnKeys(displayRows, threadView.foldedBy, turn => turn !== liveThreadKey
        && !(threadFolds.get(turn) ?? threadFoldsOpen)),
  [awayFromLatestRows, displayRows, threadView, liveThreadKey, threadFolds, threadFoldsOpen]);
  // The user's own turns, for the sticky anchor above the scroller. Taken from
  // `rows` rather than `displayRows` because a prompt is never folded into
  // anything: it is always a line of its own, and always has a node registered.
  const prompts = useMemo(() => transcriptPrompts(rows), [rows]);
  const anchor = promptAnchorView(prompts, anchorKey);
  // Not memoized: it returns on the first line while the reader is following,
  // which is nearly always.
  // A Threads pane docks every waiting approval above its composer, always in
  // view, so its capsule never has to say one is waiting.
  const jumpBacklog = jumpLatestBacklog(rows, drawnKeys, jumpAnchor,
    live && !threadLayout ? ownerApprovalIds : NO_APPROVAL_IDS);
  const jumpLabel = jumpLatestLabel(jumpBacklog);
  const turnStartedAt = useRef<number | null>(null);
  const [finishedElapsed, setFinishedElapsed] = useState<Record<string, number>>({});
  const ownedStart = props.turnStartedAt;
  const liveStartedAt = thinking
    ? ownedStart ?? turnStartedAt.current ?? Date.now()
    : null;
  // The shell computes the live activity; an absent or blank one is the
  // ordinary case (the turn has not named a tool yet) and falls back to the
  // word the strip has always said.
  const workingWord = props.workingLabel?.trim() || "Working";
  // THE TURN CLOCK, COMMITTED. The start is kept and the finish stamped after
  // the commit that shows them, never during a render React may throw away:
  // an abandoned pass that had cleared the start used to lose the stamp. A
  // layout effect, so the stamped frame is the one painted. A Threads pane
  // also remembers the measure for that turn, so it outlives the next send.
  const stampTurn = threadPlan?.turns[threadPlan.turns.length - 1];
  const stampMeasured = stampTurn === undefined ? undefined : threadTurns?.get(stampTurn.userKey);
  useLayoutEffect(() => {
    if (thinking) {
      turnStartedAt.current = liveStartedAt;
      return;
    }
    if (ownedStart !== undefined) {
      // The turn is still running; this pane is just not the live surface.
      turnStartedAt.current = ownedStart;
      return;
    }
    if (turnStartedAt.current === null) return;
    const now = Date.now();
    const stamped = stampTurnClock(turnStartedAt.current, now, lastRowKey);
    turnStartedAt.current = null;
    if (stamped === null) return;
    setFinishedElapsed(current => current[stamped.key] === stamped.ms
      ? current : { ...current, [stamped.key]: stamped.ms });
    if (sessionKey !== undefined && stampMeasured !== undefined) {
      rememberThreadTurnTiming(sessionKey, stampMeasured, { durationMs: stamped.ms, endedAt: now }, "window");
    }
  }, [thinking, ownedStart, liveStartedAt, lastRowKey, sessionKey, stampMeasured]);
  // Closed find bars cost nothing on a streaming transcript. Once open, rows
  // rebuild the bounded corpus; query keystrokes reuse it.
  const findCorpus = useMemo(() => find.open
    ? buildTranscriptSearchCorpus(rows) : EMPTY_FIND_CORPUS, [find.open, rows]);
  const listRef = useRef<HTMLDivElement>(null);
  // Refs, not state: scrolling must not re-render the transcript.
  const stick = useRef(true);
  // A session switch owes this pane one pin. Set in render, cleared only by the
  // commit that consumes it — see the effect for why `stick` cannot do this job.
  const pinNext = useRef(false);
  // What the last commit measured, for the one case where growth must NOT move
  // the reader — see the pin effect.
  const lastHeight = useRef(0);
  const lastPastCount = useRef(0);
  const rootRef = props.rootRef;
  const findInputRef = useRef<HTMLInputElement>(null);
  const findReturnFocus = useRef<HTMLElement | null>(null);
  const findNodes = useRef(new Map<string, HTMLDivElement>());
  const registerFindNode = useCallback((key: string, node: HTMLDivElement | null) => {
    if (node === null) findNodes.current.delete(key);
    else findNodes.current.set(key, node);
  }, []);
  const findResult = useMemo(
    () => searchTranscriptCorpus(findCorpus, find.query), [findCorpus, find.query]);
  const findMatches = findResult.matches;
  const findIndex = findMatches.length === 0
    ? -1 : Math.min(Math.max(find.index, 0), findMatches.length - 1);
  const currentFindKey = findIndex < 0 ? null : findMatches[findIndex].key;
  const matchingFindKeys = useMemo(
    () => new Set(findMatches.map(match => match.key)), [findMatches]);
  // Memoized because this component re-renders on every streamed delta and
  // SteeringQueue's own normalize keys off this array's identity.
  const queueItems = useMemo(
    () => normalizeSteeringQueue(props.queueItems ?? NO_QUEUE), [props.queueItems]);
  const queueControlsReady = props.onQueueMove !== undefined
    && props.onQueueEdit !== undefined && props.onQueueRemove !== undefined
    && props.onQueueSendNow !== undefined && props.onQueueClear !== undefined;
  const queueError = props.queueError?.trim() || null;

  function rememberFindReturnFocus() {
    const active = document.activeElement;
    findReturnFocus.current = active instanceof HTMLElement
      && rootRef.current?.contains(active) === true ? active : rootRef.current;
  }

  function openFind() {
    if (!find.open) {
      rememberFindReturnFocus();
      setFind(state => ({ ...state, open: true }));
      return;
    }
    findInputRef.current?.focus();
    findInputRef.current?.select();
  }

  function closeFind() {
    setFind(state => ({ ...state, open: false }));
    findReturnFocus.current?.focus();
  }

  function moveFind(direction: -1 | 1) {
    setFind(state => ({
      ...state,
      index: moveTranscriptSearchIndex(state.index, findMatches.length, direction),
    }));
  }

  // SESSION SWITCH WITHOUT A REMOUNT — detected DURING RENDER, and that is the
  // whole point of it being here rather than in an effect.
  //
  // The pin below is a useLayoutEffect, because it has to move the scroller
  // before paint. React runs EVERY layout effect during commit and every
  // passive effect only after paint, so a `useEffect` reset would land a full
  // frame later than the pin no matter which of the two was declared first —
  // declaration order only ever decides ties WITHIN one phase. On the commit
  // where `sessionKey` changes, the pin would therefore read the PREVIOUS
  // session's `stick.current` and park session B at session A's offset; for an
  // idle session nothing renders again, so nothing ever corrects it. Render is
  // the only placement that is ordered before the commit by construction.
  //
  // This is React's documented "adjust state when a prop changes": setSeenKey
  // makes React throw this render pass away and re-run the component
  // immediately, so the DOM is committed once and A's transcript is never
  // painted under B's title.
  //
  // Writing refs during render is otherwise forbidden precisely because a pass
  // can be abandoned, so nothing here depends on THIS pass being the one that
  // commits: every ref write assigns a constant, and `pinNext` is a latch
  // that is only ever set here and only ever cleared by the commit that acts on
  // it. A discarded pass (StrictMode's double render, or a switch superseded
  // before it commits) leaves exactly the same state. On mount seenKey starts
  // equal, so this costs nothing.
  //
  // These state/ref resets, and no more. `lastHeight`/`lastPastCount` are NOT reset: the
  // pin below takes its first branch on this commit (the latch), so neither is
  // read there, and both are overwritten at the end of that same commit —
  // zeroing them would be a write no consumer can observe, dressed up as state.
  // The echoes need no reset either: they are selected by session key above.
  const [seenKey, setSeenKey] = useState(sessionKey);
  if (seenKey !== sessionKey) {
    setSeenKey(sessionKey);
    // The composer resets its own draft in its own render of this same commit.
    setFind(CLOSED_FIND);       // A's query/results do not describe B's transcript
    setAwayFromLatest(false);
    setJumpAnchor(null);        // B opens at its end, so nothing is behind the reader
    // A's row keys and B's are minted by the same rules, so a stale anchor
    // could resolve against B's transcript and name the wrong prompt.
    setAnchorKey(null);
    // A pending confirmation belongs to A's turn key and A's working tree. It
    // must never be answered over B's transcript.
    setPendingRestore(null);
    setPendingEdit(null);       // the same for an Edit from here on A's turn
    setThreadFolds(NO_THREAD_FOLDS);   // A's work lines are folded or open over A's turns
    // A's turn clock is A's. Without these, B's last row would be stamped with
    // the time since A's turn began, and A's positional row keys (`l:text-40`)
    // would put A's "Done in" lines on B's rows.
    setFinishedElapsed({});
    turnStartedAt.current = null;
    setRisingKey(null);         // A's send does not rise in B
    sendArmed.current = false;
    stick.current = true;      // keep following B's stream as it grows
    pinNext.current = true;    // B opens at ITS newest message, not A's offset
  }

  // Only the focused workspace tile owns the browser-style find shortcut. This
  // listener is window-level so Cmd/Ctrl+F works from the composer and controls,
  // but the prop prevents sibling conversations from all answering it.
  useEffect(() => {
    if (props.focused !== true) return;
    const onFindKey = (event: KeyboardEvent): void => {
      if (event.defaultPrevented) return;
      const root = rootRef.current;
      if (root === null) return;
      if (!paneOwnsShortcut(root, event)) return;
      // Leave Cmd/Ctrl+Shift+F available to the app's all-files search.
      const primary = (event.metaKey || event.ctrlKey)
        && !event.altKey && !event.shiftKey;
      if (primary && event.key.toLowerCase() === "f") {
        event.preventDefault();
        openFind();
      } else if (event.key === "Escape" && find.open) {
        event.preventDefault();
        closeFind();
      }
    };
    window.addEventListener("keydown", onFindKey);
    return () => window.removeEventListener("keydown", onFindKey);
  }, [props.focused, find.open]);

  // The keys printed on the approval card's own buttons. Pane-scoped exactly
  // the way Find above is — only the focused tile answers, and only while focus
  // is inside it or nowhere — and then it CLICKS the button rather than calling
  // a verdict of its own, so the chord and the pointer cannot diverge: Deny
  // still opens its note field first, an unanswered question card still refuses,
  // and the optimistic latch is written once, in the one handler that writes it.
  //
  // The text-field guard is what makes the chord safe to bind at all: Cmd+Enter
  // in the composer sends the prompt, and it must keep doing exactly that.
  //
  // With more than one request outstanding the LAST card wins — the bottom-most
  // one, which is the one the reader is looking at.
  useEffect(() => {
    if (props.focused !== true) return;
    const onApprovalKey = (event: KeyboardEvent): void => {
      if (event.defaultPrevented) return;
      const root = rootRef.current;
      if (root === null) return;
      if (!paneOwnsShortcut(root, event)) return;
      const target = event.target;
      const verdict = approvalKeyVerdict(event,
        approvalKeyTyping(target instanceof HTMLElement ? target : null));
      if (verdict === null) return;
      const buttons = root.querySelectorAll<HTMLButtonElement>(
        `button[${APPROVAL_KEY_ATTR}="${verdict}"]`);
      const button = buttons[buttons.length - 1];
      if (button === undefined || button.disabled) return;
      event.preventDefault();
      // REVEAL, THEN ANSWER. The chord reaches the keyed card wherever it is,
      // including far above a paused reader with only the amber capsule to say
      // a request exists — and allowing a command nobody can see is the exact
      // outcome this card exists to prevent. A card off screen spends this
      // press on being shown and answers nothing; the next press answers it.
      // See approvalChordStep for why partial visibility already counts.
      // A card docked above a Threads composer is outside the scroller and
      // always on screen, so it is answered at once.
      const list = listRef.current;
      const card = button.closest(".conv-approval, .thread-dock-card");
      if (list !== null && card !== null && list.contains(card)) {
        const region = list.getBoundingClientRect().top;
        const rect = card.getBoundingClientRect();
        if (approvalChordStep(rect.top - region, rect.bottom - region,
          list.clientHeight) === "reveal") {
          card.scrollIntoView({
            block: "center",
            inline: "nearest",
            // The same route the capsule and the prompt steppers take, and the
            // same reason: a reveal paced from script is invisible to the
            // stylesheet's reduced-motion guard — see jumpScrollBehavior.
            behavior: jumpScrollBehavior(mediaQuery()),
          });
          return;
        }
      }
      button.click();
    };
    window.addEventListener("keydown", onApprovalKey);
    return () => window.removeEventListener("keydown", onApprovalKey);
  }, [props.focused]);

  useEffect(() => {
    if (!find.open) return;
    findInputRef.current?.focus();
    findInputRef.current?.select();
  }, [find.open]);

  // No dep array on purpose — a growing stream keeps pace render by render, but
  // only while pinned. Once the user scrolls up (>40px off the bottom) their
  // view is left alone until they come back down.
  //
  // useLayoutEffect, not useEffect, BECAUSE OF HISTORY: a loaded transcript
  // lands as one block of hundreds of items, and a post-paint scroll paints the
  // TOP of that block for a frame before jumping to the end. Before paint, the
  // pane is simply already at the end — which is where a chat app opens.
  //
  // WHY THE SWITCH USES A LATCH RATHER THAN `stick.current`. It would be
  // convenient to argue that the render-phase reset always wins because event
  // handlers cannot interleave with a commit — but that is not a proof under
  // this root. `main.tsx` mounts with createRoot, and not every sessionKey
  // change starts in a discrete event: App.tsx dispatches "select" from a
  // promise callback after a fork, which schedules in a time-sliceable lane, so
  // React may yield between the render above and this commit. A scroll event
  // delivered in that window runs onScroll and sets `stick.current = false`
  // before the line below reads it. `pinNext` cannot be lost that way — nothing
  // but this effect clears it — so "B opens at its newest message" is an actual
  // invariant, not an accident of some other ref starting at 0.
  //
  // `stick` stays true through that same window in practice anyway: the pin's
  // own scrollTop write fires a scroll event, and onScroll re-reads the pinned
  // position as "at the bottom". The latch is what makes the first frame right.
  //
  // ONE PIN FOR THE WHOLE PANE. The composer and the shell commit without this
  // component whenever only they changed, and either can resize the scroller
  // (an error line, the lock banner, the context row, the recap slot). They
  // call `pin` from their own layout effects, as the single-component pane ran
  // this on every commit. It is idempotent within a commit: a second run
  // measures the same height and finds nothing grown.
  const pastNow = useRef(past);
  const pin = useCallback((): void => {
    const el = listRef.current;
    if (el === null) return;      // no DOM yet: the pin is still owed, latch kept
    const past = pastNow.current;
    const height = el.scrollHeight;
    const grew = height - lastHeight.current;
    if (pinNext.current || stick.current) el.scrollTop = height;
    else if (past.length !== lastPastCount.current && grew > 0) {
      // History arrived ABOVE the viewport while the user was reading something
      // further down. Leaving scrollTop alone would slide their content down by
      // the full height of everything just inserted; adding that growth back
      // leaves the line they were on exactly where it was. Gated on the history
      // COUNT changing, not on array identity, so a caller that rebuilds the
      // array every render cannot make this fight a live stream growing below.
      el.scrollTop += grew;
    }
    pinNext.current = false;
    lastHeight.current = height;
    lastPastCount.current = past.length;
  }, []);
  useLayoutEffect(() => {
    pastNow.current = past;
    pin();
  });

  // Search navigation deliberately breaks the live-stream pin: the requested
  // result wins until the reader scrolls back to the bottom or sends/resumes.
  // Nested subagent ancestors render open from currentFindKey before this layout
  // effect runs, so even a match that was folded away has a node by commit time.
  useLayoutEffect(() => {
    if (!find.open || currentFindKey === null) return;
    const node = findNodes.current.get(currentFindKey);
    if (node === undefined) return;
    stick.current = false;
    // Said here rather than left to the scroll event this line usually fires: a
    // match that is ALREADY centred moves nothing, so onScroll never runs, and
    // the pane would sit paused with no capsule on it. That is the one state
    // where an approval arriving below could raise nothing at all, and the
    // exception's whole word is "always".
    setAwayFromLatest(true);
    node.scrollIntoView({ block: "center", inline: "nearest" });
  }, [find.open, currentFindKey]);

  function onScroll() {
    const el = listRef.current;
    if (el) {
      // ONE read of the ref feeds both setters. The updater below can run in a
      // later render pass, by which time another scroll event may have written
      // `stick.current` again — reading it twice could leave the capsule drawn
      // over a cleared anchor, or an anchor under a hidden capsule.
      stick.current = el.scrollHeight - el.scrollTop - el.clientHeight <= 40;
      const following = stick.current;
      setAwayFromLatest(!following);
      // ANCHORED ONCE, on the edge into paused. `current ?? lastRowKey` is the
      // whole rule: re-anchoring on every scroll event while still paused would
      // zero the count each time the reader nudged the wheel, so the number
      // would only ever read what arrived since their last twitch.
      setJumpAnchor(current => following ? null : current ?? lastRowKey);
      measurePromptAnchor();
    }
  }

  // WHERE THE PROMPTS ARE, from the one scroll event that already runs. A
  // second listener on the same scroller would read the same event in a
  // different frame, and the bar could then disagree with the capsule beside
  // it. Nothing else needs to re-measure: a prompt's position relative to the
  // reader only changes when the scroller moves, and every write to it (the
  // pin, history landing above, a jump, a step) moves it and fires this.
  //
  // The nodes come from the find bar's registry, which every drawn item row
  // fills unconditionally — one rect for the scroller and one per prompt, and
  // no second ref map to keep in step with it.
  //
  // MEASURED FROM THE NEWEST BACKWARDS, and stopped at the first prompt at or
  // above the region's top edge. Tops rise with document order, so that prompt
  // is exactly the last one promptAnchorKey would find scanning forward, and
  // nothing above it can change the answer — so nothing above it is measured.
  // The rule itself stays where it was: this hands the collected tail to the
  // same pure function, which still sees the newest spot last and still gets to
  // decide. A paused transcript with two hundred prompts reads one to three
  // rects per scroll event instead of two hundred.
  function measurePromptAnchor() {
    const el = listRef.current;
    if (el === null) return;
    const region = el.getBoundingClientRect().top;
    const spots: PromptAnchorSpot[] = [];
    for (let index = prompts.length - 1; index >= 0; index -= 1) {
      const prompt = prompts[index];
      if (prompt === undefined) continue;
      const node = findNodes.current.get(prompt.key);
      if (node === undefined) continue;
      const top = node.getBoundingClientRect().top - region;
      spots.unshift({ key: prompt.key, top });
      if (top <= 0) break;
    }
    const next = promptAnchorKey(spots, el.clientHeight);
    // Same value, same object: scroll events arrive by the dozen and the
    // transcript underneath is the most expensive thing this app renders.
    setAnchorKey(current => current === next ? current : next);
  }

  // Both steppers and the line itself land the prompt exactly at the top of the
  // visible region, which is the position promptAnchorKey then reads back as
  // "this one governs" — so the bar names the prompt the click asked for rather
  // than the one above it.
  function goToPrompt(key: string) {
    const el = listRef.current;
    const node = findNodes.current.get(key);
    if (el === null || node === undefined) return;
    // Stepping BACK is leaving the live stream, and it is said here rather than
    // left to the scroll event this fires: a prompt that is already at the top
    // moves nothing, so onScroll would never run and the pane would sit paused
    // with no capsule on it — the same case the find navigation covers.
    stick.current = false;
    setAwayFromLatest(true);
    setAnchorKey(key);
    // THE BAR CAN VANISH UNDER THE PRESS, and only on this one step. Landing the
    // NEWEST prompt at the top of the region is exactly the state
    // promptAnchorKey answers null for, so a few frames into the scroll the
    // control the reader is holding unmounts — and a focused element that is
    // removed drops focus to document.body, i.e. the keyboard reader is
    // silently returned to the top of the document.
    //
    // Focus moves to the transcript itself, before the scroll rather than
    // after: it is what the step was FOR, it is where the arrow keys should go
    // next, and role="log" + aria-label already give it a name to announce.
    // preventScroll because the line below owns where the pane lands.
    //
    // The other route — keeping the bar mounted for this one case — was the
    // larger change: it needs a second piece of state and a rule for when to
    // let go of it, and it would make the pane draw a bar over a prompt that is
    // on screen, which is the one thing the hide rule exists to prevent.
    if (key === prompts[prompts.length - 1]?.key) el.focus({ preventScroll: true });
    el.scrollTo({
      top: el.scrollTop + (node.getBoundingClientRect().top - el.getBoundingClientRect().top),
      // The one animation a stylesheet cannot reach, on the same route the jump
      // capsule takes — see jumpScrollBehavior.
      behavior: jumpScrollBehavior(mediaQuery()),
    });
  }

  function jumpToLatest() {
    const el = listRef.current;
    if (el === null) return;
    stick.current = true;
    setAwayFromLatest(false);
    setJumpAnchor(null);
    // The one animation in the app that a stylesheet cannot reach, so it reads
    // the reduced-motion preference itself — see jumpScrollBehavior.
    el.scrollTo({
      top: el.scrollHeight,
      behavior: jumpScrollBehavior(mediaQuery()),
    });
  }

  // Your own message always pulls the view back down. Called by the composer
  // right after it hands a send to the caller; see ConversationTranscriptControls.
  const followLatest = useCallback((): void => {
    stick.current = true;   // your own message always pulls the view back down
    // Stated here rather than left to the scroll event the pin below fires: a
    // pane already at the bottom writes the same scrollTop and fires nothing,
    // and a stale "12 new" outliving the send would be a lie about the pane the
    // reader is looking straight at.
    setAwayFromLatest(false);
    setJumpAnchor(null);
    // The message being sent IS the newest prompt, and the pin above puts it on
    // screen. Said here for the same reason as the two lines above it: a pane
    // already at the bottom fires no scroll event to re-measure from.
    setAnchorKey(null);
    // The message being sent rises into place when its row appears.
    sendArmed.current = true;
  }, []);
  useImperativeHandle(props.controls, () => ({ followLatest, pin }), [followLatest, pin]);

  // Both halves of the contract, or no control at all: a map with no callback
  // could only draw a button that does nothing, and a callback with no map has
  // no turn it is allowed to be offered on.
  // A worktree thread's Edit from here stands in for the Undo control.
  const restoreReady = props.turnRestorePoints !== undefined
    && props.onRestoreTurn !== undefined && props.thread?.worktree !== true;
  const confirmingRestore = turnRestoreConfirmation(pendingRestore,
    props.turnRestorePoints);

  const findActive = find.open && find.query.length > 0;
  const findRender = useStableValue(useMemo<FindRenderContext>(() => ({
    active: findActive,
    matches: matchingFindKeys,
    currentKey: findActive ? currentFindKey : null,
    register: registerFindNode,
  }), [findActive, matchingFindKeys, currentFindKey, registerFindNode]));
  const findStatus = transcriptSearchStatus(find.query, findIndex, findResult);
  // Fork from here, under the row a thread turn's answer ends on. Keeps that
  // turn and every one before it (`keepTurns` counts prompts, from 1).
  const forkAt = props.onThreadForkAt;
  const forkRow = (key: string | undefined): ReactNode => {
    const turn = key === undefined || forkAt === undefined ? undefined : threadEnds.get(key);
    if (turn === undefined || forkAt === undefined) return null;
    return (
      <div className="conv-turn-end">
        <button type="button" className="conv-turn-action"
          aria-label={`Fork from here: a new thread with the first ${turn.ordinal + 1} ${
            turn.ordinal === 0 ? "message" : "messages"} of this one`}
          title="Fork from here"
          // One Fork or Edit per pane at a time; a double click is one fork.
          disabled={thread?.busy === true}
          onClick={() => { forkAt(turn.ordinal + 1); }}>
          <Icon name="type-hierarchy-sub" sm />
          Fork from here
        </button>
      </div>
    );
  };

  // ---- the Threads layout's rows -------------------------------------------
  // The find hit being shown opens the fold it is in, as it opens a folded run.
  const findFoldKey = findActive && currentFindKey !== null && threadPlan !== null
    ? threadPlan.foldedIn.get(currentFindKey.split("/")[0]) : undefined;
  const threadFoldOpen = (turnKey: string): boolean => turnKey === findFoldKey
    || turnKey === selectedFoldKey || (threadFolds.get(turnKey) ?? threadFoldsOpen);
  // The live turn says what the bottom strip said, under its prompt, so a
  // Threads pane drawing one has no strip. Every other pane keeps it.
  const workingStrip = thinking && !(liveDrawn && liveThreadKey !== null);
  const livePill = liveDrawn && liveThreadKey !== null && liveModel !== null ? liveModel.header : null;
  const liveRunStart = ownedStart ?? rustRunStart ?? liveStartedAt ?? trailMeasured?.sentAt;

  // The work line moving: rows playing in after it opens, or out before it folds.
  const closingFold = foldMotion?.dir === "close" ? foldMotion.key : null;
  // Under a finished turn's answer: what it changed, then its controls. A turn
  // this pane watched finish brings them in last (`arriving`).
  const threadEnd = (turnKey: string): ReactNode => {
    const turn = threadPlan?.byUser.get(turnKey);
    if (turn === undefined) return null;
    const measured = threadTurns?.get(turnKey);
    const control = threadControls.get(turnKey);
    const changes = threadChangesView(turn, measured, props.pathRoot);
    const point = restoreReady ? props.turnRestorePoints?.get(turnKey) : undefined;
    const arriving = beat !== null && beat.phase === "arrive" && beat.key === turnKey;
    return (
      <>
        {changes !== null && (
          <ThreadChangesCard view={changes} onReview={onThreadReview} arriving={arriving}
            onPreview={props.onThreadPreview === undefined ? undefined : onThreadPreview}
            ordinal={control?.diff === true ? control.ordinal : undefined}
            // Shipping rides the latest card only, in a project thread.
            {...(turnKey === latestFinishedKey && thread?.project === true && props.onThreadShip !== undefined
              ? { ship: thread.ship, onShip: onThreadShip } : {})} />
        )}
        <ThreadTurnFoot turnKey={turnKey} latest={turnKey === latestFinishedKey} arriving={arriving}
          // Rust's stamp while it is this turn's, else the one this window remembered.
          endedAt={threadTurnTiming(sessionKey, measured, thread).endedAt} answer={turn.answerText}
          // Fork keeps this turn and every one before it, as Fork from here
          // always did; a turn still running, or one with nothing after its
          // prompt, has no answer to keep.
          fork={forkAt !== undefined && measured !== undefined && measured.state !== "running"
            && turn.replied ? measured.ordinal : undefined}
          busy={threadBusy}
          // A worktree thread has no Undo: restoreReady already says so.
          undo={point === undefined ? undefined : point.label ?? "Restore"}
          restoring={props.restoringTurnKey === turnKey}
          restoreBusy={props.restoringTurnKey != null}
          retry={retryFor(turnKey)} onRetry={onThreadRetry}
          onFork={onThreadFork} onUndo={onThreadUndo} />
        {measured !== undefined && awayCards.has(measured.ordinal) && thread?.away !== undefined && (
          <AwayReviewCard card={awayCards.get(measured.ordinal) as AwayTurnCard}
            reviewer={awayReviewerOf(thread.away)}
            onAction={props.onThreadAway === undefined ? undefined : onThreadAway} />
        )}
      </>
    );
  };
  const threadRow = (display: ConversationDisplayRow, view: ThreadDisplayPlan): ReactNode => {
    const key = threadDisplayKey(display);
    const foldedBy = view.foldedBy.get(key);
    const endOf = view.endOf.get(key);
    // The running turn, and one still playing its finish, have no end block yet.
    const end = endOf === undefined || endOf === liveThreadKey || endOf === trailKey
      ? null : threadEnd(endOf);
    // An earlier turn's controls show while its answer is hovered or focused.
    const answerClass = end === null ? undefined
      : endOf === latestFinishedKey ? "thread-answer is-latest" : "thread-answer";
    if (foldedBy !== undefined && foldedBy !== liveThreadKey && !threadFoldOpen(foldedBy)) {
      // Folded away. A turn with no final message still owes its end block.
      return end === null ? null : <div key={key} className={answerClass}>{end}</div>;
    }
    // A row of a work line that is opening or closing plays in or out.
    const reveal = foldedBy !== undefined && foldMotion !== null && foldMotion.key === foldedBy
      ? foldMotion.dir : null;
    const className = reveal === null ? answerClass
      : `${answerClass ?? ""} thread-reveal is-${reveal === "open" ? "opening" : "closing"}`.trim();
    const box = (body: ReactNode): ReactNode => reveal === null ? body
      : <div className="thread-reveal-body">{body}</div>;
    // A failure is drawn in its own tinted block, never folded (see the plan).
    const tint = (body: ReactNode): ReactNode => view.failed.has(key)
      ? <div className="thread-failed">{body}</div> : body;
    if (display.kind !== "row") {
      return <div key={key} className={className}>
        {box(<>
          {tint(display.kind === "tool-run"
            ? <ToolRun run={display} all={itemContext} find={findRender}
                density={density} densityEpoch={densityEpoch} />
            : display.kind === "file-change"
              ? <FileChangeRow display={display} all={itemContext} find={findRender}
                  density={density} densityEpoch={densityEpoch} />
              : display.kind === "subagent-grid"
                ? <SubagentGrid run={display} all={itemContext} find={findRender} />
                : <EditedFilesRow run={display} find={findRender} pathRoot={props.pathRoot} />)}
          {end}
        </>)}
      </div>;
    }
    const row = display.row;
    if (row.kind === "mark") {
      return <div key={key} className="conv-mark">{row.label ?? "New messages"}</div>;
    }
    // The composer docks every approval the owner can answer; the transcript
    // only says which step waits on them, never with the card itself.
    if (row.item.kind === "approval" && row.item.resolved === undefined
        && ownerApprovals.has(row.item.id)) {
      const steps: ThreadTrailRow[] = threadLiveTurn([row], { waiting: ownerApprovals,
        root: props.pathRoot }).segments.flatMap(segment => segment.kind === "trail" ? segment.rows : []);
      return <div key={key} className={className}>
        {box(<><ThreadStepList rows={steps} all={itemContext} find={findRender} />{end}</>)}
      </div>;
    }
    // A prompt's bubble carries no toolbar here: its change summary is the
    // changes card, Undo and Fork are the turn's controls, and Edit from here
    // is in the quiet line under it.
    // A switch's handoff (protocol 59) is folded to one line, as an Away
    // round's instructions are.
    const handoff = row.item.kind === "user" ? threadHandoffLine(row.item.text) : null;
    // A scheduled or watch prompt (protocol 62) folds the same way.
    const scheduled = handoff === null && row.item.kind === "user" ? scheduledPromptFold(row.item.text) : null;
    const item = handoff !== null && row.item.kind === "user"
      ? <ThreadHandoffPrompt line={handoff} text={row.item.text} />
      : scheduled !== null ? <ThreadHandoffPrompt line={scheduled.line} text={scheduled.body} icon="watch" />
      : <SearchableItem item={row.item} all={itemContext}
        findKey={row.key} find={findRender} density={density} densityEpoch={densityEpoch}
        followingDiffPaths={diffPaths.get(row.key)} />;
    const turn = threadPlan?.byUser.get(row.key);
    if (turn === undefined || row.item.kind !== "user") {
      return <div key={key} className={className}>{box(<>{tint(item)}{end}</>)}</div>;
    }
    const measured = threadTurns?.get(row.key);
    const control = threadControls.get(row.key);
    const running = row.key === liveThreadKey;
    const awayRound = measured === undefined ? undefined : awayCards.get(measured.ordinal);
    const trail = row.key === trailKey && liveModel !== null ? liveModel : null;
    const timing = threadTurnTiming(sessionKey, measured, thread);
    const foldLine = turn.folded > 0 && !running ? (
      <ThreadWorkFold turnKey={row.key} open={threadFoldOpen(row.key) && closingFold !== row.key}
        onToggle={toggleThreadFold} arriving={trail !== null && trailPhase === "collapse"}
        // Rust's stamps, else what this window remembered or measured itself.
        lead={threadWorkLead(timing.durationMs
          ?? (turn.endKey === null ? undefined : finishedElapsed[turn.endKey]))}
        work={turn.work} />
    ) : null;
    return <div key={key}>
      <div className={row.key === risingKey ? "thread-user is-sent" : "thread-user"}>
        {measured !== undefined && stampKeys.has(row.key) && <ThreadStamp at={measured.sentAt} />}
        {awayRound === undefined ? item : <AwayRoundPrompt card={awayRound} text={row.item.text} />}
        <ThreadUserMeta turnKey={row.key} text={row.item.text} sentAt={measured?.sentAt}
          edit={control?.edit === true} busy={control?.busy === true} onEdit={onThreadEdit} />
      </div>
      {trail !== null ? (
        <ThreadLiveTurn key="live" model={trail} phase={trailPhase}
          startedAt={liveRunStart}
          endedAt={timing.endedAt ?? beat?.at} fold={foldLine}
          all={itemContext} find={findRender} density={density} densityEpoch={densityEpoch} />
      ) : foldLine}
      {/* A turn that failed before writing anything has no end block to
          carry its controls, so Retry stands under the prompt itself. */}
      {turn.endKey === null && retryFor(row.key) !== undefined && (
        <div className="thread-answer is-latest">
          <ThreadTurnFoot turnKey={row.key} latest endedAt={timing.endedAt} answer=""
            busy={thread?.busy === true} restoring={false}
            restoreBusy={props.restoringTurnKey != null}
            retry={retryFor(row.key)} onRetry={onThreadRetry}
            onFork={onThreadFork} onUndo={onThreadUndo} />
        </div>
      )}
    </div>;
  };

  return (
    <>
      {find.open && (
        <div className="conv-findbar" role="search" aria-label="Find in conversation">
          <Icon name="search" className="conv-kind" />
          <input
            ref={findInputRef}
            className="conv-findfield"
            type="search"
            value={find.query}
            maxLength={TRANSCRIPT_SEARCH_MAX_QUERY_CHARS}
            aria-label="Find in conversation"
            aria-keyshortcuts="Enter Shift+Enter Escape"
            placeholder="Find in conversation"
            onChange={event => setFind(state => ({
              ...state,
              query: event.target.value.slice(0, TRANSCRIPT_SEARCH_MAX_QUERY_CHARS), index: 0,
            }))}
            onKeyDown={event => {
              if (event.key === "Enter") {
                event.preventDefault();
                moveFind(event.shiftKey ? -1 : 1);
              } else if (event.key === "Escape") {
                event.preventDefault();
                event.stopPropagation();
                closeFind();
              }
            }}
          />
          <output className={findMatches.length === 0
            ? "conv-findcount conv-findcount-empty" : "conv-findcount"}
            aria-live="polite" aria-atomic="true">
            {findStatus}
          </output>
          <button type="button" className="conv-findbtn"
            disabled={findMatches.length === 0}
            aria-label="Previous match" title="Previous match (Shift+Enter)"
            onClick={() => moveFind(-1)}>
            <Icon name="chevron-up" />
          </button>
          <button type="button" className="conv-findbtn"
            disabled={findMatches.length === 0}
            aria-label="Next match" title="Next match (Enter)"
            onClick={() => moveFind(1)}>
            <Icon name="chevron-down" />
          </button>
          <button type="button" className="conv-findbtn"
            aria-label="Close find" title="Close find (Escape)" onClick={closeFind}>
            <Icon name="close" />
          </button>
        </div>
      )}

      {/* THE QUESTION THE VISIBLE REGION IS THE ANSWER TO. A long turn pushes
          the prompt that started it off the top, and what is left on screen is
          an answer with no question — this names it again, in one line, with
          the reader's place in the run and a step either way.

          THE STACK, DECIDED: this is a rail ABOVE the scroller, not a second
          sticky inside it. The plan header (.conv-plan) keeps its own
          position:sticky at top:0 and slides UNDER this bar's bottom edge as
          the transcript moves. Two stickies at top:0 in one scroller would
          have to know each other's heights — and .conv-plan is a <details>
          whose height changes when the reader opens it — so the offset would
          be wrong for exactly as long as the transcript is interesting. Out
          here there is no offset to get wrong and no z-index contest: bar,
          then plan header, then transcript, top to bottom, always.

          It hides itself the moment the newest prompt is on screen, so a short
          session and a reader at the end of a long one both see nothing. */}
      {anchor !== null && (
        <div className="conv-anchor">
          {/* A prompt, not a play. `play` is spoken for twice on this surface —
              Steer and the queue's Send now, one act said the same way twice —
              and this bar points BACK at something the reader already sent. */}
          <Icon name="comment" className="conv-anchor-glyph" sm />
          {/* The whole prompt is the tooltip, because the line is one line. */}
          <button type="button" className="conv-anchor-text" title={anchor.text}
            onClick={() => { goToPrompt(anchor.key); }}>
            {anchor.text}
          </button>
          {/* "3/7" is a glance, not a sentence: the label is what a screen
              reader says instead of spelling the slash. */}
          <span className="conv-anchor-ord"
            aria-label={`Prompt ${anchor.ordinal} of ${anchor.total}`}>
            {anchor.ordinal}/{anchor.total}
          </span>
          <button type="button" className="conv-anchor-step"
            disabled={anchor.prevKey === null}
            aria-label="Previous prompt" title="Previous prompt"
            onClick={() => { if (anchor.prevKey !== null) goToPrompt(anchor.prevKey); }}>
            <Icon name="chevron-left" sm />
          </button>
          <button type="button" className="conv-anchor-step"
            disabled={anchor.nextKey === null}
            aria-label="Next prompt" title="Next prompt"
            onClick={() => { if (anchor.nextKey !== null) goToPrompt(anchor.nextKey); }}>
            <Icon name="chevron-right" sm />
          </button>
        </div>
      )}

      {/* The scroller and everything that floats over it, in one positioned
          box that ends where the transcript ends. Its two children are the
          .conv-list below and the jump capsule after it; the contents of the
          list keep their original indentation, so the wrapper reads as a
          one-line addition in the diff rather than a 400-line reflow. */}
      <div className="conv-scroll">
      {/* tabIndex=-1: not in the tab order, but a legal focus target for the
          one script that needs one — see goToPrompt, where the control being
          pressed is about to unmount and focus has to land somewhere real. */}
      <div className="conv-list" ref={listRef} onScroll={onScroll} tabIndex={-1}
        role="log" aria-label="Conversation transcript" aria-live="off">
        <div className="conv-column">
          {/* How a terminal opens: who is here, what it is running, and where.
              CLI chrome only — refined keeps its cards, and the pane's own empty
              hero still handles a session with nothing in it. */}
          {cliChrome && welcome !== null && welcome.title !== "" && (
            <div className="conv-welcome">
              <span className="conv-welcome-mark" aria-hidden="true">
                <ProviderLogo engine={props.config.engine ?? ""}
                  label={props.config.engineLabel ?? ""} size={28} />
              </span>
              <span className="conv-welcome-title">{welcome.title}</span>
              {welcome.detail !== "" && (
                <span className="conv-welcome-detail">{welcome.detail}</span>
              )}
              {welcome.path !== "" && (
                <span className="conv-welcome-path">{welcome.path}</span>
              )}
            </div>
          )}
          {plan !== undefined && plan.length > 0 && (() => {
            const completed = plan.filter(entry => entry.status === "completed").length;
            const current = plan.some(entry => entry.status === "in_progress");
            return <details className="conv-plan" open={current}>
              <summary className="conv-plan-summary">
                <Icon name="checklist" className="conv-kind" />
                Task plan
                <span className="conv-plan-progress">{completed}/{plan.length} complete</span>
              </summary>
              <ol className="conv-plan-list">
                {plan.map((entry, index) => <li key={`${index}-${entry.content}`}
                  className={`conv-plan-item ${entry.status === "in_progress" ? "is-current"
                    : entry.status === "completed" ? "is-complete" : ""}`}>
                  <Icon name={entry.status === "completed" ? "pass"
                    : entry.status === "in_progress" ? "sync" : "circle-outline"}
                    className="conv-plan-mark" />
                  <span>{entry.content}</span>
                  <span className="conv-plan-priority">{entry.priority}</span>
                </li>)}
              </ol>
            </details>;
          })()}
          {/* The history states come first because they stand where the missing
              messages would: above everything already loaded. Live items render
              underneath regardless, so a slow read never holds up the turn the
              user is watching right now. */}
          {/* Two different waits wearing one sentence until now. With nothing on
              screen there is no "earlier" for messages to be earlier THAN: this
              is the session opening, and it says so over the shape of what is
              coming. Once the pane holds something, the same status really is a
              page of older messages arriving above it, and keeps its spinner —
              a short, genuinely indeterminate wait that moves nothing. */}
          {history.status === "loading" && (rows.length === 0 ? (
            <div role="status">
              <p className="conv-hist">Loading conversation…</p>
              <TranscriptSkeleton />
            </div>
          ) : (
            <p className="conv-hist" role="status">
              {/* Declared in codicons.css, and theme.css already stops the spin
                  under prefers-reduced-motion. */}
              <Icon name="loading" className="codicon-modifier-spin" />
              Loading earlier messages…
            </p>
          ))}
          {history.status === "error" && (
            <div className="conv-hist conv-hist-fail" role="alert">
              <Icon name="error" className="conv-glyph-fail" />
              <span className="conv-hist-msg">{history.message}</span>
              {props.onRetryHistory !== undefined && (
                <button type="button" className="btn" onClick={props.onRetryHistory}>
                  Retry
                </button>
              )}
            </div>
          )}
          {history.status === "unavailable" && (
            <p className="conv-hist" role="status">
              <Icon name="info" className="conv-kind" />
              {history.message}
            </p>
          )}
          {/* With the count, because "some messages are missing" is not a fact
              anyone can act on and "853 older messages are not shown" is. */}
          {history.status === "ready" && (() => {
            const notice = truncationNotice(history.olderMessages, history.truncated);
            if (notice === null) return null;
            const canLoadOlder = props.onLoadOlderHistory !== undefined
              && history.canLoadOlder !== false;
            return (
              <>
                <div className="conv-hist" role="status">
                  <Icon name={history.loadingOlder === true ? "loading" : "ellipsis"}
                    className={history.loadingOlder === true ? "codicon-modifier-spin" : "conv-kind"} />
                  <span className="conv-hist-msg">
                    {history.loadingOlder === true ? "Loading earlier messages…" : notice}
                  </span>
                  {canLoadOlder && (
                    <button type="button" className="btn"
                      disabled={history.loadingOlder === true}
                      onClick={props.onLoadOlderHistory}>
                      {history.loadingOlder === true ? "Loading…" : "Load older"}
                    </button>
                  )}
                </div>
                {history.loadOlderError !== undefined && (
                  <div className="conv-hist conv-hist-fail" role="alert">
                    <Icon name="error" className="conv-glyph-fail" />
                    <span className="conv-hist-msg">{history.loadOlderError}</span>
                  </div>
                )}
              </>
            );
          })()}

          {/* One list, one order, decided by mergeTranscript — including where
              the seam goes. Nothing is sorted here. */}
          {displayRows.map((display, index) => {
            // The live turn draws its own rows, under its prompt.
            if (liveFrom >= 0 && index > liveFrom) return null;
            if (threadView !== null) return threadRow(display, threadView);
            if (display.kind === "tool-run") {
              const finalKey = display.rows[display.rows.length - 1]?.key;
              return <div key={display.key}>
                <ToolRun run={display} all={itemContext} find={findRender}
                  density={density} densityEpoch={densityEpoch} />
                {finalKey !== undefined && finishedElapsed[finalKey] !== undefined
                  && <TurnClock ms={finishedElapsed[finalKey]} cliChrome={cliChrome} />}
                {forkRow(finalKey)}
              </div>;
            }
            if (display.kind === "file-change") {
              const finalKey = display.keys[display.keys.length - 1];
              return <div key={display.key}>
                <FileChangeRow display={display} all={itemContext} find={findRender}
                  density={density} densityEpoch={densityEpoch} />
                {finishedElapsed[finalKey] !== undefined
                  && <TurnClock ms={finishedElapsed[finalKey]} cliChrome={cliChrome} />}
                {forkRow(finalKey)}
              </div>;
            }
            if (display.kind === "subagent-grid") {
              return <SubagentGrid key={display.key} run={display} all={itemContext}
                find={findRender} />;
            }
            if (display.kind === "edited-files") {
              return <EditedFilesRow key={display.key} run={display} find={findRender}
                pathRoot={props.pathRoot} />;
            }
            const row = display.row;
            return row.kind === "mark"
              ? <div key={row.key} className="conv-mark">{row.label ?? "New messages"}</div>
              : <div key={row.key}>
                  <SearchableItem item={row.item} all={itemContext}
                    findKey={row.key} find={findRender} density={density}
                    densityEpoch={densityEpoch}
                    followingDiffPaths={diffPaths.get(row.key)}
                    turnChanges={turnChanges.get(row.key)}
                    onRequestRestore={restoreReady ? setPendingRestore : undefined}
                    threadTurn={threadControls.get(row.key)}
                    onThreadTurn={threadControls.size > 0 ? onThreadTurn : undefined} />
                  {/* A clock only under a finished turn: no fiber per row for nothing. */}
                  {finishedElapsed[row.key] !== undefined
                    && <TurnClock ms={finishedElapsed[row.key]} cliChrome={cliChrome} />}
                  {forkRow(row.key)}
                </div>;
          })}

          {/* An Away loop's report, or its status while it runs, ends its thread. */}
          {threadView !== null && awayReportView !== null && (
            <AwayReportCard report={awayReportView}
              onAction={props.onThreadAway === undefined ? undefined : onThreadAway} />
          )}

          {/* The genuinely empty session — not "still loading" and not "the read
              failed", both of which say their own piece above. */}
          {rows.length === 0 && !thinking
            && (history.status === "idle" || history.status === "ready") && (
            <div className="empty conv-hero">
              <p className="conv-hero-wordmark"><ProductName /></p>
              <p className="conv-hero-copy">No messages yet.</p>
            </div>
          )}
        </div>
      </div>

      {/* THE ONE EXCEPTION TO THE PAUSE. Scrolling up stops the stream from
          moving the view, and nothing here overrides that — but a pending
          approval has stopped the AGENT, and a reader who cannot see the card
          waits for a run that is waiting for them. So the capsule they already
          have changes what it says instead of the transcript changing where it
          sits: the count gives way to "Approval waiting", in the same amber the
          card itself wears, one click from the request. Yanking the scroller
          would have been the other answer and it is the wrong one — it takes
          the reader's place away to tell them something a label can say. The
          pending card also carries role="alert", so a screen reader is told the
          moment it renders whatever the scroller is doing. */}
      {/* A Threads pane's live turn rides on the capsule while the reader is
          away from it: the Wave, the step's short line and the run's clock.
          It still jumps to the bottom and resumes following. */}
      {awayFromLatest && livePill !== null && (
        <button type="button" className="conv-jump-latest is-live" onClick={jumpToLatest}
          title={`Jump to latest: ${livePill.line}`} aria-label={`Jump to latest: ${livePill.line}`}>
          <ThreadLivePill header={livePill} startedAt={liveRunStart} />
        </button>
      )}
      {awayFromLatest && livePill === null && (
        <button type="button"
          className={jumpBacklog.approvalWaiting
            ? "conv-jump-latest is-waiting" : "conv-jump-latest"}
          onClick={jumpToLatest} title={jumpLabel.aria} aria-label={jumpLabel.aria}>
          <Icon name="chevron-down" sm />
          <span className="conv-jump-full">{jumpLabel.text}</span>
          <span className="conv-jump-short">{jumpLabel.short}</span>
        </button>
      )}
      </div>{/* /.conv-scroll */}

      <div className="conv-sr" role="status" aria-live="polite" aria-atomic="true">
        {turnAnnouncement}
      </div>

      {(runningAgents.length > 0 || workingStrip || queueItems.length > 0 || queueError !== null) && (
        <div className="conv-status-sink">
      {runningAgents.length > 0 && (
        <div className="conv-subrun" role="status" aria-live="polite"
          aria-label={runningAgents.length === 1
            ? "1 subagent running"
            : `${runningAgents.length} subagents running`}>
          {runningAgents.map(agent => {
            const selected = props.selectedSubagentKey === agent.id;
            const select = props.onSelectSubagent;
            return (
              <button key={agent.id} type="button"
                className={selected ? "sub-mini is-selected" : "sub-mini"}
                aria-label={`${agent.subagentType} running`}
                title={agent.description.trim() !== ""
                  ? `${agent.subagentType} — ${agent.description}` : agent.subagentType}
                aria-current={selected ? "true" : undefined}
                disabled={select === undefined}
                onClick={() => { select?.(agent.id, false); }}>
                <WorkingIndicator />
                <span className="sub-mini-type">{agent.subagentType}</span>
              </button>
            );
          })}
        </div>
      )}

      {workingStrip && liveStartedAt !== null && (
        <div className="conv-working" role="status" aria-live="polite"
          data-provider={providerKindFromSession(props.config.engine, sessionKey)}>
          {/* Clean uses the shared wave; Terminal keeps its provider mark. */}
          <WorkingIndicator className="conv-working-mark" compact={false}>
            <ProviderLogo engine={props.config.engine ?? ""}
              label={props.config.engineLabel ?? ""} size={14} />
          </WorkingIndicator>
          {/* CLI folds the clock into the sentence; refined keeps the word and
              the clock in two spans, exactly as it shipped. Either way the
              VISIBLE text is aria-hidden and one static word is announced
              beside it: this strip is a polite live region and the clock
              reticks every second, so an announced clock would say the same
              sentence over and over for the length of the turn.

              The VISIBLE word is the live one the shell computed — "Thinking",
              the tool that is running — and only the announced span stays the
              static "Working", for the same reason: four tiles all saying
              "Working" tell the reader nothing about which is on a 90-second
              test run. */}
          {!cliChrome && (
            <span className="conv-working-label" aria-hidden="true">{workingWord}</span>
          )}
          <LiveTurnElapsed startedAt={liveStartedAt} cliChrome={cliChrome}
            label={workingWord} />
          <span className="conv-sr">Working</span>
          {/* In the Threads layout the composer's round Stop is the one
              Stop (redesign, 2026-10-06); two would read as two actions. */}
          {!props.threadLayout && (
            <button type="button" className="conv-iconbtn conv-working-stop"
              onClick={props.onStop} aria-label="Stop" title="Stop">
              <Icon name="debug-stop" />
            </button>
          )}
        </div>
      )}

      {(queueItems.length > 0 || queueError !== null) && (
        <div className="conv-queue-slot">
          {queueError !== null && (
            <div className="conv-queue-error" role="alert">
              <Icon name="error" className="conv-glyph-fail" />
              <span>{queueError}</span>
            </div>
          )}
          {queueItems.length > 0 && (
            <SteeringQueue
              key={props.sessionKey ?? ""}
              sessionKey={props.sessionKey}
              items={queueItems}
              sendingId={props.queueSendingId}
              disabled={!queueControlsReady || props.queueSendingId != null
                || props.queueControlsLocked === true}
              label="Queued follow-ups"
              compact={threadLayout}
              onMove={props.onQueueMove ?? NO_QUEUE_MOVE}
              onEdit={props.onQueueEdit ?? NO_QUEUE_TEXT}
              onRemove={props.onQueueRemove ?? NO_QUEUE_ID}
              onSendNow={props.onQueueSendNow ?? NO_QUEUE_ID}
              onClear={props.onQueueClear ?? NO_QUEUE_CLEAR}
              onEditingChange={props.onQueueEditing}
            />
          )}
        </div>
      )}
        </div>
      )}

      {/* Restoring overwrites files on disk outside this transcript, so it is
          the app's own destructive-action dialog rather than a second pattern
          invented here. Mounted only while a confirmation is open — see
          pendingRestore for why one dialog serves every turn. */}
      {confirmingRestore !== null && (
        <ConfirmDialog
          open
          title="Restore files to before this message?"
          description={confirmingRestore.point.description}
          confirmLabel="Restore files"
          tone="danger"
          onConfirm={() => {
            setPendingRestore(null);
            props.onRestoreTurn?.(confirmingRestore.key);
          }}
          onCancel={() => { setPendingRestore(null); }}
        />
      )}
      {pendingEdit !== null && props.onThreadEditFrom !== undefined && (
        <ThreadEditDialog
          ordinal={pendingEdit.ordinal}
          revertFilesBlocked={thread?.revertFilesBlocked ?? null}
          onChoose={async restoreFiles => {
            await props.onThreadEditFrom?.(pendingEdit.ordinal, restoreFiles, pendingEdit.prompt);
            setPendingEdit(null);
          }}
          onCancel={() => { setPendingEdit(null); }}
        />
      )}
    </>
  );
});

const NO_THREAD_ENDS: ReadonlyMap<string, ThreadTurn> = new Map();
const NO_TURN_CHANGES: ReadonlyMap<string, TurnChangeSummary> = new Map();
const NO_THREAD_ROW_KEYS: ReadonlyMap<number, string> = new Map();
const NO_STAMP_KEYS: ReadonlySet<string> = new Set();
