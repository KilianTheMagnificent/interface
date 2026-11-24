import { KeptPromptLine, keptPromptDraft, keptPromptPaths } from "./PromptRecoveryPanel";
import type { VisualFrameLoader } from "../visual-reply";
import type { RecoveryMode, RecoveryResponse } from "../../shared/recovery";
import { memo, useEffect, useMemo, type ComponentProps, type Dispatch, type SetStateAction } from "react";
import type { EngineEvent, SessionKey } from "../../shared/engine";
import type { AgentControlResponse } from "../../shared/agents";
import { parseProviderCommandLike } from "../../shared/provider-commands";
import { agentApprovalGateFor, agentPastThread } from "../agent-roster";
import { activeApprovalsFor } from "../app-session-projection";
import type { ApprovalVerdict } from "../approval-keys";
import type { ConversationThread, SessionConfigView } from "../conversation-types";
import type { DraftStore } from "../draft-store";
import { withoutKey } from "../helpers";
import {
  promptContextCapabilities, type PromptContextItem, type PromptContextKind,
  type PromptTurnAttachment,
} from "../prompt-context";
import type { PromptDropPath } from "../prompt-drop";
import type { PastedFile } from "../prompt-paste";
import {
  clearSteeringQueue, editSteeringPrompt, moveSteeringPrompt, removeSteeringPrompt,
  type SteeringQueueItem, type SteeringQueueMove,
} from "../steering-queue";
import type { AwayCardAction } from "../away-thread";
import type { ShipNext } from "../thread-ship";
import type { ThreadLimitOffer, ThreadLimitReset, ThreadOpinionCard } from "../thread-opinion";
import type { UsageHeadroom } from "../usage-view";
import type { PendingEcho } from "../transcript-merge";
import { AgentPastThreadNotice } from "./AgentWorkspacePanel";
import { ProjectedConversation } from "./ProjectedConversation";
import { ThreadOpinionCards, type ThreadOpinionAction } from "./ThreadOpinionCards";
import { StartedThreads } from "./StartedThreads";
import type { StartedThreadsLine } from "../started-threads";
import type { ThreadStashState } from "../use-draft-stash";
import type { ConversationStash } from "./Conversation";
import { ThreadScheduleLine } from "./ThreadScheduleLine";
import type { ScheduleComposerLine } from "../thread-schedules";
import type { ThreadScheduleView } from "../../shared/threads";
import type { ThreadSecretRequest } from "../../shared/thread-secrets";
import { ThreadSecretCard } from "./ThreadSecretCard";
import type { FolderTrustRow } from "../folder-trust";

/**
 * One workspace tile's conversation (`SessionTile`) and the return recap strip
 * it carries. App derives every value a tile shows during its own render and
 * hands its callbacks over as one committed actions object, so this component
 * never reads App state or `window.iface` itself.
 */

const NO_QUEUE: readonly SteeringQueueItem[] = [];
const NO_OPINIONS: readonly ThreadOpinionCard[] = [];
const NO_PENDING: readonly string[] = [];
const NO_APPROVAL_IDS: ReadonlySet<string> = new Set<string>();

/* The return recap (see ./recap): one dim line directly above the composer of
   the pane the user just came back to, on the transcript's own reading measure.

   IT RIDES THE PANE'S beforeComposer SLOT, so the measure is --conv-measure /
   --conv-gutter — declared on .conv, used by the composer this line now sits
   directly above, and already overridden for CLI chrome. The strip is still a
   DOM descendant of .tile-stack, so the private --recap-* pair that used to
   live there would have kept resolving; it was simply a third copy of two
   numbers that already agree twice, free to drift from both. Reading the
   composer's own tokens makes "the same measure" true by construction rather
   than by three declarations staying in step.

   The stack is unconditional. A wrapper mounted only while a recap exists would
   move ProjectedConversation in the React tree and remount the pane — throwing
   away the draft and the scroll position the user is returning TO, to tell them
   what they missed. So the wrapper is always there and only the strip comes and
   goes.

   Nothing here animates. A line that is gone in thirty seconds cannot afford to
   spend the first of them arriving, and with no motion at all there is no
   reduced-motion branch to get wrong. */
export const RECAP_CSS = `
.tile-stack {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  min-width: 0;
}
/* .conv asks for height:100%, which in a column flex parent is the full track.
   Scoped to the stack so the pane renders exactly as it always did anywhere
   else. */
.tile-stack > .conv { flex: 1 1 auto; height: auto; min-height: 0; }
/* The permanent, usually empty live region. NEVER display:none and never
   visibility:hidden: either one takes it out of the accessibility tree, and a
   live region that is not in the tree announces nothing — which is the whole
   reason the node is permanent. Empty it has no box worth the name, and
   .conv-strip is a plain block with no gap, so it costs nothing on screen. */
.tile-recap-live { display: block; }
.tile-recap {
  flex: none;
  box-sizing: border-box;
  display: flex;
  align-items: center;
  gap: 6px;
  width: min(calc(100% - 2 * var(--conv-gutter)), var(--conv-measure));
  margin: 0 auto;
  padding: 5px 0;
  color: var(--fg-dim);
  font-size: var(--text-ui);
  line-height: var(--lh-ui);
  border-top: var(--border-w) solid var(--border);
}
.tile-recap-glyph { flex: none; display: block; opacity: 0.8; }
.tile-recap-text {
  flex: 1 1 auto;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.tile-recap-close {
  flex: none;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  min-width: 24px;
  min-height: 24px;
  padding: 0;
  color: inherit;
  background: none;
  border: 0;
  border-radius: var(--radius-control);
  cursor: pointer;
}
.tile-recap-close:hover { color: var(--fg-strong); }
.tile-recap-close:focus-visible {
  outline: var(--focus-ring-w) solid var(--focus);
  outline-offset: var(--focus-ring-offset);
}
`;

type PaneProps = ComponentProps<typeof ProjectedConversation>;

/** App's key-first session callbacks, handed to every pane as one committed,
 * never-changing object (useCommittedActions). */
export type SessionTileActions = {
  /** Protocol 67: the private secret card's Give (answers why it was refused, or null) and Decline. */
  giveSecret: (key: SessionKey, id: string, value: string) => Promise<string | null>;
  /** Protocol 67: `/btw` asks a side question (answers why it was refused, or null). */
  askSideQuestion: (key: SessionKey, question: string) => Promise<string | null>;
  declineSecret: (key: SessionKey, id: string) => void;
  readPromptRecovery: (key: SessionKey) => Promise<RecoveryResponse>;
  forgetPromptRecovery: (key: SessionKey, id: string) => Promise<void>;
  recoverPrompt: (key: SessionKey, mode: RecoveryMode, id?: string, expectedPrompt?: string) => Promise<void>;
  dismissRecap: (key: SessionKey) => void;
  appendConversationEcho: (key: SessionKey, echo: PendingEcho) => void;
  checkpointTurn: (key: SessionKey, turnKey: string | null) => void;
  failConversationEcho: (key: SessionKey, id: string, error: string) => void;
  retryHistoryFor: (key: SessionKey) => void;
  loadOlderHistoryFor: (key: SessionKey) => void;
  changeModelFor: (key: SessionKey, model?: string) => void;
  changeEffortFor: (key: SessionKey, effort: string | null) => void;
  changeFastModeFor: (key: SessionKey, enabled: boolean) => void;
  changePermissionModeFor: (key: SessionKey, mode: string) => void;
  openSubagentDetail: (key: SessionKey, agentKey: string, viaKeyboard: boolean) => void;
  setSessionContextOpen: Dispatch<SetStateAction<Record<SessionKey, boolean>>>;
  requestPromptContext: (target: string, kind: PromptContextKind, cwd: string) => void;
  removeContextFor: (target: string, id: string) => void;
  consumeSessionContext: (key: SessionKey, consumed: readonly PromptContextItem[]) => void;
  attachDroppedPaths: (target: string, paths: readonly PromptDropPath[]) => void;
  attachPastedImages: (target: string, files: readonly PastedFile[]) => Promise<readonly string[]>;
  enqueue: (key: SessionKey, text: string, context: readonly PromptContextItem[]) => boolean;
  applyQueueUpdate: (key: SessionKey,
    update: (items: readonly SteeringQueueItem[]) => SteeringQueueItem[]) => boolean;
  sendQueuedNow: (key: SessionKey, id: string) => void;
  /** A queued row's editor opened or closed; delivery holds at an open one. */
  setQueueEditing: (key: SessionKey, id: string, editing: boolean) => void;
  submitPrompt: (key: SessionKey, text: string,
    context: readonly PromptContextItem[]) => Promise<void>;
  runCommand: (key: SessionKey, text: string) => Promise<void>;
  stopSession: (key: SessionKey) => void;
  showAgentThread: (agentId: string) => void;
  approveFor: (key: SessionKey, approvalId: string, answers?: Record<string, string>) => void;
  denyFor: (key: SessionKey, approvalId: string, note?: string) => void;
  restoreTurnCheckpoint: (key: SessionKey, turnKey: string) => void;
  openThreadTurnDiff: (key: SessionKey, ordinal: number) => void;
  editThreadFromTurn: (key: SessionKey, ordinal: number, restoreFiles: boolean,
    prompt: string) => Promise<void>;
  forkThreadAtTurn: (key: SessionKey, keepTurns: number) => void;
  /** Shipping from the latest changes card: its next step, or "read" (protocol 37). */
  shipThread: (key: SessionKey, step: ShipNext | "read") => void;
  /** An Away loop's thread cards: stop the loop, open a review, review the changes. */
  awayThread: (key: SessionKey, action: AwayCardAction) => void;
  /** The pane's turn → prompt row join, for the diff panel's Undo. */
  reportThreadTurnRows: (key: SessionKey, rowKeys: ReadonlyMap<number, string>) => void;
  /** A turn failed because the provider's CLI is signed out: open its sign-in. */
  signInFor: (key: SessionKey) => void;
  /** A second opinion's card button (review B4). */
  opinionAction: (key: SessionKey, card: ThreadOpinionCard, action: ThreadOpinionAction) => void;
  /** Switch the thread to another provider, from the limit offer (review E, protocol 59). */
  switchThreadProvider: (key: SessionKey, engine: string) => void;
  /** Continue on another account of the thread's provider, from the limit offer (protocol 60). */
  continueThreadOnAccount: (key: SessionKey, engine: string, account: string) => void;
  /** Preview a file the thread changed beside it (review B5). */
  previewThreadFile: (key: SessionKey, path: string) => void;
  /** Open a thread this one's agent started (team tools, protocol 57). */
  openStartedThread: (key: SessionKey) => void;
  /** Change one of the thread's scheduled prompts (protocol 62). */
  editSchedule: (key: SessionKey, schedule: ThreadScheduleView) => void;
  /** Delete a scheduled prompt (its line's Cancel). */
  cancelSchedule: (id: string) => void;
  /** Resume at reset, from the limit line (protocol 66). */
  resumeThreadAtReset: (key: SessionKey) => void;
  /** Snooze until reset, from the limit line (protocol 66): snooze until `until` (ms). */
  snoozeThreadUntil: (key: SessionKey, until: number) => void;
  /** Trust folder, from an untrusted thread's line (protocol 72). */
  trustThreadFolder: (key: SessionKey, folder: string) => void;
  /** Preview an attached file, folder, diff or image in the side-by-side preview panel. */
  previewAttachment: (
    key: SessionKey,
    attachment: PromptTurnAttachment | PromptContextItem,
    projectRoot: string,
  ) => void;
  /** A visual reply's page, read and framed (T3 section 8); and its release. */
  loadVisual: VisualFrameLoader["load"];
  releaseVisual: VisualFrameLoader["release"];
  /** The pane's two host reads: pre-launch command discovery and a dropped
      file's path. App answers both, as the one `window.iface` caller. */
  loadProviderCommands: NonNullable<PaneProps["loadProviderCommands"]>;
  pathForFile: NonNullable<PaneProps["pathForFile"]>;
  /** A thread's stashed drafts (T3 section 9): read, stash, restore, remove. */
  loadStash: (key: SessionKey) => void;
  stashDraft: (key: SessionKey) => void;
  restoreStash: (key: SessionKey, id: string) => void;
  removeStash: (key: SessionKey, id: string) => void;
};

/** One pane's inputs as values a shallow compare can hold: primitives, this
 * session's own slices of App state, and stable constants. Facts App keeps in
 * refs (liveness, history paging, in-flight sends and config) are read during
 * App's render and passed in, because the tile skips renders that change none
 * of these props and would otherwise show a stale ref. */
export interface SessionTileProps {
  recoveryWake?: number;
  sessionKey: SessionKey;
  title: string;
  cwd: string;
  pathRoot: string;
  folderScoped: boolean;
  /** App's one draft store; the pane reads and writes its session's entry. */
  drafts: DraftStore;
  recap: string | undefined;
  pendingEchoes: PaneProps["pendingEchoes"];
  events: EngineEvent[];
  commands: PaneProps["commands"];
  engineVersion: string | undefined;
  planName: string | undefined;
  history: PaneProps["history"];
  historyFrozen: boolean;
  focused: boolean;
  thinking: boolean;
  workingLabel: PaneProps["workingLabel"];
  turnStartedAt: PaneProps["turnStartedAt"];
  live: boolean;
  config: SessionConfigView;
  contextItems: readonly PromptContextItem[];
  contextLocked: boolean;
  contextOpen: boolean;
  selectedSubagentKey: string | undefined;
  queue: readonly SteeringQueueItem[];
  queueSendingId: string | null;
  queueError: string | null;
  queueControlsLocked: boolean;
  sendLocked: boolean;
  queueWhileBusy: boolean;
  canSteer: boolean;
  approvals: Record<string, ApprovalVerdict>;
  agentStore: AgentControlResponse;
  agentStartKnownKeys: ReadonlySet<SessionKey> | null;
  turnRestorePoints: PaneProps["turnRestorePoints"];
  restoringTurnKey: string | null;
  /** A Threads pane's turn records and what its thread allows; absent elsewhere. */
  thread: ConversationThread | undefined;
  /** True for a pane in the Threads grid: the redesigned turn and composer layout. */
  threadLayout: boolean;
  /** A prompt the shell asks this pane to send once (Fix failing checks). */
  threadSend?: { id: number; text: string };
  /** The provider's plan headroom, quiet on the model chip (review E). */
  headroom?: UsageHeadroom;
  /** Second opinions this thread asked for (review B4); one stable array per thread. */
  opinions?: readonly ThreadOpinionCard[];
  /** Cards with an action on its way. */
  opinionPending?: readonly string[];
  /** The thread's provider hit its limit: continue elsewhere (review E). */
  limitOffer?: ThreadLimitOffer | null;
  /** The limit's known reset: Resume at reset and Snooze until reset (protocol 66). */
  limitReset?: ThreadLimitReset | null;
  /** Protocol 72: the thread's folder is not trusted; its line. */
  folderTrust?: FolderTrustRow | null;
  /** The threads this thread's agent started (protocol 57); absent when none. */
  startedThreads?: StartedThreadsLine;
  /** A Threads pane's stashed drafts (T3 section 9); absent offers no stash. */
  stash?: ThreadStashState;
  /** The context picker's Thread kind is offered (a project thread, T3 section 9). */
  canReferThreads?: boolean;
  /** The thread's soonest scheduled prompt (protocol 62); absent when none. */
  scheduleLine?: ScheduleComposerLine;
  /** The view's clock, for the line's words. */
  scheduleNow?: number;
  /** Protocol 67: the secret this thread's agent asks for; absent when none. */
  secretRequest?: ThreadSecretRequest;
  actions: SessionTileActions;
}

/**
 * One session's transcript, exactly as a workspace tile draws it. Memoized:
 * App re-renders for every event batch, registry push, voice frame and
 * keystroke, and a pane whose own inputs did not change skips all of them.
 */
export const SessionTile = memo(function SessionTile({
  recoveryWake = 0, sessionKey: key, title, cwd, pathRoot, folderScoped, drafts, recap: tileRecap, pendingEchoes,
  events, commands, engineVersion, planName, history, historyFrozen, focused, thinking,
  workingLabel, turnStartedAt, live: tileLive, config, contextItems: tileContext, contextLocked,
  contextOpen, selectedSubagentKey, queue: tileQueue, queueSendingId, queueError, queueControlsLocked,
  sendLocked, queueWhileBusy, canSteer, approvals: verdicts, agentStore, agentStartKnownKeys,
  turnRestorePoints, restoringTurnKey, thread, threadLayout, threadSend, headroom,
  opinions = NO_OPINIONS, opinionPending = NO_PENDING, limitOffer = null, limitReset = null, folderTrust = null,
  startedThreads,
  scheduleLine, scheduleNow = 0, actions, stash: stashState, canReferThreads = false, secretRequest,
}: SessionTileProps) {
  const {
    readPromptRecovery, recoverPrompt, forgetPromptRecovery, dismissRecap, appendConversationEcho, checkpointTurn,
    failConversationEcho, retryHistoryFor, loadOlderHistoryFor, changeModelFor, changeEffortFor,
    changeFastModeFor, changePermissionModeFor, openSubagentDetail, setSessionContextOpen,
    requestPromptContext, removeContextFor, consumeSessionContext, attachDroppedPaths,
    attachPastedImages, enqueue,
    applyQueueUpdate, sendQueuedNow, setQueueEditing, submitPrompt, runCommand,
    stopSession, showAgentThread, approveFor, denyFor, restoreTurnCheckpoint,
    openThreadTurnDiff, editThreadFromTurn, forkThreadAtTurn, reportThreadTurnRows, shipThread,
    awayThread, loadProviderCommands, pathForFile, signInFor, opinionAction, switchThreadProvider,
    continueThreadOnAccount,
    previewThreadFile, previewAttachment, openStartedThread, loadVisual, releaseVisual,
    editSchedule, cancelSchedule, resumeThreadAtReset, snoozeThreadUntil, trustThreadFolder, giveSecret, declineSecret,
    askSideQuestion,
  } = actions;
  const visualFrames = useMemo<VisualFrameLoader>(() => ({ load: loadVisual, release: releaseVisual }),
    [loadVisual, releaseVisual]);
  const effectiveProjectRoot = pathRoot || cwd;
  // T3 section 9: a thread's stash is read when its pane first shows it.
  const stashOffered = stashState !== undefined;
  const { loadStash, stashDraft, restoreStash, removeStash } = actions;
  useEffect(() => { if (stashOffered) loadStash(key); }, [stashOffered, key, loadStash]);
  const stash = useMemo((): ConversationStash | undefined => stashState === undefined ? undefined : {
    entries: stashState.entries,
    busy: stashState.busy,
    onStash: () => { stashDraft(key); },
    onRestore: id => { restoreStash(key, id); },
    onRemove: id => { removeStash(key, id); },
  }, [stashState, key, stashDraft, restoreStash, removeStash]);
  const contextCapabilities = useMemo(() => (folderScoped
    ? { ...promptContextCapabilities(cwd),
      ...(canReferThreads ? { chat: { available: true } } : {}) }
    : undefined), [folderScoped, cwd, canReferThreads]);
  // A past agent thread is read-only: no composer (sending would resume it),
  // no queue to send from, and no verdicts. The host refuses all three too.
  const pastThread = agentPastThread(agentStore, key);
  // The kept prompt (owner, 2026-11-15): one line inside the composer's top
  // edge, no longer a strip across the pane. Memoized so a streamed event
  // leaves the composer alone; Put it back writes this pane's draft and
  // attaches the kept files and folders again through the drop path, which
  // asks the host for access as a drop does.
  const lastTurnState = thread?.turns.at(-1)?.state;
  const recoveryInterrupted = pastThread === null
    && (lastTurnState === "failed" || lastTurnState === "interrupted");
  const recoveryDisabled = thinking || sendLocked || queueWhileBusy || contextLocked;
  const canRecover = pastThread === null;
  const composerTop = useMemo(() => (
    <KeptPromptLine sessionKey={key} wake={recoveryWake}
      interrupted={recoveryInterrupted}
      disabled={recoveryDisabled}
      read={readPromptRecovery} recover={canRecover ? recoverPrompt : undefined} forget={forgetPromptRecovery}
      restore={receipt => {
        drafts.set(key, keptPromptDraft(drafts.get(key), receipt.input.text));
        const paths = keptPromptPaths(receipt.input.context);
        if (folderScoped && paths.length > 0) attachDroppedPaths(key, paths);
      }}
      preview={item => { previewAttachment(key, item, effectiveProjectRoot); }} />
  ), [key, recoveryWake, recoveryInterrupted, recoveryDisabled, readPromptRecovery, canRecover, recoverPrompt,
    forgetPromptRecovery, drafts, folderScoped, attachDroppedPaths, previewAttachment, effectiveProjectRoot]);
  return (
    <ProjectedConversation
      sessionKey={key}
      drafts={drafts}
      // The recap rides the pane's own slot, which draws it
      // directly ABOVE the composer on the transcript's reading
      // measure. Below the pane it read as a footnote to the
      // controls rather than as the last thing said about the
      // conversation. .tile-stack still wraps the pane
      // unconditionally — see RECAP_CSS for why that matters.
      composerTop={composerTop}
      dockCard={secretRequest === undefined ? undefined : (
        <ThreadSecretCard request={secretRequest}
          onGive={(id, value) => giveSecret(key, id, value)}
          onDecline={id => { declineSecret(key, id); }} />
      )}
      beforeComposer={(
        /* THE LIVE REGION IS THE PERMANENT PART. A role="status"
           that arrives already holding its own text announces
           nothing: a live region has to be in the tree BEFORE its
           contents change for the change to be observed at all.
           So this node is mounted for every pane, empty almost
           always, and only the line inside it comes and goes.
           Empty it draws nothing — .conv-strip is a plain block
           with no gap, so a childless div costs zero pixels.

           status and not alert: a briefing about something
           already over waits its turn rather than interrupting
           whatever is being read. */
        <>
        {startedThreads !== undefined && (
          <StartedThreads line={startedThreads} onOpen={openStartedThread} />
        )}
        {scheduleLine !== undefined && (
          <ThreadScheduleLine line={scheduleLine} now={scheduleNow}
            onEdit={schedule => { editSchedule(key, schedule); }} onCancel={cancelSchedule} />
        )}
        {(opinions.length > 0 || limitOffer !== null || limitReset !== null || folderTrust !== null) && (
          <ThreadOpinionCards cards={opinions} limit={limitOffer} pending={opinionPending}
            folderTrust={folderTrust}
            onTrustFolder={folderTrust === null ? undefined : () => { trustThreadFolder(key, folderTrust.folder); }}
            reset={limitReset} onResume={() => { resumeThreadAtReset(key); }}
            onSnooze={until => { snoozeThreadUntil(key, until); }}
            onAction={(card, action) => { opinionAction(key, card, action); }}
            onSwitch={offer => { switchThreadProvider(key, offer.engine); }}
            onContinue={offer => {
              if (offer.account !== undefined) continueThreadOnAccount(key, offer.engine, offer.account);
            }} />
        )}
        <div className="tile-recap-live" role="status">
          {tileRecap !== undefined && (
            <div className="tile-recap">
              <svg className="tile-recap-glyph" width="11" height="11"
                viewBox="0 0 12 12" aria-hidden="true" focusable="false">
                <circle cx="6" cy="6" r="4.75" fill="none"
                  stroke="currentColor" strokeWidth="1" />
                <path d="M6 3.4V6l1.9 1.2" fill="none" stroke="currentColor"
                  strokeWidth="1" strokeLinecap="round" />
              </svg>
              <span className="tile-recap-text" title={tileRecap}>{tileRecap}</span>
              <button type="button" className="tile-recap-close"
                aria-label={`Dismiss the away recap for ${title}`}
                title="Dismiss"
                onClick={() => { dismissRecap(key); }}>
                <span className="codicon codicon-sm codicon-close"
                  aria-hidden="true" />
              </button>
            </div>
          )}
        </div>
        </>
      )}
      pendingEchoes={pendingEchoes}
      onPendingEcho={echo => {
        appendConversationEcho(key, echo);
        // THE ROW KEY, from the pane itself. This callback is the
        // moment the transcript mints the row for a send —
        // `e:<id>` is mergeTranscript's own key for it — and it
        // fires immediately before onSend, so the snapshot is
        // taken with the prompt rather than guessed at from
        // outside. A provider command mints an echo too and is
        // not a prompt that edits files, so it is left out by the
        // same rule the queue uses to refuse one.
        // Wrapped because this runs INSIDE the pane's send: a
        // throw here would abort the dispatch that follows it,
        // and no snapshot is worth a message that never left.
        try {
          if (parseProviderCommandLike(echo.text) === null) {
            checkpointTurn(key, `e:${echo.id}`);
          }
        } catch (err: unknown) {
          console.warn("[app] could not start this turn's checkpoint:", err);
        }
      }}
      onPendingEchoError={(id, error) => {
        failConversationEcho(key, id, error);
      }}
      events={events}
      commands={commands}
      // A relaunched chat has no adapter until its first send, so
      // the live catalogue above is undefined and typing "/" would
      // face an empty menu. Same discovery lane the New Chat card
      // uses, asked for this session's own directory and model.
      loadProviderCommands={loadProviderCommands}
      cwd={cwd}
      engineVersion={engineVersion}
      planName={planName}
      headroom={headroom}
      history={history}
      focused={focused}
      onRetryHistory={historyFrozen ? undefined : () => { retryHistoryFor(key); }}
      onLoadOlderHistory={historyFrozen ? undefined : () => { loadOlderHistoryFor(key); }}
      thinking={thinking}
      workingLabel={workingLabel}
      turnStartedAt={turnStartedAt}
      live={tileLive}
      config={config}
      onSetModel={model => { changeModelFor(key, model); }}
      onSetEffort={effort => { changeEffortFor(key, effort); }}
      onSetFastMode={enabled => { changeFastModeFor(key, enabled); }}
      threadLayout={threadLayout}
      onSetPermissionMode={threadLayout
        ? mode => { changePermissionModeFor(key, mode); } : undefined}
      contextItems={tileContext}
      contextLocked={contextLocked}
      pathRoot={pathRoot}
      selectedSubagentKey={selectedSubagentKey}
      onSelectSubagent={(agentKey, viaKeyboard) => {
        openSubagentDetail(key, agentKey, viaKeyboard);
      }}
      onSignIn={() => { signInFor(key); }}
      contextCapabilities={contextCapabilities}
      stash={stash}
      onSideQuestion={threadLayout && thread !== undefined
        ? question => askSideQuestion(key, question) : undefined}
      contextOpen={contextOpen}
      onContextOpenChange={open => {
        setSessionContextOpen(current => open
          ? { ...current, [key]: true }
          : withoutKey(current, key));
      }}
      onContextRequest={folderScoped
        ? kind => { requestPromptContext(key, kind, cwd); } : undefined}
      onContextRemove={id => { removeContextFor(key, id); }}
      onContextPreview={item => { previewAttachment(key, item, effectiveProjectRoot); }}
      onPreviewAttachment={attachment => { previewAttachment(key, attachment, effectiveProjectRoot); }}
      visualFrames={visualFrames}
      onContextConsumed={consumed => { consumeSessionContext(key, consumed); }}
      onDroppedPaths={folderScoped ? paths => { attachDroppedPaths(key, paths); } : undefined}
      onPastedImages={folderScoped ? files => attachPastedImages(key, files) : undefined}
      pathForFile={pathForFile}
      queueItems={pastThread === null ? tileQueue : NO_QUEUE}
      queueSendingId={queueSendingId}
      queueError={queueError}
      queueControlsLocked={queueControlsLocked}
      sendLocked={sendLocked}
      sendLockReason="Applying session settings…"
      queueWhileBusy={queueWhileBusy}
      canSteer={canSteer}
      onQueue={text => enqueue(key, text, tileContext)}
      onQueueMove={(id, move: SteeringQueueMove) => {
        return applyQueueUpdate(key,
          queue => moveSteeringPrompt(queue, id, move));
      }}
      onQueueEdit={(id, text) => {
        if (parseProviderCommandLike(text) !== null) return false;
        return applyQueueUpdate(key,
          queue => editSteeringPrompt(queue, id, text));
      }}
      onQueueRemove={id => {
        return applyQueueUpdate(key,
          queue => removeSteeringPrompt(queue, id));
      }}
      onQueueSendNow={id => { sendQueuedNow(key, id); }}
      onQueueEditing={(id, editing) => { setQueueEditing(key, id, editing); }}
      onQueueClear={() => {
        return applyQueueUpdate(key, () => clearSteeringQueue());
      }}
      onSend={text => {
        // The briefing has done its job the moment the user types
        // back into the pane; leaving it up would make a summary
        // of the past sit under a prompt about the present.
        dismissRecap(key);
        // Reserve the captured "next prompt" context immediately
        // so a second queued prompt cannot attach the same items
        // while validation is still in flight. Rejection restores
        // them by id without removing anything added meanwhile.
        consumeSessionContext(key, tileContext);
        // The lane resolves an unknown outcome and restores the context
        // only on a definite refusal (outbound-lane.ts).
        return submitPrompt(key, text, tileContext);
      }}
      onRunCommand={text => runCommand(key, text)}
      onStop={() => { stopSession(key); }}
      approvals={verdicts}
      activeApprovalIds={pastThread !== null ? NO_APPROVAL_IDS
        : activeApprovalsFor(key, events, tileLive, verdicts)}
      composer={pastThread === null ? undefined
        : <AgentPastThreadNotice hasCurrent={pastThread.currentKey !== null}
          onBack={() => { showAgentThread(pastThread.agentId); }} />}
      // An agent's approvals are Rust's policy's first. `answerApproval`
      // stays the only path that responds; this only withholds the
      // controls until the owner is the one being asked.
      approvalGate={agentApprovalGateFor(agentStore, key, agentStartKnownKeys)}
      onApprove={(approvalId, answers) => { approveFor(key, approvalId, answers); }}
      onDeny={(approvalId, note) => { denyFor(key, approvalId, note); }}
      // Both halves or no control: undefined until this run has
      // captured something for this session, and every turn then
      // looks exactly as it did before.
      turnRestorePoints={turnRestorePoints}
      onRestoreTurn={turnKey => { restoreTurnCheckpoint(key, turnKey); }}
      restoringTurnKey={restoringTurnKey}
      thread={thread}
      threadSend={threadSend}
      onThreadTurnDiff={thread === undefined ? undefined
        : ordinal => { openThreadTurnDiff(key, ordinal); }}
      onThreadEditFrom={thread === undefined ? undefined
        : (ordinal, restoreFiles, prompt) => editThreadFromTurn(key, ordinal, restoreFiles, prompt)}
      onThreadForkAt={thread === undefined ? undefined
        : keepTurns => { forkThreadAtTurn(key, keepTurns); }}
      onThreadShip={thread?.project !== true ? undefined
        : step => { shipThread(key, step); }}
      onThreadPreview={thread?.project !== true ? undefined
        : path => { previewThreadFile(key, path); }}
      onThreadAway={thread?.away === undefined ? undefined
        : action => { awayThread(key, action); }}
      onThreadTurnRows={thread === undefined ? undefined
        : rowKeys => { reportThreadTurnRows(key, rowKeys); }}
      onThreadRecover={thread === undefined ? undefined : text => recoverPrompt(key, "continue", undefined, text)}
      // The shell's new prompts use the ordinary lane; recovery has a separate,
      // receipt-bound path and never falls back to sending transcript text.
      onThreadRetry={thread === undefined ? undefined : text => {
        dismissRecap(key);
        return submitPrompt(key, text, []);
      }}
    />
  );
});
