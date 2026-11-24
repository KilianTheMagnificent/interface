import { PendingLaunchRecoveryPanel } from "./components/PromptRecoveryPanel";
import { visualFrameLoader } from "./visual-reply";
import type { RecoveryMode, RecoverySource } from "../shared/recovery";
import { sendWithVisibleTurn } from "./send-with-visible-turn";
import { useCollaborations } from "./use-collaborations";
import type { AttentionTarget as DurableAttentionTarget } from "../shared/harness";
import { TaskInspector } from "./components/TaskInspector";
import type { RemoteAdministrationRequest } from "../shared/remote";
import type { WorkspaceTool } from "./components/TerminalPane";
import {
  canvasSessionRows, fallbackEffortLevels, liveTurnProgress, nextCatalogueRevision,
  outputFeedbackTargetRows, perLog, reuseList, reuseRecord, sessionConfigFor,
  type CatalogueRevision, type OutputFeedbackTarget, type PendingMap, type PendingConfig,
} from "./app-session-projection";
const manageRemoteAccess = (request: RemoteAdministrationRequest) => window.iface.remoteAccess(request);
// Protocol 56: the Threads search's "In messages" matches, from Rust's thread journal.
const searchThreadMessages = (query: string) =>
  window.iface.threadJournal({ kind: "search", query }).then(journalHitsOf);

import { providerLabel } from "./provider-labels";
import {
  givenPermissionMode, modeInForce, modeToRestore, sessionHasWorkspaceContext,
} from "./session-presentation";
import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useReducer, useRef, useState } from "react";
import type { ComponentProps } from "react";
import { useCommittedActions, useCommittedCallback } from "./use-stable-value";
import {
  isSessionKind, sessionKey,
} from "../shared/engine";
import { ORCHESTRATOR_ROLE } from "../shared/orchestration";
import {
  HISTORY_LIMIT, HISTORY_MAX_LIMIT, isProviderListingFailure, parseKey,
} from "../shared/ipc";
import type {
  EngineEvent, EngineId, ModelOption, SessionKey, SessionKind, SessionStatus,
  SessionSummary,
} from "../shared/engine";
import type {
  EngineProbe,
  ProjectFileEntry, PromptContextValidationTarget, TerminalLaunchProfile,
  VoiceState,
} from "../shared/ipc";
import type { WorktreeNotice } from "../main/bridge";
import {
  BUILT_IN_ENGINES, DANGEROUS_MODES, ENGINE_CHOICES, UNKNOWN_ENGINE_CHOICES, engineChoices,
} from "./engine-choices";
import {
  isCliThemePreference, pairThemeWithChrome, type ThemeId,
} from "../shared/appearance";
import { updateButtonLabel } from "../shared/updater";
import {
  agentEngineRefusal, agentForSession, agentTaskCommand, newAgentEngine, newAgentId, nextAgentColor,
  type AgentProfile,
} from "./agent-profiles";
import {
  agentPermissionMode, permissionModeChange, type AgentPermissionMode, type AgentRequest,
} from "../shared/agents";
import { equalValue } from "./stable-values";
import {
  AgentWorkspacePanel, type AgentRequestResult,
} from "./components/AgentWorkspacePanel";
import { touchedFileStats } from "./agent-task";
import {
  agentBoundSession, agentLatestTasks, agentThreadOwner,
} from "./agent-roster";
import { useAppearance } from "./appearance";
import {
  AgentHeader, AgentMenu, AgentProfileDialog, type AgentEngineOption,
} from "./components/AgentMenu";
import { AgentTask } from "./components/AgentTask";
import { AgentTile } from "./components/AgentTile";
import { ConversationPaneSurface, PaneVisibleContext } from "./components/ConversationPaneSurface";
import {
  APP_SELECT_CSS, APP_SELECT_DISMISS_EVENT,
} from "./components/AppSelect";
import { ContextValueDialog } from "./components/ContextValueDialog";
import { ThreadReferenceDialog } from "./components/ComposerReferenceDialogs";
import { appendTerminalExcerpt } from "./composer-quote";
import { stashOtherItems, stashPathItems } from "./draft-stash-view";
import { threadAgeLabel } from "./thread-inbox";
import { threadReferenceCapability, threadReferenceChoices, threadReferenceItem } from "./thread-reference";
import { useDraftStash, type ThreadStashState } from "./use-draft-stash";
import { CommandPalette, type CommandPaletteCommand } from "./components/CommandPalette";
import {
  paletteCommandKeepsComposer, paletteThemes, shellPaletteCommands, THEME_COMMAND_PREFIX,
} from "./palette-commands";
import { ConfirmDialog } from "./components/ConfirmDialog";
import { CreateProjectDialog } from "./components/CreateProjectDialog";
import type { ProjectsFolderView } from "../shared/projects";
import {
  SessionLibrary,
} from "./components/SessionLibrary";
import { Composer } from "./components/Composer";
import {
  COMPOSER_TEAM_DEFAULTS, concreteEffortLevels,
  startingPermissionMode, type ComposerLaunchForm, type EngineChoices, type EngineOption,
  type ScopeOptions,
} from "./composer-model";
import type {
  HistoryView, SessionConfigView, TurnRestorePoint,
} from "./conversation-types";
import {
  createDraftStore, draftWriteNeedsPublish, isCollaborationSourceKey, nextDraftRevision,
} from "./draft-store";
import {
  RECAP_CSS, SessionTile, type SessionTileActions,
} from "./components/SessionTile";
import {
  CollaborationPanel, type CollaborationSessionOption,
} from "./components/CollaborationPanel";
import {
  AgentCanvas as AgentCanvasView, type AgentCanvasSession,
} from "./components/AgentCanvas";
import {
  FOREGROUND_DIALOG_SELECTOR, Sidebar as SidebarView,
  type SidebarImporter, type SidebarLifecycle,
} from "./components/Sidebar";
import type { SessionPeekFacts } from "./components/SessionPeek";
import { SettingsPage, type SettingsSectionId } from "./components/SettingsPage";
import { SubagentPanel } from "./components/SubagentPanel";
import {
  IntegratedTerminalCenter as IntegratedTerminalCenterView,
  type IntegratedTerminalCenterHandle,
} from "./components/IntegratedTerminalCenter";
import { WorkspaceMoreMenu } from "./components/WorkspaceMoreMenu";
import { SurfaceSwitch } from "./components/SurfaceSwitch";
import { ProductName } from "./components/ProductName";
import { InterfaceMark } from "./components/InterfaceMark";
import { productLabel } from "./product";
import { noticesOf, storeNoticeText, type StoreNotice } from "../shared/diagnostics";
import { parseMenuCommand } from "../shared/app-menu";
import { parseBrowserEvent } from "../shared/browser-events";
import type { TerminalMenuModel, TerminalWorkspaceSwitcher } from "./components/TerminalWorkspace";
import { statusWord } from "./components/status-words";
import { isClosePaneChord, shellOwnsChord } from "./terminal-keys";
import { holdTerminalsWhileSliding } from "./terminal-pane-motion";
import { presenceClass, RetainedPresence, usePresence } from "./presence";
import { useSidebarPeek } from "./sidebar-peek";
import { UsageMenu } from "./components/UsageMenu";
import { approvalVerdictKey } from "./approval-keys";
import {
  conversationItems, latestPlanEntries,
} from "./conversation-reducer";
import { echoPosition, echoSeen, launchPendingEcho, newPendingEcho } from "./transcript-merge";
import {
  releaseConversationProjection,
} from "./conversation-projection";
import { historyPageItems } from "./history-page-projection";
import type { ApprovalVerdict } from "./approval-keys";
import type { ConvItem } from "./conversation-reducer";
import type { PendingEcho } from "./transcript-merge";
import { type DiffStat, type SessionDiffFacts, sessionDiffFacts } from "./diff-stats";
import { buildLaunchOptions, DEFAULT_ENGINE } from "./launch";
import {
  latestModelPresetFor, modelPresetFor, readModelPresets, rememberModelPreset,
  writeModelPresets, type ModelPreset,
} from "./model-presets";
import {
  notificationPlan, postNotification, rememberNotified, shouldNotify, type NotificationKind,
} from "./notify";
import type { TerminalActivitySummary } from "./terminal-activity";
import { useTerminalNotifications } from "./terminal-notifications";
import {
  EMPTY_ATTENTION, attentionPlace, attentionRows, attentionUnreadCount, clearEvents, markOpened,
  recordEvent, trackNeeds, type AttentionRow, type AttentionState, type LiveNeed,
} from "./attention-tray";
import { AttentionTray } from "./components/AttentionTray";
import { CornerNotices, type CornerNoticeItem } from "./components/CornerNotices";
import {
  refreshErrorText, refreshFailureDetail, refreshFailureId, refreshFailureSentence,
  type RefreshFailure,
} from "./corner-notices";
import {
  PRIVACY_SUMMARY_BODY, PRIVACY_SUMMARY_DONE, PRIVACY_SUMMARY_ID, PRIVACY_SUMMARY_OPEN, PRIVACY_SUMMARY_TITLE,
  privacySummarySeen, showPrivacySummary,
} from "./privacy";
import { parseAgentQuestions, questionSummary } from "../shared/questions";
import {
  addPromptContextItem, promptTurnAttachments, promptTurnImages, removePromptContextItem,
  serializePromptWithContext,
  normalizePromptContextItems,
  promptContextCapabilities, type PromptContextItem, type PromptContextKind,
} from "./prompt-context";
import type { PromptDropPath } from "./prompt-drop";
import type { PastedFile } from "./prompt-paste";
import {
  attachmentContextItems, pastedImagePayloads, type GrantedAttachment,
} from "./prompt-attach";
import { repositoryReviewPromptContent } from "./review";
import { appReducer, initialState } from "./store";
import {
  applyAppearancePreferences, applySettingsChromeLock, initializeAppPreferences,
} from "./appearance";
import {
  notificationPreferencesOf, providerFaultSentence, signedOutSentence, signInProfile,
  writeAppPreferences, type AppPreferences,
} from "./settings";
import {
  parseSteeringQueueMap, removeSteeringPrompt, type SteeringQueueItem,
} from "./steering-queue";
import { deliveryOutcomeUnknown, type OutboundLanePort } from "./outbound-lane";
import { useOutboundLane } from "./use-outbound-lane";
import { useTaskInspector, type TaskInspectorHost } from "./use-task-inspector";
import { useThreads } from "./use-threads";
import { useThreadSchedules } from "./use-thread-schedules";
import { useThreadSecrets } from "./use-thread-secrets";
import { SideQuestionPanel } from "./components/SideQuestionPanel";
import { bringBackSide, sideAnswer, sideQuestionCommand, sideQuestionOffered } from "./side-question";
import { heldSecretRows } from "./thread-secrets";
import type { ThreadSecretCommand } from "../shared/thread-secrets";
import { watchOffered, watchTitle } from "./thread-schedules";
import { ThreadScheduleDialog } from "./components/ThreadScheduleDialog";
import { useThreadInbox } from "./use-thread-inbox";
import { startedThreadsByParent } from "./started-threads";
import { useThreadBridges } from "./use-thread-bridges";
import { useProviderAccounts } from "./use-provider-accounts";
import { accountSignInTitle, threadAccountLabel } from "./provider-accounts-view";
import { accountGroupFor } from "../shared/provider-accounts";
import { useOutputPreviewState } from "./use-output-preview-state";
import { useQuickOpen } from "./use-quick-open";
import { useTurnRestore } from "./use-turn-restore";
import { useAgentStore } from "./use-agent-store";
import { useSubagentDetail, workspaceTile } from "./use-subagent-detail";
import { useReturnRecap } from "./use-return-recap";
import { useAwayController } from "./use-away-controller";
import {
  threadFocusVisit, threadKeptReason, threadKeptWorktrees, threadListingRevealed,
  threadProjectRoot, threadScopeRoot, threadTerminalRefusal, type ThreadKeptWorktree,
} from "./thread-inbox";
import { useCanvasReads, useOrchestration } from "./use-orchestration";
import { ConfigAttempts, configToken } from "./session-config-tokens";
import { forgetTurnCheckpoints } from "./turn-checkpoints";
import {
  EMPTY_WORKSPACE_STATE, closeChordTarget, closeWorkspaceKey, cycleWorkspaceFocus,
  hasWorkspacePaneCapacity, mayOpenWorkspaceKeyWithReservation, openWorkspaceKey,
  parseWorkspaceState, replaceWorkspaceKey, switchWorkspaceKey,
  toggleWorkspaceMaximized, workspaceOverlayShortcut,
  type WorkspaceState,
} from "./workspace";
import {
  CONVERSATION_SURFACES, conversationSurfaceFor, layoutFor, reconcileConversationLayouts,
  type ConversationLayouts,
} from "./conversation-layouts";
import {
  CANVAS_LAYOUT_KEY, canvasProjectRoot, parseCanvasLayout, type CanvasLayoutState,
} from "./canvas-layout";
import {
  attentionAnnouncement, attentionSessions, pendingApproval, rememberTurnStart, sameThreadLiveFacts,
  sessionActivity, threadLiveFacts, type ThreadLiveFacts,
} from "./session-activity";
import {
  buildSidebarView, sessionSurfaceOf, sidebarEmptyState, type ConversationSurface,
} from "./sidebar-view";
import {
  configInForce, engineCan, engineOpsMap, withoutDefaultPermissionMode, type EngineOpMap,
} from "./session-config-view";
import {
  threadUsageHeadroom, usageHeadrooms, usageRailReading, type UsageEngineRef, type UsageHeadroom,
} from "./usage-view";
import { ProviderLogo } from "./components/ProviderLogo";
import { UpdateButton } from "./components/UpdateButton";
import { withoutKey } from "./helpers";
import { lastSegment } from "./display-paths";
import {
  HistoryPagingController, IDLE_HISTORY, LOADING_HISTORY, PendingEventBatch, frozenHistoryView, historyBeforeLive,
} from "./history-paging";
import { ipcMessage } from "./ipc-message";
import { useAcpProfiles } from "./use-acp-profiles";
import { usePlanUsage } from "./use-plan-usage";
import { useSessionImport } from "./use-session-import";
import { useSessionLibrary } from "./use-session-library";
import { useUpdater } from "./use-updater";
import { useVoiceAssistant } from "./use-voice-assistant";
import { VOICE_METER_BARS, voiceMeterBars } from "./voice-ui";
import { AwayControl } from "./components/AwayControl";
import { awayProject } from "./away-view";
import type { AwayCardAction } from "./away-thread";
import { workInFlight } from "./work-in-flight";
import { dockAttention } from "./dock-attention";
// Threads: the New thread card, a thread pane's head and turn controls, and
// its diff panel. Rust owns the state; `threads` carries it.
import { OutputPreviewPanel } from "./components/OutputPreviewPanel";
import { ThreadDiffPanel } from "./components/ThreadDiffPanel";
import { turnRestoreForOrdinal, normalizeThreadPrompt } from "./thread-turns";
import {
  ThreadDiffToggle, ThreadPaneHead, type ThreadProviderBridges,
} from "./components/ThreadPaneHead";
// Second opinion (review B4), Switch to (protocol 59) and the limit offer (review E).
import {
  NO_THREAD_OFFERS, latestRateLimitNotice, threadBridgeWait, threadLimitNotices, threadLimitOffer,
  threadLimitReset, resumeQueuedPrompt, threadProviderOffers, threadSwitchedFrom, type ThreadLimitOffer, type ThreadLimitReset,
} from "./thread-opinion";
import type { ThreadSettings, WorkspaceKept } from "../shared/threads";
// Folder trust (protocol 72): the question and an untrusted thread's line.
import {
  FOLDER_TRUST_CANCEL, FOLDER_TRUST_CONFIRM, FOLDER_TRUST_TITLE, folderTrustBody, folderTrustCommand,
  folderTrustRow, openThreadAsksFolderTrust, type FolderTrustChoice, type FolderTrustRow,
} from "./folder-trust";
import { appendDiffQuote, defaultThreadDiffChoice, type ThreadDiffQuote } from "./thread-diff";
import { addBrowserNoteToDraft } from "./browser-point";
import {
  carryKeyed, switchSeed, conversationThread, editThreadFrom, forkThreadAt, isProjectThread, restoreThreadFiles,
  threadChangeTotals, THREAD_PANE_BUSY, threadFilesRestorePoint,
  threadDiffSignal, ThreadPaneLock, type ThreadPanePort, withShip,
} from "./thread-pane";
import { appendReviewComments, shipCardView, writeWithAiBlocked, type ShipNext } from "./thread-ship";
import { useThreadShip, type ThreadShipPort } from "./use-thread-ship";
import { useThreadCard } from "./use-thread-card";
import { journalHitsOf } from "../shared/thread-journal";
import {
  focusThreadDiffToggle, threadDiffToggleAction, useThreadDiff, useThreadIndex,
} from "./use-thread-pane";
import "./theme.css";

const Sidebar = memo(SidebarView);
const AgentCanvas = memo(AgentCanvasView);
const IntegratedTerminalCenter = memo(IntegratedTerminalCenterView);

// App is the only place in the renderer that touches `window.iface`. Every
// component below is presentational: state comes down as props, effects leave
// through callbacks. That keeps the IPC surface auditable in one file. Logic
// worth testing lives in plain modules beside it (outbound-lane.ts,
// history-paging.ts, app-session-projection.ts, ...), which their tests import
// without loading App.

// Workspace keys App owns directly. Component-owned state remains beside its
// component, while versioned app preferences and per-model launch presets keep
// their keys and corruption handling in settings.ts and model-presets.ts.
const LAST_ENGINE = "iface.lastEngine";
// Shared with IntegratedTerminalCenter: last folder a Code session or terminal
// workspace actually launched in. Presentation preference, not a host grant.
const LAST_CWD = "iface.lastCwd";
const LAST_NOTIFIED = "iface.lastNotified";
// "0" shows every session; anything else (including a missing or corrupt value)
// hides the short ones. Default-on is deliberate: the sidebar should open
// decluttered, and the filter row says so in the one place the user is looking.
const HIDE_TRIVIAL = "interface.sidebar.hideTrivial";
// The other half of the same control: "0" lists every session the engines know
// about, anything else lists only the chats started in Interface. Default-on
// for the same reason the Claude Code desktop app lists its own chats rather
// than every terminal session on the machine — and it is a VIEW filter only.
// Nothing here writes, clears or reads an engine-side tag.
const APP_ONLY = "interface.sidebar.appOnly";
const WORKSPACE_STATE = "interface.workspace.v2";
const SURFACE_KEY = "interface.surface.v1";
// The rail toggle's last choice on a desktop-width window; see sidebarOpen.
const SIDEBAR_KEY = "interface.sidebar.v1";
// Chat and Agent keep independent tile layouts. Code holds terminals only:
// its old slot (the bare v2 key) and the v1 layout that slot migrated from
// are swept on load.
function workspaceStorageKey(kind: ConversationSurface): string {
  return `${WORKSPACE_STATE}.${kind}`;
}
const RETIRED_WORKSPACE_STATES = [WORKSPACE_STATE, "interface.workspace.v1"];
const STEERING_QUEUES = "interface.steeringQueues.v1";
const COMPOSER_CONTEXT_TARGET = "@composer";
// Shared empty array: `state.events[key] ?? []` would mint a new identity on
// every render and defeat the useMemo below it.
const NO_EVENTS: EngineEvent[] = [];
const MAX_REMEMBERED_SESSION_ROOTS = 200;
// What a refresh re-reads. "focus" (returning to the window) skips the
// provider probe, which launches every provider's CLI; "full" (mount and
// session-library changes) probes; "explicit" (the Refresh actions) probes and
// lists once more after the probe answers. A running refresh answers a later
// one of the same or a lower rank.
type RefreshKind = "focus" | "full" | "explicit";
const REFRESH_RANK: Record<RefreshKind, number> = { focus: 0, full: 1, explicit: 2 };
const NO_CONVERSATION_ITEMS: readonly ConvItem[] = [];
const NO_MODELS: ModelOption[] = [];
const NO_PINNED_PROJECTS: readonly string[] = Object.freeze([]);
// Agent draws its selected task itself, so its ordinary tile grid stays empty.
const NO_TILE_KEYS: readonly SessionKey[] = [];
const NO_PENDING_ECHOES: readonly PendingEcho[] = [];
const NO_PALETTE_COMMANDS: readonly CommandPaletteCommand[] = [];
const NO_QUEUE: readonly SteeringQueueItem[] = [];
const NO_CONTEXT_ITEMS: readonly PromptContextItem[] = [];
const NO_SIDE_EVENTS: readonly EngineEvent[] = [];
const NO_STASH: ThreadStashState = { entries: [], busy: false };

const SURFACE_ZERO_TEXT: Readonly<Record<SessionKind, string>> = {
  chat: "No threads yet.",
  code: "No workspaces yet.",
  agent: "No tasks yet.",
};

const SURFACE_INTRO: Readonly<Record<SessionKind, { title: string; description: string }>> = {
  chat: { title: "Start a thread", description: "Ask a question, or pick a project and give an agent work." },
  code: { title: "Build in your workspace", description: "Open a folder and work alongside your coding agents." },
  agent: { title: "Give your agent a task", description: "Choose a folder, describe the result, and follow its progress." },
};

const AGENT_FIRST_DESCRIPTION = "Name it, say what it does, and pick its icon. Then give it tasks.";

const SURFACE_NEW_LABEL: Readonly<Record<SessionKind, string>> = {
  chat: "New thread",
  code: "New workspace",
  agent: "New task",
};

/**
 * What the sidebar is listing, per surface. One column with three jobs, and a
 * header that said "Workspaces" over a list of chats on two of them.
 */
const SURFACE_SECTION_LABEL: Readonly<Record<SessionKind, string>> = {
  chat: "Threads",
  code: "Workspaces",
  agent: "Tasks",
};

/** The one word the statusbar shows for each voice state, in the user's terms. */
const VOICE_STATE_TEXT: Readonly<Record<VoiceState, string>> = {
  off: "Voice off",
  starting: "Starting voice",
  listening: "Listening",
  thinking: "Thinking",
  speaking: "Speaking",
  error: "Voice error",
};

/** The composer's loading copy names the button that opened it. */
const preparingLabel = (kind: SessionKind): string =>
  `Preparing ${SURFACE_NEW_LABEL[kind].toLowerCase()}`;

type ApprovalEvent = Extract<EngineEvent, { kind: "approval.requested" }>;

/* ---- pending model/effort changes ---------------------------------------- */

// A change the user asked for and the engine has not answered yet. Per session,
// per field: the model pill can be waiting while the effort pill is settled.
// A field's PRESENCE is the flag, so `{ model: undefined }` is a real pending
// value — "back to the engine default" — and not an absence.

function withPending(map: PendingMap, key: SessionKey, patch: PendingConfig): PendingMap {
  return { ...map, [key]: { ...map[key], ...patch } };
}

// Revert of one field, leaving any other in-flight field alone.
function withoutPending(map: PendingMap, key: SessionKey,
    field: keyof PendingConfig): PendingMap {
  const current = map[key];
  if (current === undefined || !(field in current)) return map;
  const next: PendingConfig = { ...current };
  delete next[field];
  const out: PendingMap = { ...map };
  if (Object.keys(next).length === 0) delete out[key];
  else out[key] = next;
  return out;
}

/* ---- localStorage (never throws; a blocked store just means no memory) ---- */

function readStored(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

/** One tile layout per conversation surface (see conversation-layouts.ts). */
function loadInitialWorkspaces(): ConversationLayouts {
  for (const key of RETIRED_WORKSPACE_STATES) {
    try { localStorage.removeItem(key); } catch { /* storage refused: nothing reads it */ }
  }
  return {
    chat: parseWorkspaceState(readStored(workspaceStorageKey("chat"))),
    agent: parseWorkspaceState(readStored(workspaceStorageKey("agent"))),
  };
}

/**
 * Chat and Agent open on their launch sheet only when their own layout is
 * empty; Code opens on its terminals in either appearance, never covered by a
 * chat launch sheet.
 */
function initialComposerOpen(): boolean {
  const stored = readStored(SURFACE_KEY);
  return (stored === "chat" || stored === "agent")
    && parseWorkspaceState(readStored(workspaceStorageKey(stored))).openKeys.length === 0;
}

function writeStored(key: string, value: string): boolean {
  try {
    localStorage.setItem(key, value);
    return true;
  } catch {
    return false;
  }
}

function rendererItemId(prefix: string): string {
  return `${prefix}:${crypto.randomUUID()}`;
}

const pathLeaf = (path: string): string => lastSegment(path) || path;

// Tolerant read: anything that is not a string map is treated as absent rather
// than crashing the shell on a hand-edited or half-written value.
function loadNotified(): Record<SessionKey, string> {
  const raw = readStored(LAST_NOTIFIED);
  if (raw === null) return {};
  try {
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return {};
    const out: Record<SessionKey, string> = {};
    for (const [k, v] of Object.entries(parsed)) if (typeof v === "string") out[k] = v;
    return out;
  } catch {
    return {};
  }
}

// Latest rate-limit notice per session, keyed for the sidebar badge
// (thread-opinion.ts, `latestRateLimitNotice`).
const rateLimitNotice = latestRateLimitNotice;

/** No Threads row reads a live line off this surface. */
const NO_THREAD_LIVE: Readonly<Record<SessionKey, ThreadLiveFacts>> = {};

function sessionPeekFacts(log: readonly EngineEvent[], files: number): SessionPeekFacts | null {
  const activity = sessionActivity(log);
  const ask = pendingApproval(log);
  if (activity === "" && files === 0 && ask === undefined) return null;
  return {
    ...(activity === "" ? {} : { activity }),
    ...(files === 0 ? {} : { files }),
    ...(ask === undefined ? {} : { approval: ask.summary }),
  };
}

/* ---- workspace destinations --------------------------------------------- */

/**
 * What the flexible workspace is showing. The toolbar switches it.
 *
 *  - "sessions" up to four live conversations side by side. The home view.
 *  - "canvas" the orchestrator/worker graph for one project root.
 *  - "terminal" persistent integrated shells, tabs, and split panes.
 *
 * Plan usage lives in the always-visible toolbar menu, not here: it is a
 * thing you check, not a place you work.
 */
type CenterView = "sessions" | "canvas" | "terminal";

const MOBILE_SIDEBAR_QUERY = "(max-width: 840px)";

/** The transcript's find bar. Not a dialog, but it does own Escape.
 *
 *  Its `find.open` state lives per-tile inside Conversation, so App cannot read
 *  it — but the bar only renders while open, which makes its presence the
 *  state. App must DECLINE the key here rather than preventDefault it: both
 *  listeners sit on window, `stopPropagation` does not cancel a sibling on the
 *  same target, and Conversation's handler is the one guarded on
 *  `defaultPrevented`. Bailing is therefore the only outcome that is correct in
 *  both registration orders — Escape closes find, a second Escape closes the
 *  panel, innermost surface first. */
const TRANSCRIPT_FIND_SELECTOR = ".conv-findbar";


/** The session is working, or waiting on the owner. */
function sessionBusy(summary: SessionSummary | undefined): boolean {
  return summary?.status === "working" || summary?.status === "waiting";
}


/** The outbound lane's port members that are App callbacks, bound once they exist. */
type OutboundAppHooks = Pick<OutboundLanePort, "validateContext" | "restoreAccessMode"
  | "publishDraft" | "checkpoint" | "markDelivered" | "restoreContext">;

/** A late-bound ref's value, once App has bound it. */
function bound<T>(ref: { readonly current: T | null }): T {
  if (ref.current === null) throw new Error("App used a callback before binding it.");
  return ref.current;
}

/* ---- app ----------------------------------------------------------------- */

export function App() {
  const [state, dispatch] = useReducer(appReducer, initialState, current => ({
    ...current,
    composerOpen: initialComposerOpen(),
  }));
  const {
    appearance, pending: appearancePending, error: appearanceError,
    setPreference: setAppearancePreference,
    setSystemThemes,
    setTranslucency,
    setWorkspacePresentation,
    setProjectPresentation,
  } = useAppearance(window.iface);
  const appearanceRef = useRef(appearance);
  appearanceRef.current = appearance;
  // Stable identities keep memo(Sidebar) effective; both read the latest
  // presentation through the ref rather than the render's snapshot.
  const pinProject = useCallback((path: string, pinned: boolean): void => {
    const current = appearanceRef.current.projectPresentation;
    if (current === undefined) return;
    const paths = current.pinnedProjects.filter(value => value !== path);
    if (pinned) paths.push(path);
    setProjectPresentation({ ...current, pinnedProjects: paths });
  }, [setProjectPresentation]);
  const pinWorkspace = useCallback((id: string, pinned: boolean): void => {
    const current = appearanceRef.current.workspacePresentation;
    if (current === undefined) return;
    const ids = current.pinnedWorkspaces.filter(value => value !== id);
    if (pinned) ids.push(id);
    setWorkspacePresentation({ ...current, pinnedWorkspaces: ids });
  }, [setWorkspacePresentation]);
  const [appPreferences, setAppPreferences] = useState<AppPreferences>(() =>
    initializeAppPreferences(localStorage));
  const [preferencesError, setPreferencesError] = useState<string | null>(null);
  const [modelPresets, setModelPresets] = useState<ModelPreset[]>(() =>
    readModelPresets(localStorage));
  // `startAgentThread` requests in flight, and the sessions that existed when
  // the first began: until the answer names the new thread, an approval on a
  // session outside that set may be the thread's, so it waits for the policy.
  const agentStartsPending = useRef(0);
  const [agentStartKnownKeys, setAgentStartKnownKeys] = useState<ReadonlySet<SessionKey> | null>(null);
  // The agent page reads a thread only while the owner can see it.
  const [documentVisible, setDocumentVisible] = useState(() => document.visibilityState === "visible");
  useEffect(() => {
    const update = (): void => { setDocumentVisible(document.visibilityState === "visible"); };
    document.addEventListener("visibilitychange", update);
    return () => { document.removeEventListener("visibilitychange", update); };
  }, []);
  const [agentDialog, setAgentDialog] = useState<
    { mode: "new"; id: string } | { mode: "edit"; id: string } | null>(null);
  const [agentDeleteId, setAgentDeleteId] = useState<string | null>(null);
  // "Trust this folder?" (protocol 72): the folder it asks about, and how
  // the one waiting for the answer hears it.
  const [folderTrustQuestion, setFolderTrustQuestion] = useState<string | null>(null);
  const folderTrustAnswer = useRef<((choice: FolderTrustChoice) => void) | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  /** The section Settings opens on; undefined is its own default. */
  const [settingsSection, setSettingsSection] = useState<SettingsSectionId | undefined>(undefined);
  const settingsOpenerRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    applySettingsChromeLock(settingsOpen);
  }, [settingsOpen]);

  // A stored Bare CLI preference plus a leftover workspace palette (Hermes,
  // System, paper) is the pre-pairing world. First successful appearance
  // read is the one chance to apply the remembered console theme without
  // fighting a later in-session pick.
  const pairedCliThemeOnLoad = useRef(false);
  useEffect(() => {
    if (appearancePending || pairedCliThemeOnLoad.current) return;
    pairedCliThemeOnLoad.current = true;
    if (appPreferences.uiChrome !== "cli") return;
    if (isCliThemePreference(appearance.preference)) return;
    setAppearancePreference(appPreferences.lastCliTheme);
  }, [appearancePending, appearance.preference, appPreferences.lastCliTheme,
    appPreferences.uiChrome, setAppearancePreference]);
  const {
    updateState, updateRestartConfirmOpen, updateRestartPending, updateRestartOpenerRef,
    checkForUpdates, requestRestartToUpdate, cancelRestartToUpdate, confirmRestartToUpdate,
  } = useUpdater(window.iface);
  const {
    acpProfiles, acpProfilesPending, acpProfilesError, acpProfilesRestartRequired,
    acpProfilesSavedRevision, saveAcpProfiles,
  } = useAcpProfiles(window.iface);

  // Model presets are renderer preferences, not provider session state. Keep
  // them usable in memory even when privacy/quota settings block localStorage.
  // Both effects skip their first run: the state they persist was READ from
  // localStorage a moment ago, so writing it straight back is a synchronous
  // serialize plus a storage write that changes nothing.
  const modelPresetsPersisted = useRef(false);
  useEffect(() => {
    if (!modelPresetsPersisted.current) {
      modelPresetsPersisted.current = true;
      return;
    }
    writeModelPresets(localStorage, modelPresets);
  }, [modelPresets]);

  // The Rust agent store: reads, mutations and the 5 s visible poll (use-agent-store.ts).
  const {
    agentStore, agentStateRef, agentReload, agentError, setAgentError, agentReady, agentPending,
    mutateAgent, requestAgent,
  } = useAgentStore(window.iface);

  // Keys whose engine handle is attached in this app run. Any event for a key
  // proves it was live when the bridge broadcast it; a key that is absent gets
  // the Resume button instead of a composer strip.
  const liveKeys = useRef<Set<SessionKey>>(new Set());
  const closedKeys = useRef<Set<SessionKey>>(new Set());
  const turnStartedAtByKey = useRef<Record<SessionKey, number>>({});
  const pendingEventBatch = useRef(new PendingEventBatch());
  const pendingEventFrame = useRef<number | null>(null);
  /** The hidden-window substitute for the frame above; never both at once. */
  const pendingEventTimer = useRef<number | null>(null);
  // The latest registry push, applied by the same flush right after the
  // batched events, so host order (events, then the registry they caused)
  // survives and the pair costs one render.
  const pendingRegistry = useRef<SessionSummary[] | null>(null);
  // setModel's IPC response and its command-catalogue events travel on
  // independent Electron channels. Track their order so an accepted model
  // switch can never briefly re-enable the previous model's commands while
  // the provider's invalidation broadcast is still in flight.
  const modelCommandRefresh = useRef<Record<string, {
    attempt: number;
    phase: "pending" | "invalidated" | "available";
    acknowledged: boolean;
  }>>({});
  // A ref cannot trigger a render, so a counter bump publishes membership changes.
  const [liveVersion, setLiveVersion] = useState(0);
  // Bumped only by a batch carrying a live user turn; see the re-key effect.
  const [settledTurnWake, setSettledTurnWake] = useState(0);

  /** Apply the coalesced batch now, cancelling whatever was scheduled to do it. */
  const flushPendingEvents = useCallback((): void => {
    if (pendingEventFrame.current !== null) {
      window.cancelAnimationFrame(pendingEventFrame.current);
      pendingEventFrame.current = null;
    }
    if (pendingEventTimer.current !== null) {
      window.clearTimeout(pendingEventTimer.current);
      pendingEventTimer.current = null;
    }
    const items = pendingEventBatch.current.flush();
    if (items.length > 0) dispatch({ type: "events", items });
    const registry = pendingRegistry.current;
    if (registry !== null) {
      pendingRegistry.current = null;
      dispatch({ type: "registry", sessions: registry });
    }
    // A user turn arriving live is what settles this window's echo of it —
    // see the re-key effect. Bumped HERE so the wake lands in the same
    // commit as the events it is about, and only for a batch that carries
    // one: every ACP engine streams the prompt back, no other engine does,
    // and the walk it triggers is not free.
    if (items.some(item => item.evt.kind === "user.message")) {
      setSettledTurnWake(value => value + 1);
    }
  }, []);

  /** Schedule one flush of the event batch and pending registry, if none is. */
  const armEventFlush = useCallback((): void => {
    if (pendingEventFrame.current !== null || pendingEventTimer.current !== null) return;
    // A hidden window's animation frames are throttled to a crawl or stopped
    // outright, so a backgrounded session's transcript would sit in this batch
    // until the user came back. A zero timer coalesces exactly the same way
    // without depending on the compositor; the generation fence in
    // PendingEventBatch.flush is what keeps a dropped session out either way.
    if (document.visibilityState === "hidden") {
      pendingEventTimer.current = window.setTimeout(flushPendingEvents, 0);
      return;
    }
    pendingEventFrame.current = window.requestAnimationFrame(flushPendingEvents);
  }, [flushPendingEvents]);

  const enqueueEvent = useCallback((key: SessionKey, evt: EngineEvent): void => {
    // Sparse replacement-control events should not wait behind the animation-
    // frame transcript batch. In particular, the model setter's IPC response
    // can arrive before that frame; applying these now closes the stale-command
    // window while high-frequency text/status events remain batched below.
    if (evt.kind === "commands.available" || evt.kind === "commands.invalidated") {
      dispatch({ type: "event", key, evt });
      return;
    }
    pendingEventBatch.current.enqueue(key, evt);
    armEventFlush();
  }, [armEventFlush]);

  const lastNotified = useRef<Record<SessionKey, string>>({});
  const lastNotifiedStatus = useRef<Record<SessionKey, SessionStatus>>({});
  const focusedKeyRef = useRef<SessionKey | null>(null);
  const sessionsRef = useRef(state.sessions);
  sessionsRef.current = state.sessions;
  const eventsRef = useRef(state.events);
  eventsRef.current = state.events;
  // The rail's sidebar toggle. Remembered since the hidden sidebar answers the
  // window's left edge (owner, 2026-10-20): a panel one pointer move away no
  // longer reads as a broken window after a reload.
  const [sidebarOpen, setSidebarOpen] = useState(() => readStored(SIDEBAR_KEY) !== "hidden");
  const sidebarToggleRef = useRef<HTMLButtonElement>(null);
  const appGridRef = useRef<HTMLDivElement>(null);
  // Layout phase, so the slide's first resize already sees the hold.
  useLayoutEffect(() => {
    const grid = appGridRef.current;
    return grid === null ? undefined : holdTerminalsWhileSliding(grid) ?? undefined;
  }, [sidebarOpen]);
  // The terminal surface's own controls (its menu, Flip, Tidy) ride the window
  // toolbar rather than a second bar of their own; the surface portals them
  // into this element, which only exists while the terminals are showing.
  const [terminalToolbarSlot, setTerminalToolbarSlot] = useState<HTMLElement | null>(null);
  const commandPaletteButtonRef = useRef<HTMLButtonElement>(null);
  // The terminal view's layout section of the ··· menu, published by the view.
  const [terminalMenu, setTerminalMenu] = useState<TerminalMenuModel | null>(null);
  const composerOpenerRef = useRef<HTMLElement | null>(null);
  const [mobileLayout, setMobileLayout] = useState(() =>
    window.matchMedia(MOBILE_SIDEBAR_QUERY).matches);

  useEffect(() => {
    const media = window.matchMedia(MOBILE_SIDEBAR_QUERY);
    const sync = (): void => { setMobileLayout(media.matches); };
    media.addEventListener("change", sync);
    return () => { media.removeEventListener("change", sync); };
  }, []);

  // Desktop keeps its persistent column. Only the narrow overlay dismisses
  // itself, and focus returns to the control that can open it again so closing
  // via Escape/backdrop never strands keyboard focus in display:none content.
  const closeMobileSidebar = useCallback((): void => {
    if (!sidebarOpen || !mobileLayout) return;
    setSidebarOpen(false);
    window.requestAnimationFrame(() => { sidebarToggleRef.current?.focus(); });
  }, [mobileLayout, sidebarOpen]);
  const toggleSidebar = useCallback((): void => {
    const next = !sidebarOpen;
    setSidebarOpen(next);
    // The narrow drawer closes after every pick; that is not a choice to keep.
    if (!mobileLayout) writeStored(SIDEBAR_KEY, next ? "shown" : "hidden");
  }, [mobileLayout, sidebarOpen]);
  const [centerView, setCenterView] = useState<CenterView>(() => {
    const stored = readStored(SURFACE_KEY);
    return stored === "chat" || stored === "agent" ? "sessions" : "terminal";
  });
  // Which of Chat, Code, and Agent is showing. Unlike a Code-internal
  // destination, this is a product preference and survives a reload.
  const [surface, setSurfaceState] = useState<SessionKind>(() => {
    const stored = readStored(SURFACE_KEY);
    return isSessionKind(stored) ? stored : "code";
  });
  const surfaceRef = useRef(surface);
  surfaceRef.current = surface;
  // Workspace selection owns the Code destination; visiting Chat or Agent
  // returns to that workspace without a global presentation preference.
  const codeDestinationRef = useRef<CenterView>("terminal");
  if (surface === "code" && centerView !== "sessions") codeDestinationRef.current = centerView;
  // Which agent the Agent surface is pointed at. Not persisted, for the same
  // reason `centerView` is not: it is where you are, not a preference.
  const [selectedAgentId, setSelectedAgentId] = useState<string | null>(null);
  // Notification and fork continuations are defined before showSurface. They
  // call through this ref so they still use the current registry listing.
  const ensureSurfaceForRef = useRef<(key: SessionKey) => void>(() => undefined);
  // The Rust terminal host remains the sole owner of terminal lifecycle. These
  // are renderer projections used only to place its summaries in the same
  // project navigation as chats and route a click back to one exact terminal.
  const [terminalSummaries, setTerminalSummaries] =
    useState<readonly TerminalActivitySummary[]>([]);
  const [focusedTerminalId, setFocusedTerminalId] = useState<string | null>(null);
  // The terminal center publishes its saved-workspace switcher; the sidebar
  // renders it. Workspaces are a destination in the left navigation, not a tab
  // bar above the panes.
  const [terminalSwitcher, setTerminalSwitcher] =
    useState<TerminalWorkspaceSwitcher | null>(null);
  // Adversarial review has one entry point inside the terminal launcher and one
  // in the palette; the dialog itself belongs to the terminal center, which owns
  // the launch path.
  const terminalCenter = useRef<IntegratedTerminalCenterHandle>(null);
  // The attention tray's memory: when each live need began, and the finished
  // and failed turns recorded at the edge that chimes. The live needs
  // themselves are derived below from the same listing the badge counts.
  const [attention, setAttention] = useState<AttentionState>(EMPTY_ATTENTION);
  const [attentionOpen, setAttentionOpen] = useState(false);
  // Session listings intentionally omit archived chats. Remember the project
  // identity of every session seen this app run so a live terminal does not
  // jump from its repository to a disposable worktree/root after its chat is
  // hidden. This map is renderer-only and never grants directory authority.
  const rememberedSessionProjectRoots = useRef(new Map<SessionKey, string | null>());
  const rememberedSourceRef = useRef<typeof state.sessions | undefined>(undefined);
  if (rememberedSourceRef.current !== state.sessions) {
    rememberedSourceRef.current = state.sessions;
    const listedKeys = new Set<SessionKey>();
    for (const summary of state.sessions) {
      const key = sessionKey(summary.engine, summary.sessionId);
      listedKeys.add(key);
      rememberedSessionProjectRoots.current.set(
        key,
        summary.projectRoot === undefined || summary.projectRoot === ""
          ? null
          : summary.projectRoot,
      );
    }
    const remembered = rememberedSessionProjectRoots.current;
    if (remembered.size > MAX_REMEMBERED_SESSION_ROOTS) {
      for (const key of remembered.keys()) {
        if (remembered.size <= MAX_REMEMBERED_SESSION_ROOTS) break;
        if (!listedKeys.has(key)) remembered.delete(key);
      }
    }
  }
  const logCachesRef = useRef({
    rateLimit: new WeakMap<object, { value: string | undefined }>(),
    // The stat and the touched-path count share one pass over each log.
    diffFacts: new WeakMap<object, { value: SessionDiffFacts }>(),
    peek: new WeakMap<object, { value: SessionPeekFacts | null }>(),
    threadLive: new WeakMap<object, { value: ThreadLiveFacts | null }>(),
    touched: new WeakMap<object, { value: Record<string, DiffStat> }>(),
  });
  const previousByEventsRef = useRef<{
    rateLimits?: Record<SessionKey, string>;
    diffStats?: Record<SessionKey, DiffStat>;
    peeks?: Record<SessionKey, SessionPeekFacts>;
    threadLive?: Readonly<Record<SessionKey, ThreadLiveFacts>>;
    collaborationSessions?: readonly CollaborationSessionOption[];
    canvasSessions?: readonly AgentCanvasSession[];
    canvasProjectRoots?: readonly string[];
    attentionListing?: readonly SessionSummary[];
    outputFeedbackTargets?: readonly OutputFeedbackTarget[];
  }>({});
  const acceptTerminalSummaries = useCallback((terminals: readonly TerminalActivitySummary[]): void => {
    setTerminalSummaries(terminals);
  }, []);
  const acceptFocusedTerminal = useCallback((terminalId: string | null): void => {
    setFocusedTerminalId(terminalId);
  }, []);
  // Node positions are presentation, so they live in localStorage beside the
  // workspace layout and never in the durable host store.
  const [canvasLayout, setCanvasLayout] = useState<CanvasLayoutState>(
    () => parseCanvasLayout(readStored(CANVAS_LAYOUT_KEY)));
  const [canvasRoot, setCanvasRoot] = useState("");
  const changeCanvasLayout = useCallback((next: CanvasLayoutState): void => {
    setCanvasLayout(next);
    writeStored(CANVAS_LAYOUT_KEY, JSON.stringify(next));
  }, []);
  // Cursor's Agents Window keeps the conversations a user arranged instead of
  // collapsing back to one selected chat. Only identities/layout survive a
  // reload, and Chat and Agent each own a slot. Code holds terminals only.
  const [workspaces, setWorkspaces] = useState(loadInitialWorkspaces);
  const workspacesRef = useRef(workspaces);
  workspacesRef.current = workspaces;
  const workspace = layoutFor(workspaces, surface);
  const setWorkspaceFor = useCallback((kind: ConversationSurface, update: WorkspaceState
      | ((current: WorkspaceState) => WorkspaceState)) => {
    setWorkspaces(all => {
      const next = typeof update === "function" ? update(all[kind]) : update;
      return next === all[kind] ? all : { ...all, [kind]: next };
    });
  }, []);
  // Keep the established call-site shape while routing every mutation to the
  // surface that is active at handler time. showSurface updates surfaceRef
  // before React state, so a switch-and-open sequence lands in the new slot.
  // Every session reveal moves to the session's own surface first
  // (ensureSurfaceFor), so Code, which has no slot, never takes a write.
  const setWorkspace = useCallback((update: WorkspaceState
      | ((current: WorkspaceState) => WorkspaceState)) => {
    const kind = surfaceRef.current;
    if (kind !== "code") setWorkspaceFor(kind, update);
  }, [setWorkspaceFor]);
  focusedKeyRef.current = workspace.focusedKey;
  // Agent keeps its dedicated task view and never mounts the tile grid, and
  // Code's layout is always empty. Cover the conversations when showing
  // another surface; keep their DOM, draft, disclosure and reader position
  // through a presentation switch.
  const gridKeys = surface !== "agent"
    ? workspace.openKeys : NO_TILE_KEYS;
  // A two-pane orchestration launch reserves both slots before provider side
  // effects. The builder and reviewer are ordinary (non-agent) sessions, so
  // they open in Chat and this reservation is Chat's; Agent never consumes it.
  const workspacePaneReservation = useRef(0);
  // Explicit session navigation always reveals the session workspace of the
  // Chat or Agent surface that owns the session (ensureSurfaceFor moves there
  // first). Entering Code through the surface switch goes to its terminals
  // through showSurface; this helper is only used after a concrete session key
  // has been selected, opened, launched, or focused.
  const showSessionWorkspace = useCallback(() => { setCenterView("sessions"); }, []);
  const [registryReady, setRegistryReady] = useState(false);
  // Set once the first listSessionSnapshot has answered (either way). The host
  // paints its cached registry just before that answer; until then a registry
  // push only fills rows and never prunes saved layouts.
  const firstListingAnswered = useRef(false);
  const pendingTileFocus = useRef<SessionKey | null>(null);
  // A split asked for ANOTHER pane, so the launch it opens must append rather
  // than replace the focused one. Set by splitWorkspaceTile, consumed exactly
  // once by launch's success arm — and cleared there whether or not it was set,
  // so an abandoned sheet cannot make the next ordinary New Chat open a pane.
  const pendingNewPane = useRef(false);
  const [commandPaletteOpen, setCommandPaletteOpen] = useState(false);
  const [commandQuery, setCommandQuery] = useState("");
  // The file picker's store (use-quick-open.ts): `quickOpen.open` means files mode.
  const { quickOpen, closeQuickOpen, openContextQuickOpen } = useQuickOpen(window.iface, {
    setCommandPaletteOpen, setCommandQuery,
  });
  // The orchestration snapshot and the canvas's intents (orchestration-controller.ts).
  const { snapshot: orchestrationSnapshot, reads: orchestrationReads, error: orchestrationError,
    controller: orchestration } = useOrchestration(window.iface);
  const collaborationApi = useMemo(() => ({
    listCollaborations: window.iface.listCollaborations,
    onCollaborationEvent: window.iface.onCollaborationEvent,
    submitCollaboration: window.iface.submitCollaboration,
    approveCollaboration: window.iface.approveCollaboration,
    denyCollaboration: window.iface.denyCollaboration,
    acceptCollaborationPeer: window.iface.acceptCollaborationPeer,
    discardCollaborationPeer: window.iface.discardCollaborationPeer,
    cancelCollaboration: window.iface.cancelCollaboration,
  }), []);
  const { collaborations, collaborationsLoading, collaborationsError, loadCollaborations,
    noteCollaboration,
    submitCollaboration, approveCollaboration, denyCollaboration, acceptCollaborationPeer,
    discardCollaborationPeer, cancelCollaboration } = useCollaborations(collaborationApi, ipcMessage);
  const [composerContext, setComposerContext] = useState<PromptContextItem[]>([]);
  const [composerContextOpen, setComposerContextOpen] = useState(false);
  const [composerContextError, setComposerContextError] = useState<string | null>(null);
  // A project-level New Chat must override the remembered repository without
  // writing that preference before a launch succeeds. The revision remounts an
  // already-open composer so an explicit New Chat click is always a fresh form.
  // `prompt` is the starter-prompt seed: text placed in the composer's field for
  // the user to edit, choose a repository for, and send — never sent from here.
  const [composerSeed, setComposerSeed] = useState<
    { revision: number; cwd: string | null; prompt: string; worktree?: boolean }>({
      revision: 0, cwd: null, prompt: "", worktree: false,
    });
  const [composerDirectory, setComposerDirectory] = useState(() => {
    const remembered = readStored(LAST_CWD);
    return { path: remembered ?? "", ready: remembered !== null, error: null as string | null };
  });
  // LAST_CWD is a presentation preference, not a host authority grant. The
  // Rust host deliberately starts each process with an empty in-memory grant
  // set, so acquire its canonical default on every mount even when the
  // composer can open immediately from a remembered directory. Integrated
  // terminals may use this returned root; they must never treat localStorage
  // as authorization.
  const [approvedDefaultDirectory, setApprovedDefaultDirectory] =
    useState<string | null>(null);
  const [defaultDirectoryAttempt, setDefaultDirectoryAttempt] = useState(0);
  const [composerContextRoot, setComposerContextRoot] =
    useState(() => composerDirectory.path);
  useEffect(() => {
    let active = true;
    window.iface.getDefaultDirectory().then(path => {
      if (!active) return;
      setApprovedDefaultDirectory(path);
      setComposerDirectory(current => current.ready
        ? current : { path, ready: true, error: null });
      setComposerContextRoot(current => current === "" ? path : current);
    }, (err: unknown) => {
      console.warn("[app] default directory could not be resolved:", err);
      if (active) setComposerDirectory(current => current.ready ? current : {
        path: "", ready: true,
        error: `${productLabel()} could not load the default Projects folder. ${ipcMessage(err)}`,
      });
    });
    return () => { active = false; };
  }, [defaultDirectoryAttempt]);
  const retryComposerDirectory = useCallback(() => {
    setComposerDirectory({ path: "", ready: false, error: null });
    setApprovedDefaultDirectory(null);
    setDefaultDirectoryAttempt(current => current + 1);
  }, []);
  // Chat has no project directory to resolve; the host pins its private home.
  const composerReady = composerDirectory.ready || surface === "chat";
  const contextRequestEpoch = useRef(0);
  const composerEditEpoch = useRef(0);
  const [launching, setLaunching] = useState(false);
  const launchingRef = useRef(false);
  const launchAttempt = useRef(0);
  const [sessionContext, setSessionContext] =
    useState<Record<SessionKey, PromptContextItem[]>>({});
  const collaborationDraftRevisions = useRef<Map<SessionKey, number>>(new Map());
  // The host admits a bounded number of requests. A restored workspace can
  // contain hundreds of sessions, so publishing one empty advisory state per
  // session in parallel would overflow that boundary during launch. One
  // process-wide tail preserves every session's revision order without making
  // the advisory projection compete with startup-critical IPC.
  const collaborationDraftPublishTail = useRef<Promise<void>>(Promise.resolve());
  // Whether each key's last publish said "empty": the one fact the host keeps.
  const collaborationDraftLastEmpty = useRef<Map<SessionKey, boolean>>(new Map());
  const publishCollaborationDraftState = useCallback((key: SessionKey, draft: string): void => {
    // The host refuses every other engine's session as a source.
    if (!isCollaborationSourceKey(key)) return;
    const revision = nextDraftRevision(collaborationDraftRevisions.current.get(key), Date.now());
    collaborationDraftRevisions.current.set(key, revision);
    collaborationDraftLastEmpty.current.set(key, draft.length === 0);
    const next = collaborationDraftPublishTail.current.then(() =>
      window.iface.setCollaborationDraftState({
        sourceSessionKey: key,
        revision,
        empty: draft.length === 0,
      }));
    collaborationDraftPublishTail.current = next.catch(() => undefined);
    void next.catch((err: unknown) => {
      // The host did not take it, so the next write must not be skipped as
      // already known (unless a newer publish has gone out since).
      if (collaborationDraftRevisions.current.get(key) === revision) {
        collaborationDraftLastEmpty.current.delete(key);
      }
      // Main remains fail-closed when this advisory projection cannot be
      // delivered. Do not make ordinary composing dependent on collaboration.
      console.warn("[app] collaboration draft state could not be synchronized:", ipcMessage(err));
    });
  }, []);
  // A keystroke publishes only when it changes what the host knows: the first
  // character into an empty draft, and every write that empties it.
  const publishCollaborationDraftWrite = useCallback((key: SessionKey, draft: string): void => {
    if (!draftWriteNeedsPublish(collaborationDraftLastEmpty.current.get(key), draft)) return;
    publishCollaborationDraftState(key, draft);
  }, [publishCollaborationDraftState]);
  // Conversation is allowed to remount when a pane switches sessions. Drafts
  // and echoes cannot be component-local or navigation would discard a prompt
  // the user was composing and the only live copy of their recently sent turn.
  // Drafts live in a store rather than App state: a keystroke re-renders the
  // composer that owns it and nothing else. A write is published as the
  // advisory collaboration draft state when it changes whether the draft is
  // empty (draftWriteNeedsPublish).
  const [conversationDrafts] = useState(
    () => createDraftStore({ onWrite: publishCollaborationDraftWrite }));
  const [conversationEchoes, setConversationEchoes] =
    useState<Record<SessionKey, readonly PendingEcho[]>>({});
  useEffect(() => {
    const active = new Set<SessionKey>();
    for (const summary of state.sessions) {
      const key = sessionKey(summary.engine, summary.sessionId);
      active.add(key);
      if (!collaborationDraftRevisions.current.has(key)) {
        publishCollaborationDraftState(key, conversationDrafts.get(key));
      }
    }
    for (const key of collaborationDraftRevisions.current.keys()) {
      if (!active.has(key)) {
        collaborationDraftRevisions.current.delete(key);
        collaborationDraftLastEmpty.current.delete(key);
      }
    }
  }, [publishCollaborationDraftState, state.sessions]);
  // The transcript's per-turn Restore, one map per session. Empty until this
  // run's first send captures something: the keys are that pane's own echo
  // rows, so nothing an earlier run recorded may appear here (see
  // turn-checkpoints.ts). A session with no entry is passed no map at all,
  // which is how the pane says "no control on any turn".
  const [turnRestorePoints, setTurnRestorePoints] =
    useState<Record<SessionKey, ReadonlyMap<string, TurnRestorePoint>>>({});
  // Each Threads pane's turn ordinal → prompt row, as the pane last joined
  // them (thread-turns.ts). The restore points above are keyed by row; the
  // diff panel knows only a turn, and reaches the same restore through this.
  const [threadTurnRows, setThreadTurnRows] =
    useState<Record<SessionKey, ReadonlyMap<number, string>>>({});
  const reportThreadTurnRows = useCallback((key: SessionKey, rowKeys: ReadonlyMap<number, string>) => {
    setThreadTurnRows(current => current[key] === rowKeys ? current : { ...current, [key]: rowKeys });
  }, []);
  // One rollback of one working tree at a time, across every open pane.
  const [restoringTurn, setRestoringTurn] =
    useState<{ key: SessionKey; turnKey: string } | null>(null);
  const restoringTurnRef = useRef<{ key: SessionKey; turnKey: string } | null>(null);
  // What a finished rollback did. Two tones: a clean restore is a result, not
  // an error strip, and only a conflict makes it one.
  const [turnRestoreNotice, setTurnRestoreNotice] =
    useState<{ message: string; error: boolean } | null>(null);
  const [sessionContextOpen, setSessionContextOpen] =
    useState<Record<SessionKey, boolean>>({});
  const [contextValueTarget, setContextValueTarget] = useState<string | null>(null);
  // The thread whose composer is choosing another thread to refer to (T3 section 9).
  const [threadReferenceTarget, setThreadReferenceTarget] = useState<SessionKey | null>(null);
  // A session key survives Resume, but a provider handle does not. dropLive
  // bumps this, and the outbound lane never sends across a bump.
  const sessionLifecycle = useRef<Record<SessionKey, number>>({});
  // The search box. Not persisted either — a query that survives a restart
  // would hide most of the sidebar with no visible cause on the next launch.
  const [query, setQuery] = useState("");

  const resetComposerContext = useCallback(() => {
    contextRequestEpoch.current += 1;
    closeQuickOpen();
    setContextValueTarget(null);
    setComposerContext([]);
    setComposerContextOpen(false);
    setComposerContextError(null);
  }, [closeQuickOpen]);

  // What each engine's adapter can actually be asked to do, feature-detected in
  // the main process. Empty until the answer lands, which means every lifecycle
  // action is hidden for the first moments — the safe direction: a button that
  // appears a beat late is better than one that rejects.
  const [engineOps, setEngineOps] = useState<EngineOpMap>({});
  // The last lifecycle refusal, in the engine's own words (already a sentence).
  const [actionError, setActionError] = useState<string | null>(null);
  // The Rust-owned task harness and its inspector (use-task-inspector.ts).
  // Its App callbacks are bound below, once they exist.
  const taskInspectorHost = useRef<TaskInspectorHost | null>(null);
  const taskInspector = useTaskInspector(window.iface, taskInspectorHost);
  // Rust-owned thread state for the Threads surface: the inbox and thread view
  // read `threads.view`; every change goes through `threads.request`.
  const threads = useThreads(window.iface);
  const threadRequest = threads.request;
  // Settings → Threads: one `setSettings` at a time; Rust's answer is the state.
  const [threadSettingsPending, setThreadSettingsPending] = useState(false);
  const [threadSettingsError, setThreadSettingsError] = useState<string | null>(null);
  const changeThreadSettings = useCallback((settings: ThreadSettings) => {
    setThreadSettingsPending(true);
    setThreadSettingsError(null);
    threadRequest({ kind: "setSettings", settings }).then(() => undefined, (err: unknown) => {
      setThreadSettingsError(ipcMessage(err));
    }).finally(() => { setThreadSettingsPending(false); });
  }, [threadRequest]);
  const { view: harnessView, request: harnessRequest } = taskInspector;
  const nextAttentionId = useRef<string | null>(null);
  const persistAttention = useCallback((event: Omit<AttentionRow, "live">) => {
    if (event.target.kind === "terminal" && event.target.generation === undefined) return;
    void harnessRequest({ kind: "recordAttention", item: {
      id: event.id, kind: event.kind, target: event.target as DurableAttentionTarget,
      title: event.title, place: event.where, at: event.at,
      provenance: event.target.kind === "terminal" ? "terminal-hook" : "provider", seen: false, resolved: false,
    } }).catch(() => undefined);
  }, [harnessRequest]);


  const mayOpenWorkspaceKey = useCallback((key: SessionKey,
      kind: SessionKind = surfaceRef.current): boolean => {
    const current = layoutFor(workspacesRef.current, kind);
    const reservation = kind === "chat" ? workspacePaneReservation.current : 0;
    if (mayOpenWorkspaceKeyWithReservation(
        current, key, reservation)) {
      return true;
    }
    setActionError(reservation > 0
      ? "The builder and reviewer are reserving two panes. Wait for that launch to finish."
      : "Four agent panes are already open. Close one before opening another pane.");
    return false;
  }, []);

  const { importProgress, importNote, importing, importAll, dismissImportNote } =
    useSessionImport(window.iface, setActionError);

  // Worktrees the app was told about. Only "kept" ones are surfaced, and that
  // is not a detail: a kept worktree is a directory holding an agent's
  // uncommitted work, and this notice is the ONLY place its path is ever
  // stated. Dropping it loses the work in a folder the user never hears about.
  const [notices, setNotices] = useState<WorktreeNotice[]>([]);
  // A thread's own worktree that archiving or deleting it kept (Rust says why
  // in the view once its release has run). Dismissed by hand, like the above:
  // the thread has left the list, so this is where its folder is named.
  const [keptWorktrees, setKeptWorktrees] = useState<ThreadKeptWorktree[]>([]);
  // A store Rust could not read at startup and set aside (it started empty).
  // Shown until dismissed: this is the only place the user hears of it.
  const [storeNotices, setStoreNotices] = useState<StoreNotice[]>([]);
  const keptSeen = useRef<Map<SessionKey, WorkspaceKept> | null>(null);
  useEffect(() => {
    if (threads.view === null) return;
    const { seen, kept } = threadKeptWorktrees(keptSeen.current, threads.view);
    keptSeen.current = seen;
    if (kept.length === 0) return;
    setKeptWorktrees(list => [...list.filter(notice =>
      !kept.some(next => next.key === notice.key)), ...kept]);
  }, [threads.view]);

  const { usage, usageOpen, setUsageOpen, now, refreshUsage, noteUsageTurnDone } =
    usePlanUsage(window.iface);
  const railUsage = usageRailReading(usage.views, usage.readAt, now);
  // Each provider's plan headroom (review E), for the model chips and the
  // launcher's tiles. An equal answer keeps the old object, so a clock tick
  // that changes no figure redraws no pane.
  const headroomsRef = useRef<Readonly<Record<string, UsageHeadroom>>>({});
  const headrooms = useMemo(() => {
    const next = usageHeadrooms(usage.views, usage.readAt, now);
    if (!equalValue(headroomsRef.current, next)) headroomsRef.current = next;
    return headroomsRef.current;
  }, [usage.views, usage.readAt, now]);
  const updateLabel = updateButtonLabel(updateState);
  const updateTitle = updateLabel !== null && updateState?.availableVersion
    ? `${updateLabel} — ${productLabel()} ${updateState.availableVersion}` : updateLabel ?? "";

  // The transcript each session already had before this window opened, keyed by
  // session. Kept per key rather than "the current one" so switching back to a
  // session does not re-read it — and, more importantly, so a slow read that
  // lands after the user has moved on cannot paint the wrong session's history.
  const [historyByKey, setHistoryByKey] = useState<Record<SessionKey, HistoryView>>({});
  // Keys whose read has been started, so a re-render cannot fire a second one.
  const historyAsked = useRef<Set<SessionKey>>(new Set());
  // Page size advances only on an accepted response; request generations fence
  // retries, deletes and overlapping reads from repainting a newer transcript.
  const historyPaging = useRef(new HistoryPagingController());
  // The RAW page behind each ready HistoryView, kept because the subagent
  // inspector reads a closed session's events from it. A ref: nothing renders
  // from it directly, and mirroring it into state would repaint every pane for
  // a fact only that one panel reads.
  const historyPages = useRef<Record<SessionKey,
    { events: EngineEvent[]; items: ConvItem[] }>>({});
  // Sessions born in this app run are tracked by historyPaging. Their transcript
  // is already whole in the live event log, so reads remain frozen until an
  // authoritative deletion permits that provider key to be reused.

  // Permission mode per session, mid-session. Nothing echoes back on the event
  // stream when it succeeds — no EngineEvent kind describes permission mode —
  // so this map IS the record, and a rejection is the cue to put it back.
  //
  // Being the only record is exactly why both maps are DROPPED on
  // `session.closed`: with no event to correct them, an entry left behind by a
  // run that has ended is the app asserting a policy the engine was never
  // given. See the event subscription.
  const [permissionMode, setPermissionMode] = useState<Record<SessionKey, string>>({});
  const [permissionPending, setPermissionPending] =
    useState<Record<SessionKey, boolean>>({});
  // Every settings write owns a per-session/per-field attempt token and holds
  // a send lock until it is acknowledged (session-config-tokens.ts). A lock
  // taken or released bumps configWake.
  const [configWake, setConfigWake] = useState(0);
  const [configTokens] = useState(() =>
    new ConfigAttempts(() => { setConfigWake(value => value + 1); }));
  const [launchPermission, setLaunchPermission] = useState<Record<SessionKey, string>>({});
  // changePermissionMode reads both maps through refs so its identity
  // survives every render.
  const permissionModeRef = useRef(permissionMode);
  permissionModeRef.current = permissionMode;
  const launchPermissionRef = useRef(launchPermission);
  launchPermissionRef.current = launchPermission;
  // The declutter filter. Persisted, unlike the toggle above, because this one
  // IS a setting: it hides nothing the sidebar does not immediately admit to,
  // so carrying it across launches cannot leave the user confused about where a
  // session went. Read once on mount — nothing else writes the key.
  const [hideTrivial, setHideTrivial] = useState(() => readStored(HIDE_TRIVIAL) !== "0");
  // The origin filter, persisted for the same reason and read the same way: the
  // sidebar admits in one row how many sessions it is holding back, so carrying
  // the setting across launches cannot leave the user hunting for a chat.
  const [appOnly, setAppOnly] = useState(() => readStored(APP_ONLY) !== "0");
  // Every session key selected at any point during THIS app run. The declutter
  // filter never hides one of these, and membership is permanent until restart:
  // exempting only the current selection made a row disappear the moment the
  // user clicked away from it — and take its project header with it when it was
  // the last row in the group. Not persisted, on purpose. It is a guard against
  // a row moving under the pointer in this sitting, not a preference; carrying
  // it across launches would slowly refill the sidebar with the clutter the
  // filter exists to remove.
  const [revealed, setRevealed] = useState<ReadonlySet<SessionKey>>(() => new Set());

  // Model/effort control. `pending` is the optimistic layer, `configErrors` the
  // engine's refusals, and `launchEffort` remembers what a session was launched
  // with in this process. The registry now also stores the last accepted pair,
  // so a later reopen can show and restore it after `session.closed`.
  const [pending, setPending] = useState<PendingMap>({});
  const [configErrors, setConfigErrors] = useState<Record<SessionKey, string>>({});
  const [launchEffort, setLaunchEffort] = useState<Record<SessionKey, string | null>>({});
  // Why the last launch did not happen, shown under the still-open composer.
  // A failed create used to be a console.warn and nothing else, which left the
  // card sitting there looking like the click had not registered. Codex makes
  // that reachable in normal use: the bridge now waits for the engine to mint a
  // thread id, so a bad cwd or a server that never answers fails the IPC call
  // rather than producing a session that quietly errors later.
  const [launchError, setLaunchError] = useState<string | null>(null);

  // THE APPROVAL VERDICTS — one map, above both surfaces that can answer one.
  // The sidebar and the conversation pane each used to hold a private latch, so
  // allowing in one and then opening the session offered Allow/Deny for a
  // request that had already been answered; the second verdict is dropped by
  // the bridge with a console warning, and the pane printed it as the outcome.
  // Keyed by approvalVerdictKey(sessionKey, approvalId) — session-scoped,
  // because an approvalId is only unique within its own session. Never pruned:
  // an approval is answered once, the entries are tiny, and forgetting one
  // would put the buttons back for a request that was already answered.
  const [verdicts, setVerdicts] = useState<Record<string, ApprovalVerdict>>({});
  const refreshRunning = useRef<{ run: Promise<void>; kind: RefreshKind } | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  // Settings' Terminal tools (review C4): every launch profile with Rust's
  // details, read when Settings opens and again after each provider refresh
  // (which rebuilt the catalog), so the page has one Refresh.
  const [terminalTools, setTerminalTools] = useState<TerminalLaunchProfile[] | null>(null);
  useEffect(() => {
    if (!settingsOpen || refreshing) return;
    let live = true;
    window.iface.listTerminalLaunchProfiles({ details: true })
      .then(profiles => { if (live) setTerminalTools(profiles); })
      .catch(() => undefined);
    return () => { live = false; };
  }, [settingsOpen, refreshing]);
  // Protocol 48: Grok and Antigravity use Interface's tools only once the
  // owner switches them on here; Rust changes the CLI's config and answers
  // the listing again.
  const [paneToolsPending, setPaneToolsPending] = useState<TerminalLaunchProfile["id"] | null>(null);
  const [paneToolsError, setPaneToolsError] = useState<string | null>(null);
  const changePaneTools = useCallback((profileId: TerminalLaunchProfile["id"], enabled: boolean): void => {
    setPaneToolsPending(profileId);
    setPaneToolsError(null);
    window.iface.listTerminalLaunchProfiles({ details: true, paneTools: { profileId, enabled } })
      .then(setTerminalTools)
      .catch((error: unknown) => {
        setPaneToolsError(ipcMessage(error));
        void window.iface.listTerminalLaunchProfiles({ details: true })
          .then(setTerminalTools).catch(() => undefined);
      })
      .finally(() => { setPaneToolsPending(null); });
  }, []);
  // What the last refresh could not do, one entry per failure: each is its own
  // corner notice, and `refreshError` is the one line Settings still quotes.
  const [refreshFailures, setRefreshFailures] = useState<readonly RefreshFailure[]>([]);
  const refreshError = useMemo(() => refreshErrorText(refreshFailures), [refreshFailures]);

  const refreshAll = useCallback((kind: RefreshKind = "full"): Promise<void> => {
    const running = refreshRunning.current;
    if (running !== null && REFRESH_RANK[running.kind] >= REFRESH_RANK[kind]) return running.run;
    // A focus refresh probes no provider, so it does not say "Refreshing
    // provider health…" or cost two App renders on every window focus.
    const shown = kind !== "focus";
    if (shown) setRefreshing(true);
    setRefreshFailures([]);
    const failures: RefreshFailure[] = [];
    const failed = (label: string, kind: "listing" | "health" | "capability", into = failures) =>
      (err: unknown): void => {
        console.warn(`[app] ${label} failed:`, err);
        into.push({ kind, line: `${label}: ${ipcMessage(err)}` });
      };
    // An explicit refresh lists twice; the later listing's failures stand.
    let listingFailures: RefreshFailure[] = [];
    const listSessions = (): Promise<void> => {
      const found: RefreshFailure[] = [];
      return window.iface.listSessionSnapshot()
        .then(snapshot => {
          firstListingAnswered.current = true;
          // A push still waiting for the frame arrived before this answer
          // and is at most as new: never let it land on top of it.
          pendingRegistry.current = null;
          dispatch({ type: "registry", sessions: snapshot.sessions });
          setRegistryReady(true);
          for (const provider of snapshot.providers) {
            if (!isProviderListingFailure(provider)) continue;
            const detail = provider.error ?? (provider.complete
              ? "using a stale provider listing"
              : "provider listing is incomplete; cached sessions were retained");
            found.push({ kind: "provider", engine: provider.engine,
              stale: provider.status !== "fresh", line: `${provider.engine}: ${detail}` });
          }
        })
        .catch((err: unknown) => {
          firstListingAnswered.current = true;
          failed("session refresh", "listing", found)(err);
        })
        .then(() => { listingFailures = found; });
    };
    // Asked before the listing: the host's probe restarts the provider
    // listings it keeps (Codex keeps a complete one for a minute).
    // An explicit Refresh first looks the provider CLIs up again (protocol
    // 40), so one installed since Interface started appears.
    const probe = kind === "focus"
      ? undefined
      : window.iface.probeEngines(kind === "explicit" ? { refresh: true } : undefined)
        .then(engines => { dispatch({ type: "health", engines }); })
        .catch(failed("engine health refresh", "health"));
    const listing = listSessions();
    const run = Promise.all([
      // The first listing can still start before the host restarted what it
      // keeps, so an explicit Refresh lists again once the probe answered: it
      // never ends on a listing kept from before the click.
      kind === "explicit" ? Promise.all([listing, probe]).then(listSessions) : listing,
      probe,
      // The adapters' capability map is fixed for the host process: a focus
      // asks for it no more than it probes providers.
      kind === "focus" ? undefined : window.iface.engineOps()
        .then(support => {
          // Keeping an equal map keeps canDo and lifecycle, and so
          // memo(Sidebar), from changing.
          const next = engineOpsMap(support);
          setEngineOps(current => equalValue(current, next) ? current : next);
        })
        .catch(failed("engine capability refresh", "capability")),
      // Pushes keep the list current; a focus relist only catches a missed
      // one, without the loading state's two renders.
      loadCollaborations({ quiet: kind === "focus" }),
      orchestration.load(),
    ]).then(() => {
      const all = [...listingFailures, ...failures];
      // A higher-ranked refresh that started meanwhile owns the error line.
      if (all.length > 0 && refreshRunning.current?.run === run) setRefreshFailures(all);
    });
    refreshRunning.current = { run, kind };
    void run.finally(() => {
      if (refreshRunning.current?.run !== run) return;
      refreshRunning.current = null;
      if (shown) setRefreshing(false);
    });
    return run;
  }, [loadCollaborations, orchestration]);

  const freezeHistoryFor = useCallback((key: SessionKey): void => {
    if (!historyPaging.current.freeze(key)) return;
    const late = historyPaging.current.canReadLate(key);
    setHistoryByKey(map => {
      const current = map[key];
      const frozen = frozenHistoryView(current, late);
      return frozen === current ? map : { ...map, [key]: frozen };
    });
  }, []);

  const markLive = useCallback((key: SessionKey) => {
    freezeHistoryFor(key);
    if (liveKeys.current.has(key)) return;
    liveKeys.current.add(key);
    setLiveVersion(v => v + 1);
  }, [freezeHistoryFor]);

  // The one death announcement on the wire is `session.closed`: the bridge emits
  // it exactly when a handle leaves its live map, which is the fact this set is
  // trying to mirror. Dropping the key swaps the composer strip for Resume.
  // Nothing else is a trigger. A non-retryable `error` looks like one and is
  // not: a failed turn is not a failed session, and the engine can keep the
  // handle open through it. `status.changed` to "failed" has the same problem.
  const dropLive = useCallback((key: SessionKey) => {
    freezeHistoryFor(key);
    if (!liveKeys.current.delete(key)) return;
    sessionLifecycle.current[key] = (sessionLifecycle.current[key] ?? 0) + 1;
    setLiveVersion(v => v + 1);
  }, [freezeHistoryFor]);

  // THE OUTBOUND LANE (outbound-lane.ts): every composer prompt, provider
  // command and queued steering prompt, and the durable queue. Created above
  // the event stream so the `session.closed` / failed-turn handler can pause
  // it. Its port is built once, so it reads refs, and reaches App callbacks
  // through `outboundApp`, bound below once they all exist.
  const [recoveryWake, setRecoveryWake] = useState(0);
  const outboundApp = useRef<OutboundAppHooks | null>(null);
  const { lane: outbound, snapshot: outboundSnapshot } = useOutboundLane(() => ({
    send: (key, prompt, recovery) => window.iface.send(key, prompt, recovery).finally(() => { setRecoveryWake(value => value + 1); }),
    interrupt: key => window.iface.interrupt(key),
    validateContext: (key, items) => bound(outboundApp).validateContext(key, items),
    restoreAccessMode: key => bound(outboundApp).restoreAccessMode(key),
    isLive: key => liveKeys.current.has(key),
    lifecycle: key => sessionLifecycle.current[key] ?? 0,
    configPending: key => configTokens.pending(key),
    persist: serialized => writeStored(STEERING_QUEUES, serialized),
    publishDraft: key => { bound(outboundApp).publishDraft(key); },
    checkpoint: key => { bound(outboundApp).checkpoint(key); },
    reportError: message => { setActionError(message); },
    markDelivered: key => { bound(outboundApp).markDelivered(key); },
    restoreContext: (key, items) => { bound(outboundApp).restoreContext(key, items); },
    queueItemId: () => rendererItemId("queued"),
    now: () => Date.now(),
  }), () => parseSteeringQueueMap(readStored(STEERING_QUEUES)));
  const { queues: steeringQueues, sending: queueSending, errors: queueErrors,
    notices: queueNotices } = outboundSnapshot;

  // Persist retention before the only operation that can prune pasted images.
  // The queue remains the existing durable queue; these bounded Rust hints grant no access.
  const draftAttachmentRefs = useRef({ composerContext, sessionContext });
  draftAttachmentRefs.current = { composerContext, sessionContext };
  const draftRetentionTail = useRef<Promise<void>>(Promise.resolve());
  const retainDraftAttachments = useCallback((): Promise<void> => {
    const current = draftAttachmentRefs.current;
    const contexts = [...current.composerContext, ...Object.values(current.sessionContext).flat(),
      ...outbound.snapshot().retainedContext,
      ...Object.values(outbound.snapshot().queues).flatMap(rows => rows.flatMap(row => row.context ?? []))];
    const paths = [...new Set(contexts.filter(item => item.kind === "file").map(item => item.value))];
    const next = draftRetentionTail.current.catch(() => {}).then(() =>
      window.iface.recoveryControl({ version: 1, key: "draft-attachments", kind: "retainDrafts", paths })).then(() => {});
    draftRetentionTail.current = next;
    return next;
  }, [outbound]);
  useEffect(() => {
    void retainDraftAttachments().catch(failure => { setActionError(ipcMessage(failure)); });
  }, [composerContext, sessionContext, steeringQueues, outboundSnapshot.retainedContext, retainDraftAttachments]);

  // Edge-triggered OS notification. Re-rendering the same log must not re-notify.
  // `track` is the tray's row for this same edge: recorded before the OS
  // notification, so a denied permission cannot make the tray silent too.
  // Settings › Notifications, read when an alert fires.
  const notificationSettings = useRef(notificationPreferencesOf(appPreferences));
  notificationSettings.current = notificationPreferencesOf(appPreferences);
  const notifySession = useCallback((key: SessionKey, eventId: string, kind: NotificationKind,
      title: string, body: string, track?: Omit<AttentionRow, "live">) => {
    const watching = document.hasFocus() && focusedKeyRef.current === key;
    if (track !== undefined) { setAttention(current => recordEvent(current, track)); persistAttention(track); }
    if (!shouldNotify(lastNotified.current, key, eventId, watching)) return;
    lastNotified.current = rememberNotified(lastNotified.current, key, eventId);
    writeStored(LAST_NOTIFIED, JSON.stringify(lastNotified.current));
    const plan = notificationPlan(notificationSettings.current, kind);
    postNotification(title, body, () => {
      // A notification may be clicked while a provider launch is still in
      // flight. Do not route its foreign-kind key through the launch
      // surface; the notification remains actionable after launch settles.
      if (launchingRef.current) return;
      ensureSurfaceForRef.current(key);
      dispatch({ type: "select", key });
      setWorkspace(current => switchWorkspaceKey(current, key));
      showSessionWorkspace();
      window.focus();
    }, plan);
  }, [postNotification, showSessionWorkspace, persistAttention]);

  const notifyApproval = useCallback((key: SessionKey, evt: ApprovalEvent) => {
    const questions = parseAgentQuestions(evt.tool, evt.input);
    const body = questions !== undefined ? questionSummary(questions) : evt.summary;
    notifySession(key, evt.approvalId, "needs-input", questions !== undefined
      ? `${productLabel()} — question` : `${productLabel()} — needs input`, body);
  }, [notifySession]);

  const deferredApprovalNotices = useRef(new Map<string, { key: SessionKey; evt: ApprovalEvent }>());
  const notifiedAgentApprovals = useRef(new Set<string>());
  useEffect(() => {
    for (const approval of agentStore.pendingApprovals) {
      const id = `${approval.sessionKey}:${approval.approvalId}`;
      if (notifiedAgentApprovals.current.has(id)) continue;
      notifiedAgentApprovals.current.add(id);
      notifySession(approval.sessionKey, approval.approvalId, "needs-input",
        `${productLabel()} — needs input`, approval.summary);
    }
  }, [agentStore.pendingApprovals, notifySession]);

  useEffect(() => {
    for (const [id, { key, evt }] of deferredApprovalNotices.current) {
      const session = state.sessions.find(session => sessionKey(session.engine, session.sessionId) === key);
      if (agentBoundSession(agentStore, key) || session?.kind === "agent") {
        deferredApprovalNotices.current.delete(id);
      } else if (session !== undefined && agentStartsPending.current === 0) {
        deferredApprovalNotices.current.delete(id);
        notifyApproval(key, evt);
      }
    }
  }, [agentStore, state.sessions, notifyApproval]);

  const notifyTurnDone = useCallback((key: SessionKey, status: SessionStatus) => {
    const summary = sessionsRef.current.find(session =>
      sessionKey(session.engine, session.sessionId) === key);
    const title = summary?.title ?? key;
    const at = Date.now();
    notifySession(key, `done:${at.toString()}`, "finished",
      status === "failed" ? `${productLabel()} — agent failed` : `${productLabel()} — agent finished`,
      title, {
        id: `done:${key}:${at.toString()}`,
        kind: status === "failed" ? "failed" : "finished",
        target: { kind: "session", key },
        title,
        where: attentionPlace(summary?.cwd),
        at,
      });
  }, [notifySession]);

  // Mount once: both callbacks above are stable, so this never re-subscribes.
  useEffect(() => {
    lastNotified.current = loadNotified();
    if (typeof Notification !== "undefined" && Notification.permission === "default") {
      void Notification.requestPermission().catch((err: unknown) => {
        console.warn("[app] notification permission request failed:", err);
      });
    }

    const offRegistry = window.iface.onRegistry(sessions => {
      // Latest wins; it lands with the next event flush, after its events.
      pendingRegistry.current = sessions;
      armEventFlush();
      // The cached registry the host paints before the first listing answers
      // can be empty or partial (a quarantined cache, a crash before its
      // coalesced write), so it is never the answer that prunes layouts.
      if (firstListingAnswered.current) setRegistryReady(true);
    });
    const offEvent = window.iface.onEvent((key, evt) => {
      const commandToken = configToken(key, "model");
      const commandRefresh = modelCommandRefresh.current[commandToken];
      if (commandRefresh !== undefined
          && commandRefresh.attempt === configTokens.current(key, "model")) {
        if (evt.kind === "commands.invalidated") {
          commandRefresh.phase = "invalidated";
        } else if (evt.kind === "commands.available"
            && commandRefresh.phase === "invalidated") {
          commandRefresh.phase = "available";
          if (commandRefresh.acknowledged) delete modelCommandRefresh.current[commandToken];
        }
      }
      // Any other event proves the key was live when the bridge sent it. A
      // non-retryable error is the one abstention: it neither marks nor drops,
      // because it can ride on a handle that keeps running, and when it really
      // is the end, the session.closed right behind it does the work.
      if (evt.kind === "session.closed") {
        closedKeys.current.add(key);
        dropLive(key);
      } else if (evt.kind === "config.changed") {
        // Grok effort is recorded without attaching a handle. Treating that
        // event as liveness turned the Working strip back on over a paused chat.
      } else if (evt.kind !== "error" || evt.retryable) {
        closedKeys.current.delete(key);
        markLive(key);
      }
      if (evt.kind === "approval.requested") {
        const session = sessionsRef.current.find(session => sessionKey(session.engine, session.sessionId) === key);
        const agentOwned = agentBoundSession(agentStateRef.current, key) || session?.kind === "agent";
        if (!agentOwned) {
          if (session !== undefined && agentStartsPending.current === 0) notifyApproval(key, evt);
          else deferredApprovalNotices.current.set(`${key}:${evt.approvalId}`, { key, evt });
        }
        // The agent policy decides this card in Rust; read now, once, rather
        // than leave it saying "Checking" until the next poll.
        if (agentOwned || agentStartsPending.current > 0) agentReload.current();
      }
      if (evt.kind === "status.changed") {
        const previous = lastNotifiedStatus.current[key];
        lastNotifiedStatus.current[key] = evt.status;
        if ((previous === "working" || previous === "waiting")
            && (evt.status === "idle" || evt.status === "failed")) {
          notifyTurnDone(key, evt.status);
          noteUsageTurnDone();
        }
      }
      // Turn latches for sends and the steering queue, and the durable pause
      // when a turn ends badly.
      outbound.observe(key, evt);
      // The engine has accepted a change: the optimistic layer has served its
      // purpose and the pills read from the log from here on. Any earlier
      // refusal is stale too — this is the change that worked. A closed session
      // clears the same two for the opposite reason: nothing is in flight on a
      // handle that no longer exists, and a stale optimistic value must not be
      // waiting to reappear on the pills when the session is resumed.
      if (evt.kind === "config.changed") {
        setPending(current => {
          let next = current;
          const active = current[key];
          if (active !== undefined && "model" in evt && "model" in active
              && active.model === evt.model) {
            next = withoutPending(next, key, "model");
          }
          const afterModel = next[key];
          if (afterModel !== undefined && "effort" in evt && "effort" in afterModel
              && afterModel.effort === evt.effort) {
            next = withoutPending(next, key, "effort");
          }
          return next;
        });
        setConfigErrors(e => withoutKey(e, key));
      } else if (evt.kind === "session.closed") {
        setPending(p => withoutKey(p, key));
        setConfigErrors(e => withoutKey(e, key));
      }
      // No handle-local or optimistic state crosses a run boundary. Accepted
      // model/effort values are durable in SessionSummary and the bridge may
      // restore them when it reattaches; these maps describe only the handle
      // that just closed. Permission overrides in particular are not restored,
      // so retaining one could label a newly resumed agent "plan" while it is
      // editing files. The fresh run reports its own events and otherwise the
      // controls fall back to the durable summary/provider default.
      if (evt.kind === "session.closed") {
        delete modelCommandRefresh.current[commandToken];
        configTokens.invalidate(key);
        setPermissionMode(m => withoutKey(m, key));
        setPermissionPending(m => withoutKey(m, key));
        setLaunchPermission(m => withoutKey(m, key));
        setLaunchEffort(m => withoutKey(m, key));
      }
      enqueueEvent(key, evt);
    });

    // A worktree appearing, going, or being kept. Creation can carry the
    // important warning that the isolated agent cannot see dirty source edits;
    // kept carries the location of work that was deliberately preserved.
    const offWorktree = window.iface.onWorktree(notice => {
      if (notice.state === "removed") return;
      if (notice.state === "created" && notice.detail === undefined) return;
      setNotices(list => [...list, notice]);
    });

    // Initial fill, plus a coalesced refresh whenever the user returns to the
    // window. Sessions created or renamed in a terminal should not require an
    // Interface restart to appear. Returning does not probe the providers: the
    // mount fill and the visible Refresh actions do.
    void refreshAll();
    const onFocus = (): void => { void refreshAll("focus"); };
    const onVisible = (): void => {
      if (document.visibilityState === "visible") {
        void refreshAll("focus");
        return;
      }
      // Going hidden strands whatever was waiting on an animation frame, so
      // drain the batch on the transition itself; enqueueEvent uses timers from
      // here until the window comes back.
      flushPendingEvents();
    };
    window.addEventListener("focus", onFocus);
    document.addEventListener("visibilitychange", onVisible);

    return () => {
      offRegistry();
      offEvent();
      offWorktree();
      if (pendingEventFrame.current !== null) {
        window.cancelAnimationFrame(pendingEventFrame.current);
        pendingEventFrame.current = null;
      }
      if (pendingEventTimer.current !== null) {
        window.clearTimeout(pendingEventTimer.current);
        pendingEventTimer.current = null;
      }
      pendingEventBatch.current.clear();
      window.removeEventListener("focus", onFocus);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [armEventFlush, dropLive, enqueueEvent, flushPendingEvents, markLive, noteUsageTurnDone,
    notifyApproval,
    notifyTurnDone, outbound, refreshAll]);

  const selectedKey = state.selectedKey;

  // Persist only the slots that actually moved. Switching surface changes
  // neither, and one slot changing is no reason to re-serialize the other
  // into localStorage on the interaction path.
  const persistedWorkspaces = useRef<Partial<Record<ConversationSurface, WorkspaceState>>>({});
  useEffect(() => {
    for (const kind of CONVERSATION_SURFACES) {
      if (persistedWorkspaces.current[kind] === workspaces[kind]) continue;
      persistedWorkspaces.current[kind] = workspaces[kind];
      writeStored(workspaceStorageKey(kind), JSON.stringify(workspaces[kind]));
    }
  }, [workspaces]);

  // Persistence is reconciled only after the first authoritative registry
  // answer.  Doing this against the initial empty reducer state would erase a
  // perfectly valid saved layout during every cold start.
  useEffect(() => {
    if (!registryReady) return;
    setWorkspaces(all => reconcileConversationLayouts(all, state.sessions));
  }, [registryReady, state.sessions]);

  // Workspace focus is the shell's selected session.  This covers restored
  // layouts and focus changes caused by close/cycle helpers; direct sidebar and
  // navigation clicks update both state machines in the same handler below.
  useEffect(() => {
    const key = workspace.focusedKey;
    if (key === null || key === selectedKey) return;
    if (!state.sessions.some(s => sessionKey(s.engine, s.sessionId) === key)) return;
    dispatch({ type: "select", key });
  }, [workspace.focusedKey, selectedKey, state.sessions]);

  useEffect(() => {
    const key = pendingTileFocus.current;
    if (key === null) return;
    // Scoped to the tile. A subagent cell in a transcript also carries
    // data-agent-key — a tool_use id, which no sessionKey can equal, but a
    // selector that only works because two id spaces happen not to collide is
    // not a selector worth keeping.
    const title = workspaceTile(key)
      ?.querySelector<HTMLButtonElement>(".workspace-tile-title");
    if (title === null || title === undefined) return;
    pendingTileFocus.current = null;
    title.focus();
  }, [state.sessions, workspace.focusedKey, workspace.openKeys]);

  // Watching the selected key rather than the click handler catches every way a
  // session becomes selected — a sidebar click, a fresh launch, a notification
  // click — with one rule instead of three. Returning the same Set when the key
  // is already known keeps the identity stable, so the view memo below does not
  // recompute on every render.
  useEffect(() => {
    if (selectedKey === null) return;
    setRevealed(prev => prev.has(selectedKey) ? prev : new Set(prev).add(selectedKey));
  }, [selectedKey]);

  // The return recap (use-return-recap.ts; see ./recap).
  const { recaps, setRecaps, recapAwayRef, recapTimersRef, dismissRecap } = useReturnRecap({
    workspace, focusedKeyRef, eventsRef, sessionsRef,
  });

  // Sessions grouped by project, ordered by the view-model at both levels and
  // decluttered by it too. The sidebar renders this verbatim and never re-sorts
  // or re-filters it; `trivial` is what its filter row counts.
  /*
   * A worker belongs to its orchestrator, not to the session list. Spawning a
   * four-way team otherwise dropped four more rows into the sidebar the human
   * did not start and does not steer — the canvas and the Agents panel are
   * where a worker is watched, opened and stopped. The row is only hidden: the
   * session is untouched and still reachable from either of those surfaces.
   * Rust names them on the Threads view from the whole orchestration history
   * (the snapshot keeps 64 agents, so older workers fall out of it); the
   * snapshot still covers a worker just spawned, before Rust's next view.
   */
  const threadHiddenKeys = threads.view?.hiddenKeys;
  const workerSessionKeys = useMemo(() => new Set<string>([...threadHiddenKeys ?? [],
    ...(orchestrationSnapshot?.agents ?? [])
      .filter(agent => agent.parentAgentId !== undefined && agent.role !== ORCHESTRATOR_ROLE)
      .map(agent => agent.sessionKey)
      .filter((key): key is SessionKey => key !== undefined)]),
  [orchestrationSnapshot, threadHiddenKeys]);
  // Away runs each loop as a thread (review B3): its work session is listed
  // as that thread, while its reviews (and every session of a loop from
  // before, which had a session per round) stay out of the lists; the
  // thread's cards open them. `awaySessionKeyList` is every session Away
  // started, which the quit guard counts as the loops themselves.
  const [awaySessionKeyList, setAwaySessionKeyList] = useState<string[]>([]);
  const [awayHiddenKeyList, setAwayHiddenKeyList] = useState<string[]>([]);
  const reportAwaySessionKeys = useCallback((keys: { all: string[]; hidden: string[] }) => {
    setAwaySessionKeyList(current => equalValue(current, keys.all) ? current : keys.all);
    setAwayHiddenKeyList(current => equalValue(current, keys.hidden) ? current : keys.hidden);
  }, []);
  // Running Away loops, for the quit guard.
  const [awayRunning, setAwayRunning] = useState(0);
  const hiddenSessionKeys = useMemo(
    () => new Set<string>([...workerSessionKeys, ...awayHiddenKeyList]),
    [workerSessionKeys, awayHiddenKeyList]);
  // Both workspace kinds share the project list. Their existing records own
  // their type, so a row click routes directly to its conversation or terminals.
  const sidebarSessions = useMemo(() => state.sessions.filter(summary =>
    sessionSurfaceOf(summary) === surface
      && !hiddenSessionKeys.has(sessionKey(summary.engine, summary.sessionId))),
  [state.sessions, surface, hiddenSessionKeys]);
  const view = useMemo(
    () => buildSidebarView(sidebarSessions,
      surface === "code" && centerView !== "sessions" ? null : selectedKey,
      // Threads lists terminal sessions under From terminals, so its counts
      // and empty state never call them hidden (review C1).
      revealed, hideTrivial, surface === "chat" ? false : appOnly, query, {
      terminals: surface === "code" ? terminalSummaries : [],
      registryTerminals: terminalSummaries,
      projectSessions: state.sessions,
      rememberedProjectRoots: rememberedSessionProjectRoots.current,
      selectedTerminalId: surface === "code" && centerView === "terminal"
        ? focusedTerminalId : null,
    }),
    [appOnly, centerView, focusedTerminalId, hideTrivial, query, revealed, selectedKey,
      sidebarSessions, state.sessions, surface, terminalSummaries],
  );
  const groups = view.groups;
  // The first read of the session store has not come back yet, so the sidebar
  // draws row-shaped skeletons instead of a sentence. Until this was passed the
  // skeletons in Sidebar were unreachable code and a cold start printed "No
  // agent sessions yet." over a store nobody had read — a claim that is not
  // true, and the one the affordance exists to remove.
  //
  // `registryReady` flips on the FIRST authoritative listing: the initial
  // refresh's answer, or a registry broadcast after that answer. The cached
  // registry the host paints just before it only fills rows: rows it brings
  // end the skeleton early, an empty one keeps it. `refreshError` is the other
  // exit: if that first read fails outright the listing is not coming, and
  // skeletons pulsing forever would be a second untrue claim on top of the
  // first — so the pane falls through to its own zero-state and the failure is
  // reported in the notice strip instead.
  // LATCHED, not read live off `refreshError`. That error is dismissible, and
  // dismissing it used to set this expression back to true while
  // `registryReady` was still false — putting the pane into a loading state it
  // would never leave, over a listing that had already failed and was not
  // coming. Once the first read has settled either way, it has settled.
  const [firstListingSettled, setFirstListingSettled] = useState(false);
  useEffect(() => {
    if (registryReady || refreshError !== null) setFirstListingSettled(true);
  }, [registryReady, refreshError]);
  const sidebarLoading = !registryReady && !firstListingSettled && state.sessions.length === 0;
  // THE attention selection, and the only one in this file. Taken off the
  // registry listing itself — the same array and the same predicate the dock
  // badge counts (src/main/badge.ts). One fact, four renderings: the badge for
  // when this window is behind something else, the toolbar pill and its live
  // region for when it is not, and the session this pill navigates to, taken
  // from THIS ARRAY rather than a second filter over a different one.
  // See attentionSessions for why this is not taken off `view.visible`.
  const attentionListing = useMemo(() => {
    // Same rows in the same order keep their identity, so the tray settles.
    const reused = reuseList(previousByEventsRef.current.attentionListing,
      attentionSessions(state.sessions));
    previousByEventsRef.current.attentionListing = reused;
    return reused as SessionSummary[];
  }, [state.sessions]);
  const waitingSessions = attentionListing.length;
  // Every session's log, not just the selected one: the badge belongs to rows
  // the user is not looking at as much as to the one they are.
  const rateLimits = useMemo(() => {
    const next: Record<SessionKey, string> = {};
    for (const [key, log] of Object.entries(state.events)) {
      const notice = perLog(logCachesRef.current.rateLimit, log, rateLimitNotice);
      if (notice !== undefined) next[key] = notice;
    }
    const reused = reuseRecord(previousByEventsRef.current.rateLimits, next);
    previousByEventsRef.current.rateLimits = reused;
    return reused;
  }, [state.events]);
  // Lines changed per session, from the diffs the engines streamed — the only
  // per-session change data that reaches this renderer at all (see ./diff-stats).
  // A key with no diff event is simply absent, which is what the tiles and cards
  // read as "unknown" and draw as nothing.
  const diffStats = useMemo(() => {
    const previous = previousByEventsRef.current.diffStats;
    const next: Record<SessionKey, DiffStat> = {};
    for (const [key, log] of Object.entries(state.events)) {
      const stat = perLog(logCachesRef.current.diffFacts, log, sessionDiffFacts).stat;
      if (stat === null) continue;
      // Same counts as last frame: keep last frame's object, so reuseRecord's
      // identity check below can hand Sidebar the record it already has.
      const prior = previous?.[key];
      next[key] = prior !== undefined && prior.added === stat.added
        && prior.removed === stat.removed ? prior : stat;
    }
    const reused = reuseRecord(previous, next);
    previousByEventsRef.current.diffStats = reused;
    return reused;
  }, [state.events]);
  // What the sidebar's hover preview shows and its row cannot: the last
  // activity line, how many files an agent has touched, and the words of the
  // request holding it up. All three come off the same event log the sidebar
  // rows are built from, through the same derivations — the card is a second
  // rendering of state this window already holds, and asks the main process
  // for nothing.
  const sidebarPeeks = useMemo(() => {
    const previous = previousByEventsRef.current.peeks;
    const next: Record<SessionKey, SessionPeekFacts> = {};
    for (const [key, log] of Object.entries(state.events)) {
      const facts = perLog(logCachesRef.current.peek, log, events => sessionPeekFacts(events,
        perLog(logCachesRef.current.diffFacts, events, sessionDiffFacts).paths.length));
      if (facts === null) continue;
      // As for diffStats: a streaming session whose line, file count and
      // request did not change keeps last frame's object.
      const prior = previous?.[key];
      next[key] = prior !== undefined && prior.activity === facts.activity
        && prior.files === facts.files && prior.approval === facts.approval ? prior : facts;
    }
    const reused = reuseRecord(previous, next);
    previousByEventsRef.current.peeks = reused;
    return reused;
  }, [state.events]);
  // What each live session's log says for its Threads row's second line: the
  // step it is on, what it asks, why it failed. Same logs, cached per log the
  // same way; Threads only, since no other surface draws it. A session whose
  // reading did not change keeps last frame's object, so its row stays put.
  const threadLive = useMemo(() => {
    if (surface !== "chat") return NO_THREAD_LIVE;
    const previous = previousByEventsRef.current.threadLive;
    const next: Record<SessionKey, ThreadLiveFacts> = {};
    for (const [key, log] of Object.entries(state.events)) {
      const facts = perLog(logCachesRef.current.threadLive, log, threadLiveFacts);
      if (facts === null) continue;
      const prior = previous?.[key];
      next[key] = prior !== undefined && sameThreadLiveFacts(prior, facts) ? prior : facts;
    }
    const reused = reuseRecord(previous, next);
    previousByEventsRef.current.threadLive = reused;
    return reused;
  }, [state.events, surface]);
  // `state.selectedKey` is the last globally selected session and deliberately
  // survives an empty workspace. Surface-owned consumers must not read through
  // that global fallback: doing so can hand Chat's private home to Code's
  // terminal, or paint Code's cwd on an empty Chat surface.
  const surfaceWorkspaceKey = workspace.focusedKey;
  /** The registry indexed by session key: the render path resolves keys per tile. */
  const sessionByKey = useMemo(() => new Map(state.sessions.map(summary =>
    [sessionKey(summary.engine, summary.sessionId), summary])), [state.sessions]);
  // For callbacks that look a session up when they run. The map changes with
  // every registry push (the 1 Hz activity rounds included); depending on it
  // re-minted selectSession, the voice listener and the ⌘P key handler.
  const sessionByKeyRef = useRef(sessionByKey);
  sessionByKeyRef.current = sessionByKey;
  const surfaceSession = useMemo(() => {
    if (surfaceWorkspaceKey === null) return null;
    const summary = sessionByKey.get(surfaceWorkspaceKey);
    return summary !== undefined && sessionSurfaceOf(summary) === surface ? summary : null;
  }, [sessionByKey, surface, surfaceWorkspaceKey]);
  const surfaceSessionKey = surfaceSession === null ? null : surfaceWorkspaceKey;
  const agentTaskSession = surface === "agent" && surfaceSession !== null
    ? surfaceSession : undefined;
  const agentTaskKey = agentTaskSession === undefined
    ? null : sessionKey(agentTaskSession.engine, agentTaskSession.sessionId);
  // Which agent a session belongs to: its assignment, else whose thread
  // (current or past) it is.
  const agentOwnerOf = useCallback((key: SessionKey): AgentProfile | undefined => {
    const assigned = agentForSession(agentStore, key);
    if (assigned !== undefined) return assigned;
    const owner = agentThreadOwner(agentStore, key);
    return owner === undefined ? undefined : agentStore.agents.find(agent => agent.id === owner);
  }, [agentStore]);
  // Whose header the Agent surface wears: the agent the user just picked in the
  // menu, and otherwise the one that launched the task on screen.
  const activeAgent = useMemo<AgentProfile | undefined>(() => {
    const selected = selectedAgentId === null
      ? undefined : agentStore.agents.find(agent => agent.id === selectedAgentId);
    if (selected !== undefined) return selected;
    return agentTaskKey === null ? undefined : agentOwnerOf(agentTaskKey);
  }, [agentOwnerOf, agentStore, agentTaskKey, selectedAgentId]);
  // An agent deleted while it was selected leaves nothing to point at.
  useEffect(() => {
    if (selectedAgentId !== null
      && !agentStore.agents.some(agent => agent.id === selectedAgentId)) {
      setSelectedAgentId(null);
    }
  }, [agentStore, selectedAgentId]);
  // Opening ANOTHER agent's task moves the selection to its owner, so the menu
  // and the header never disagree about what the pane is showing. Keyed to a
  // CHANGE of task: re-running it on every store write would drag the selection
  // back the moment the user picked an agent that has no tasks yet.
  const shownAgentTaskKey = useRef<SessionKey | null>(null);
  useEffect(() => {
    if (agentTaskKey === shownAgentTaskKey.current) return;
    shownAgentTaskKey.current = agentTaskKey;
    const owner = agentTaskKey === null
      ? undefined : agentOwnerOf(agentTaskKey);
    if (owner !== undefined) setSelectedAgentId(owner.id);
  }, [agentOwnerOf, agentTaskKey]);
  const agentWorkingCount = useMemo(() => activeAgent === undefined ? 0
    : state.sessions.filter(summary => summary.status === "working"
      && sessionSurfaceOf(summary) === "agent"
      && agentForSession(agentStore, sessionKey(summary.engine, summary.sessionId))?.id
        === activeAgent.id).length,
  [activeAgent, agentStore, state.sessions]);
  const agentDiffStats = useMemo(() => perLog(logCachesRef.current.touched,
    agentTaskKey === null ? NO_EVENTS : state.events[agentTaskKey] ?? NO_EVENTS,
    touchedFileStats),
  [agentTaskKey, state.events]);
  // The raw inputs of the task's Activity inspector. AgentTask reduces them only
  // while the inspector is open and on screen, never per streamed frame here.
  const agentTaskEvents = agentTaskKey === null
    ? NO_EVENTS : state.events[agentTaskKey] ?? NO_EVENTS;
  const agentTaskHistory = agentTaskKey === null
    || historyPaging.current.startedHere(agentTaskKey) ? undefined : historyByKey[agentTaskKey];
  // The canvas and its coordination panel stay mounted while hidden, with the
  // rows they last drew; a registry push rebuilds them only while shown.
  const canvasShown = surface === "code" && centerView === "canvas";
  const collaborationSessions = useMemo<CollaborationSessionOption[]>(() => {
    const previous = previousByEventsRef.current.collaborationSessions;
    if (!canvasShown && previous !== undefined) return previous as CollaborationSessionOption[];
    const prevByKey = new Map((previous ?? []).map(row => [row.key, row] as const));
    const next = state.sessions.map(summary => {
      const key = sessionKey(summary.engine, summary.sessionId);
      const prev = prevByKey.get(key);
      if (prev !== undefined
          && prev.title === summary.title
          && prev.engine === summary.engine
          && prev.status === summary.status) {
        return prev;
      }
      return {
        key,
        title: summary.title,
        engine: summary.engine,
        status: summary.status,
      };
    });
    const reused = reuseList(previous, next);
    previousByEventsRef.current.collaborationSessions = reused;
    return reused as CollaborationSessionOption[];
  }, [canvasShown, state.sessions]);

  // Reused row by row, like collaborationSessions: registry pushes that change
  // no canvas-visible field must not re-render memo(AgentCanvas).
  const canvasSessions = useMemo(() => {
    const previous = previousByEventsRef.current.canvasSessions;
    if (!canvasShown && previous !== undefined) return previous;
    const rows = canvasSessionRows(previous, state.sessions);
    previousByEventsRef.current.canvasSessions = rows;
    return rows;
  }, [canvasShown, state.sessions]);
  /* SHELVED 2026-08-27: the reveal latch that decided when the canvas earned a
     toolbar button. There is no button while the feature is withdrawn. */
  const canvasProjectRoots = useMemo(() => {
    const previous = previousByEventsRef.current.canvasProjectRoots;
    if (!canvasShown && previous !== undefined) return previous;
    const roots = new Set<string>();
    for (const agent of orchestrationSnapshot?.agents ?? []) roots.add(canvasProjectRoot(agent));
    for (const session of canvasSessions) {
      if (session.cwd !== "") roots.add(session.cwd);
    }
    for (const terminal of terminalSummaries) {
      if (terminal.cwd !== "") roots.add(terminal.cwd);
    }
    const sorted = reuseList(previous, [...roots].sort());
    previousByEventsRef.current.canvasProjectRoots = sorted;
    return sorted;
  }, [canvasShown, canvasSessions, orchestrationSnapshot, terminalSummaries]);
  const activeCanvasRoot = canvasProjectRoots.includes(canvasRoot)
    ? canvasRoot
    : canvasProjectRoots[0] ?? "";
  const { coordination: coordinationStatus, models: orchestratorModels } =
    useCanvasReads(window.iface, activeCanvasRoot, orchestrationReads, canvasShown);

  // An explicit project seed wins for this draft only. A normal New Chat still
  // re-reads the last successfully launched directory at the moment it opens.
  const composerDefaults = useMemo<ComposerLaunchForm>(() => {
    const chat = surface === "chat";
    // On the Agent surface the selected agent IS the launch default: its
    // provider and model/effort. Blank fields are resolved by this launch,
    // without inheriting another agent's last-used model or effort.
    const agent = surface === "agent" ? activeAgent : undefined;
    const lastEngine = chat
      ? "claude" : agent?.engine ?? readStored(LAST_ENGINE) ?? DEFAULT_ENGINE;
    const last = latestModelPresetFor(modelPresets, lastEngine);
    return {
      engine: lastEngine,
      cwd: chat ? "" : composerSeed.cwd ?? readStored(LAST_CWD) ?? composerDirectory.path,
      prompt: composerSeed.prompt,
      model: agent !== undefined ? agent.model : (last?.model ?? ""),
      effort: agent !== undefined ? agent.effort
        : (last !== undefined && last.effort !== "default" ? last.effort : ""),
      // An agent that stops to ask reads as a hung agent, so a fresh composer
      // opens on the engine's own non-blocking mode, not on its default.
      permissionMode: chat ? "default" : startingPermissionMode(lastEngine,
        ENGINE_CHOICES[lastEngine] ?? UNKNOWN_ENGINE_CHOICES),
      settingScope: "all",
      disallowedTools: "",
      // Worktrees are per-launch opt-in. A saved app preference must never move
      // every new agent away from the checkout the user selected.
      worktree: composerSeed.worktree === true,
      ...COMPOSER_TEAM_DEFAULTS,
    };
  }, [activeAgent, composerDirectory.path, composerSeed, modelPresets, state.composerOpen,
    surface]);

  // The probe's answer IS the main process's adapter map: one entry per
  // registered engine, whatever its health, in registration order. Health rides
  // along so a down engine is labelled rather than hidden — hiding it would
  // read as "this build has no Codex", which is a different fact. The cheap
  // operation-capability call normally returns before provider model probes, so
  // it seeds every registered adapter while catalogues are still loading;
  // only the very first paint falls back to Claude.
  const engines = useMemo<EngineOption[]>(() => {
    const configuredLabels = new Map(acpProfiles.map(profile =>
      [`acp:${profile.slug}`, profile.displayName]));
    if (state.engineHealth.length === 0) {
      const registered = Object.keys(engineOps);
      return (registered.length > 0 ? registered : [DEFAULT_ENGINE]).map(id => ({
        id,
        ...(configuredLabels.get(id) === undefined ? {} : { label: configuredLabels.get(id) }),
      }));
    }
    return state.engineHealth.map(h => {
      const label = h.info?.displayName ?? configuredLabels.get(h.engine);
      return h.ok
        ? { id: h.engine, ...(label === undefined ? {} : { label }) }
        : { id: h.engine, ...(label === undefined ? {} : { label }),
          unavailable: h.error ?? "engine did not respond" };
    });
  }, [acpProfiles, engineOps, state.engineHealth]);
  const engineLabels = useMemo<Readonly<Record<string, string>>>(() =>
    Object.fromEntries(engines.map(engine => [
      engine.id, providerLabel(engine.id, engine.label),
    ])), [engines]);
  // The sidebar footer's setup notice (owner, 2026-09-28; one amber line in
  // the Settings row since 2026-11-15), memoized because Sidebar is memo()'d.
  const providerAlert = useMemo(() => {
    // Signed out (the CLI's own status command says so) needs setup too.
    const down = state.engineHealth.filter(health => !health.ok || health.info?.auth === "signedOut");
    if (down.length === 0) return undefined;
    const name = (engine: string) => engineLabels[engine] ?? providerLabel(engine);
    const label = down.length === 1 ? `${name(down[0].engine)} needs setup` : `${down.length} providers need setup`;
    return {
      label,
      shortLabel: down.length === 1 ? label : `${down.length} need setup`,
      description: down.map(health => {
        if (health.ok) {
          return signedOutSentence(name(health.engine), signInProfile(health.engine) !== null);
        }
        const raw = health.error ?? "provider did not respond";
        return providerFaultSentence(name(health.engine), raw) ?? `${name(health.engine)}: ${raw}`;
      }).join("\n"),
      // Says the visible words first, then where they lead.
      accessibleName: `${label}: open Models & providers`,
    };
  }, [engineLabels, state.engineHealth]);

  // The agent form offers exactly the providers the composer does, down to the
  // down ones being listed and disabled rather than hidden: a missing Codex row
  // reads as "this build has no Codex", which is a different fact.
  // ACP engines are listed too, disabled with the reason: Rust refuses their
  // threads and routines, so the form must not create an agent on one.
  const agentEngines = useMemo<readonly AgentEngineOption[]>(() => engines.map(engine => {
    const refusal = agentEngineRefusal(engine.id);
    return {
      id: engine.id,
      label: engineLabels[engine.id] ?? providerLabel(engine.id),
      available: engine.unavailable === undefined && refusal === null,
      ...(refusal === null ? {} : { unavailableReason: refusal }),
    };
  }), [engineLabels, engines]);
  // Effort vocabularies are per provider and the form has no live session to
  // ask, so it uses the same static fallback the composer falls back to.
  const agentEffortLevels = useCallback((engine: string): readonly string[] =>
    concreteEffortLevels(fallbackEffortLevels(engine)), []);
  const agentDialogInitial = useMemo<AgentProfile | null>(() => {
    if (agentDialog === null) return null;
    if (agentDialog.mode === "edit") {
      return agentStore.agents.find(agent => agent.id === agentDialog.id) ?? null;
    }
    // A new agent arrives with its identity and colour already decided; the
    // form only has to fill in the words.
    return {
      id: agentDialog.id,
      name: "",
      engine: newAgentEngine(readStored(LAST_ENGINE), DEFAULT_ENGINE),
      model: "",
      effort: "",
      instructions: "",
      color: nextAgentColor(agentStore),
    };
  }, [agentDialog, agentStore]);
  const agentBeingDeleted = agentDeleteId === null
    ? undefined : agentStore.agents.find(agent => agent.id === agentDeleteId);

  // Settings keeps every built-in visible while the asynchronous probes are
  // still running. Once a probe arrives it replaces the neutral Checking row;
  // third-party ACP adapters are appended in the main process's reported order.
  const settingsEngineHealth = useMemo<EngineProbe[]>(() => {
    const reported = new Map(state.engineHealth.map(probe => [probe.engine, probe]));
    const ids: EngineId[] = [];
    for (const id of [...BUILT_IN_ENGINES, ...(Object.keys(engineOps) as EngineId[]),
      ...state.engineHealth.map(probe => probe.engine)]) {
      if (!ids.includes(id)) ids.push(id);
    }
    return ids.map(engine => reported.get(engine) ?? {
      engine,
      ok: false,
      // A neutral missing row means the first provider probe is still in
      // flight. Once that refresh has failed, calling it "Checking" forever
      // is false; use the visible refresh failure as the unavailable reason.
      ...(refreshing || refreshError === null ? {} : { error: refreshError }),
    });
  }, [engineOps, refreshError, refreshing, state.engineHealth]);

  // Bumped only when some session's newest models.available changed — the
  // one thing choicesFor reads from the logs — so choicesFor's identity, and
  // the composer memos keyed on it, survive frames that merely streamed text
  // while a catalogue a live session reports still reaches an open composer.
  const catalogueRef = useRef<CatalogueRevision>({ byKey: new Map(), revision: 0 });
  const catalogueRevision = useMemo(() => {
    const next = nextCatalogueRevision(catalogueRef.current, state.events);
    catalogueRef.current = next;
    return next.revision;
  }, [state.events]);

  // Called during the composer's render with whatever engine is selected there.
  // Pure lookup: provider probe catalogue first, then any catalogue a live
  // session reported, then value-only capabilities. A non-empty observed
  // catalogue is authoritative; built-in rows are a cold/down-probe fallback,
  // not stale additions to a newer provider list. Events come through the
  // render-assigned ref: the closure is minted on catalogueRevision, not on
  // every log identity. The answer depends on engine health and each log's
  // newest models.available, exactly what those two track, so one answer per
  // engine holds for the closure's life. Keying it on the live logs instead
  // missed on every streamed frame and rebuilt every list.
  const choicesCache = useMemo(() => new Map<string, EngineChoices>(),
    [state.engineHealth, catalogueRevision]);
  const choicesFor = useCallback((engine: string): EngineChoices => {
    const known = choicesCache.get(engine);
    if (known !== undefined) return known;
    const events = eventsRef.current;
    const value = engineChoices(engine, state.engineHealth, events);
    choicesCache.set(engine, value);
    return value;
  // choicesCache is minted on engine health and catalogueRevision, which
  // stands in for state.events, deliberately: see above.
  }, [state.engineHealth, choicesCache]);

  // Whether this engine takes text INTO a turn that is already running. Read
  // from the probe's own capabilities for the same reason engineCan reads the
  // reported op list: deciding from the engine's NAME is a hardcoded table that
  // goes stale the first time an adapter gains the path. An engine nobody has
  // reported on cannot steer — the composer keeps its durable queue.
  const canSteerEngine = useCallback((engine: string): boolean =>
    state.engineHealth.find(h => h.engine === engine)?.info?.capabilities.steer === true,
  [state.engineHealth]);

  // Both setters are optimistic-then-authoritative: show the ask immediately,
  // let config.changed confirm it, and on a refusal put the field back and say
  // why. The revert is per-field so a failed model change cannot silently undo
  // an effort change still in flight beside it.
  const changeModelFor = useCallback((key: SessionKey, model?: string) => {
    const token = configToken(key, "model");
    const write = configTokens.begin(key, "model");
    const attempt = write.attempt;
    modelCommandRefresh.current[token] = {
      attempt, phase: "pending", acknowledged: false,
    };
    setPending(p => withPending(p, key, { model }));
    setConfigErrors(e => withoutKey(e, key));
    window.iface.setModel(key, model).then(({ key: activeKey }) => {
      if (!write.isCurrent()) return;
      // A model selection on a paused chat is also its deliberate lazy resume.
      // The main process owns the atomic resume+set transaction; publish that
      // liveness here even when the provider emitted no event before replying.
      // Every successful model mutation proves a live handle, including a
      // retry after an earlier invalid model left the newly attached handle up.
      closedKeys.current.delete(activeKey);
      markLive(activeKey);
      const commandRefresh = modelCommandRefresh.current[token];
      if (commandRefresh?.attempt === attempt) {
        commandRefresh.acknowledged = true;
        if (commandRefresh.phase !== "available") {
          // The adapter accepted the model before resolving this IPC call, but
          // its event pump is independent. Clear locally if that invalidation
          // has not already been followed by the replacement catalogue.
          dispatch({ type: "event", key: activeKey,
            evt: { kind: "commands.invalidated" } });
        } else {
          delete modelCommandRefresh.current[token];
        }
      }
      write.settle();
    }, (err: unknown) => {
      if (!write.isCurrent()) return;
      if (modelCommandRefresh.current[token]?.attempt === attempt) {
        delete modelCommandRefresh.current[token];
      }
      write.settle();
      setPending(p => withoutPending(p, key, "model"));
      setConfigErrors(e => ({ ...e, [key]: ipcMessage(err) }));
    });
  }, [configTokens, markLive]);

  // A confirmed effort or access change is remembered against the model the
  // session runs now, keeping the preset's other field. Continuations read the
  // latest registry and log, not the snapshot captured at click time.
  const rememberConfirmedPreset = useCallback((key: SessionKey,
      patch: Pick<ModelPreset, "effort"> | Pick<ModelPreset, "permissionMode">): void => {
    const { engine } = parseKey(key);
    const summary = sessionsRef.current.find(session =>
      sessionKey(session.engine, session.sessionId) === key);
    const confirmed = configInForce(eventsRef.current[key] ?? NO_EVENTS);
    const model = confirmed.model ?? summary?.model;
    if (model === undefined || model === "" || model === "default") return;
    setModelPresets(current => {
      const existing = modelPresetFor(current, engine, model);
      return rememberModelPreset(current, {
        engine,
        model,
        effort: existing?.effort ?? "default",
        permissionMode: existing?.permissionMode ?? "default",
        ...patch,
      });
    });
  }, []);

  const changeEffortFor = useCallback((key: SessionKey, effort: string | null) => {
    const write = configTokens.begin(key, "effort");
    setPending(p => withPending(p, key, { effort }));
    setConfigErrors(e => withoutKey(e, key));
    window.iface.setEffort(key, effort).then(() => {
      // A resolution from a superseded pick must not write ITS effort into the
      // remembered preset: the session is running the newer one.
      if (!write.isCurrent()) return;
      if (effort !== null && effort !== "" && effort !== "default") {
        rememberConfirmedPreset(key, { effort });
      }
      write.settle();
    }, (err: unknown) => {
      if (!write.isCurrent()) return;
      write.settle();
      setPending(p => withoutPending(p, key, "effort"));
      setConfigErrors(e => ({ ...e, [key]: ipcMessage(err) }));
    });
  }, [configTokens, rememberConfirmedPreset]);

  const changeFastModeFor = useCallback((key: SessionKey, enabled: boolean) => {
    const write = configTokens.begin(key, "fastmode");
    setConfigErrors(errors => withoutKey(errors, key));
    window.iface.setFastMode(key, enabled).then(() => {
      if (!write.isCurrent()) return;
      write.settle();
    }, (err: unknown) => {
      if (!write.isCurrent()) return;
      write.settle();
      setConfigErrors(errors => ({ ...errors, [key]: ipcMessage(err) }));
    });
  }, [configTokens]);

  /* ---- the session's past transcript ------------------------------------- */

  // One INITIAL read per session per app run. `historyAsked` is a ref rather
  // than derived from `historyByKey`, because between the call and its answer
  // the map has no entry — and this component re-renders on every engine event.
  // Older pages are explicit user requests and replace the previous page: the
  // frozen provider contract returns a strict superset for a larger limit.
  //
  // A LATE read: a session that went live before any page of it was read (a
  // host-started send at launch: a scheduled or watch prompt, a resume, a team
  // notice, an outside agent) gets one read anyway, cut to what predates its
  // live stream (`historyBeforeLive`), or the pane shows only the new turn.
  const loadHistory = useCallback((key: SessionKey, limit = HISTORY_LIMIT,
      mode: "initial" | "older" = "initial") => {
    const late = mode === "initial" && historyPaging.current.canReadLate(key);
    if (!late && historyPaging.current.isFrozen(key)) return;
    if (mode === "initial" && !late) {
      if (historyAsked.current.has(key)) return;
      historyAsked.current.add(key);
    }
    const generation = late ? historyPaging.current.beginLate(key) : historyPaging.current.begin(key);
    if (generation === null) return;
    if (mode === "older") {
      setHistoryByKey(map => {
        const current = map[key];
        if (current?.status !== "ready") return map;
        return {
          ...map,
          [key]: { ...current, loadingOlder: true, loadOlderError: undefined },
        };
      });
    }
    const failOlder = (message: string): void => {
      setHistoryByKey(map => {
        const current = map[key];
        if (current?.status !== "ready") return map;
        return {
          ...map,
          [key]: { ...current, loadingOlder: false, loadOlderError: message },
        };
      });
    };
    window.iface.readHistory(key, { limit })
      .then(page => {
        // freeze() invalidates this generation permanently, including when the
        // handle has already closed again by the time this continuation runs.
        if (!historyPaging.current.isCurrent(key, generation)) return;
        if (page.availability === "unavailable") {
          if (!historyPaging.current.fail(key, generation)) return;
          const message = page.reason
            ?? "This provider does not expose stored transcript replay.";
          if (mode === "older") failOlder(message);
          else {
            setHistoryByKey(map => ({
              ...map,
              [key]: { status: "unavailable", message },
            }));
          }
          return;
        }
        if (!historyPaging.current.settle(key, generation, limit)) return;
        const events = late
          ? historyBeforeLive(page.events, eventsRef.current[key] ?? NO_EVENTS) : page.events;
        const items = historyPageItems(events, historyPages.current[key]?.items);
        // Replace, never concatenate. A larger provider page contains the exact
        // previous suffix; concatenation would duplicate every already-drawn row.
        historyPages.current[key] = { events, items };
        setHistoryByKey(map => ({
          ...map,
          [key]: {
            status: "ready",
            items,
            plan: latestPlanEntries(events),
            // The COUNT, not just the fact. "Older messages are not shown."
            // withholds what makes paging actionable.
            olderMessages: page.olderMessages,
            truncated: page.olderMessages > 0,
            // A late page is the last: the session is live, so no "Load older".
            canLoadOlder: !late && page.olderMessages > 0 && limit < HISTORY_MAX_LIMIT,
          },
        }));
      })
      .catch((err: unknown) => {
        if (!historyPaging.current.fail(key, generation)) return;
        const message = ipcMessage(err);
        if (mode === "older") failOlder(message);
        else {
          setHistoryByKey(map => ({
            ...map, [key]: { status: "error", message },
          }));
        }
      });
  }, []);

  // `liveVersion`: a session that just went live before its read gets its
  // late read here (loadHistory refuses every other frozen read).
  useEffect(() => {
    for (const key of workspace.openKeys) {
      if (!historyPaging.current.startedHere(key)) loadHistory(key);
    }
  }, [workspace.openKeys, liveVersion, loadHistory]);

  const retryHistoryFor = useCallback((key: SessionKey) => {
    if (historyPaging.current.isFrozen(key)) return;
    historyPaging.current.invalidate(key);
    historyAsked.current.delete(key);
    delete historyPages.current[key];
    setHistoryByKey(map => withoutKey(map, key));
    loadHistory(key);
  }, [loadHistory]);

  const loadOlderHistoryFor = useCallback((key: SessionKey) => {
    if (historyPaging.current.isFrozen(key)) return;
    const current = historyByKey[key];
    if (current?.status !== "ready" || current.loadingOlder === true) return;
    if ((current.olderMessages ?? (current.truncated === true ? 1 : 0)) <= 0) return;
    const limit = historyPaging.current.nextLimit(key);
    if (limit !== null) loadHistory(key, limit, "older");
  }, [historyByKey, loadHistory]);

  /* ---- session lifecycle (the engine's own store) ------------------------- */

  // Every one of these writes to the ENGINE's store, outlives this app and is
  // visible from the vendor CLI. The bridge answers a refusal as a written
  // sentence (lead + detail), so it is shown verbatim rather than parsed.
  const failAction = useCallback((err: unknown) => { setActionError(ipcMessage(err)); }, []);

  /** End liveness, not merely the current turn, before a lifecycle write. */
  const closeLiveSession = useCallback(async (key: SessionKey): Promise<void> => {
    if (!liveKeys.current.has(key)) return;
    outbound.pause(key,
      "Queue paused because the live provider session was closed.");
    await window.iface.closeSession(key);
    // The bridge emits session.closed while draining, but publish the completed
    // IPC result immediately even if a provider omitted that final event.
    closedKeys.current.add(key);
    dropLive(key);
  }, [dropLive, outbound]);

  const renameSession = useCallback((key: SessionKey, title: string) => {
    setActionError(null);
    window.iface.renameSession(key, title).catch(failAction);
  }, [failAction]);

  const dropSessionClientState = useCallback((key: SessionKey): void => {
    historyPaging.current.drop(key);
    pendingEventBatch.current.drop(key);
    historyAsked.current.delete(key);
    delete historyPages.current[key];
    closedKeys.current.delete(key);
    dispatch({ type: "drop", key });
    releaseConversationProjection(key);
    setHistoryByKey(map => withoutKey(map, key));
    // The queue, its banners and every send lock the session held.
    outbound.drop(key);
    setSessionContext(map => withoutKey(map, key));
    setSessionContextOpen(map => withoutKey(map, key));
    conversationDrafts.delete(key);
    setConversationEchoes(map => withoutKey(map, key));
    // Its echoes are gone, so every row key its checkpoints name is gone too.
    // The stored copy goes with them: this origin's quota is shared with the
    // steering queue and the workspace layout, and a key nothing can read is
    // one nobody gets back.
    setTurnRestorePoints(map => withoutKey(map, key));
    setThreadTurnRows(map => withoutKey(map, key));
    forgetTurnCheckpoints(key);
    // The return recap's three stores. ORDER MATTERS FOR ANY CALLER THAT CLOSES
    // A TILE FIRST: closeWorkspaceTile moves attention off the key and writes a
    // fresh absence entry for it on the way out — so dropping the entry here,
    // AFTER that has happened, is what stops a reused key from opening onto a
    // briefing about the session it replaced.
    // deleteSession, the only caller today, does not close a tile at all; it
    // has no absence entry landing behind it, and clearing the three stores is
    // unconditional, so it needs no ordering of its own.
    const recapTimer = recapTimersRef.current[key];
    if (recapTimer !== undefined) window.clearTimeout(recapTimer);
    recapTimersRef.current = withoutKey(recapTimersRef.current, key);
    recapAwayRef.current = withoutKey(recapAwayRef.current, key);
    setRecaps(map => withoutKey(map, key));
    // A deleted session is never notified again; its edge record goes with it.
    if (key in lastNotified.current) {
      lastNotified.current = withoutKey(lastNotified.current, key);
      writeStored(LAST_NOTIFIED, JSON.stringify(lastNotified.current));
    }
  }, [outbound]);

  /**
   * Every worker session this orchestrator spawned. They are hidden from the
   * sidebar, so deleting the lead is the only chance the human gets to be rid
   * of them — leaving them behind would be leaving rows they cannot see and
   * provider sessions they never started.
   */
  const workerSessionsOf = useCallback((key: SessionKey): SessionKey[] => {
    const agents = orchestrationSnapshot?.agents ?? [];
    const lead = agents.find(agent => agent.sessionKey === key);
    if (lead === undefined) return [];
    return agents
      .filter(agent => agent.parentAgentId === lead.agentId
        && agent.role !== ORCHESTRATOR_ROLE)
      .map(agent => agent.sessionKey)
      .filter((child): child is SessionKey => child !== undefined);
  }, [orchestrationSnapshot]);

  const deleteSession = useCallback((key: SessionKey) => {
    // The confirmation happened in the sidebar. Nothing below this asks again —
    // the adapter's own doc comment says so — which is why that step is real.
    setActionError(null);
    const workers = workerSessionsOf(key);
    closeLiveSession(key)
      .then(async () => {
        // The lead goes last: if a worker's delete fails the human still has
        // the chat that owns them, rather than an orphaned set with no handle.
        for (const worker of workers) {
          await closeLiveSession(worker).catch(() => undefined);
          await window.iface.deleteSession(worker).then(
            () => { dropSessionClientState(worker); },
            () => undefined,
          );
        }
      })
      .then(() => window.iface.deleteSession(key))
      .then(() => { dropSessionClientState(key); })
      .catch(failAction);
  }, [closeLiveSession, dropSessionClientState, failAction, workerSessionsOf]);

  const forkSession = useCallback((key: SessionKey) => {
    const root = sessionByKeyRef.current.get(key)?.cwd;
    setActionError(null);
    window.iface.forkSession(key)
      // The new key is already in the registry, so selecting it is enough; its
      // history reads from the engine's store like any other existing session.
      .then(async ({ key: forked }) => {
        if (root) await harnessRequest({ kind: "recordBranch", branch: { parent: key, child: forked, root, boundary: "provider-current" } }).catch(() => setActionError("The provider created the branch, but its parent link could not be saved. The source conversation remains available."));
        // Fork completion is asynchronous and can race a separate composer
        // launch. Leave the fork in the registry instead of contaminating the
        // launch surface's workspace slot.
        if (launchingRef.current) return;
        ensureSurfaceForRef.current(forked);
        dispatch({ type: "select", key: forked });
        setWorkspace(current => switchWorkspaceKey(current, forked));
        showSessionWorkspace();
      })
      .catch(failAction);
  }, [failAction, showSessionWorkspace, harnessRequest]);

  const engineArchive = useCallback((key: SessionKey) => {
    setActionError(null);
    closeLiveSession(key)
      .then(() => window.iface.setEngineArchived(key, true))
      .catch(failAction);
  }, [closeLiveSession, failAction]);

  const adoptSession = useCallback((key: SessionKey, adopt: boolean) => {
    setActionError(null);
    window.iface.adoptSession(key, adopt).catch(failAction);
  }, [failAction]);

  const stopSession = useCallback((key: SessionKey) => {
    outbound.pause(key,
      "Queue paused after Stop. Use Send now to resume queued follow-ups.");
    window.iface.interrupt(key).catch(failAction);
  }, [failAction, outbound]);

  const focusAfterNoticeDismissal = useCallback(() => {
    window.requestAnimationFrame(() => {
      const next = Array.from(document.querySelectorAll<HTMLElement>(
        ".notice-region .notice-close, .corner-notice-dismiss",
      )).find(node => node.isConnected && node.getClientRects().length > 0 && node.closest("[inert]") === null);
      (next ?? commandPaletteButtonRef.current)?.focus();
    });
  }, []);

  const dismissActionError = useCallback(() => {
    // Focus moves on only from the error's own Dismiss (or from nowhere): the
    // sidebar's copy fading out by itself never takes focus from where the
    // owner is working (hands-free run, 6 Oct 2026: a refused Archive pulled
    // focus off the inbox row eight seconds later).
    const active = document.activeElement;
    const fromError = active === null || active === document.body
      || active.closest(".corner-notice, .sidebar-say-error") !== null;
    setActionError(null);
    if (fromError) focusAfterNoticeDismissal();
  }, [focusAfterNoticeDismissal]);

  /* ---- permission mode, mid-session --------------------------------------- */

  // What this session is running under. There is no event to read it back from,
  // so the record is: what the user has chosen here, else what it was launched
  // with, else the engine's default. Both maps drop on `session.closed`.
  const changePermissionMode = useCallback((key: SessionKey, mode: string) => {
    const previous = modeInForce(
      sessionsRef.current.find(session => sessionKey(session.engine, session.sessionId) === key),
      { chosen: permissionModeRef.current[key], launched: launchPermissionRef.current[key] });
    if (mode === previous) return;
    const write = configTokens.begin(key, "permission");
    // Optimistic, then reverted on a refusal — the same shape as model and
    // effort, and necessary for the same reason: nothing confirms this on the
    // event stream, so the UI's own state is the only record there is.
    setPermissionMode(map => ({ ...map, [key]: mode }));
    setPermissionPending(map => ({ ...map, [key]: true }));
    setConfigErrors(e => withoutKey(e, key));
    window.iface.setPermissionMode(key, mode).then(() => {
      if (!write.isCurrent()) return;
      // A paused chat's access change is also its deliberate lazy resume,
      // matching setModel. Publish liveness so commands and the picker stay
      // attached without waiting for the first prompt.
      closedKeys.current.delete(key);
      markLive(key);
      write.settle();
      setPermissionPending(map => withoutKey(map, key));
      rememberConfirmedPreset(key, { permissionMode: mode });
    }, (err: unknown) => {
      if (!write.isCurrent()) return;
      write.settle();
      setPermissionPending(map => withoutKey(map, key));
      setPermissionMode(map => ({ ...map, [key]: previous }));
      setConfigErrors(e => ({ ...e, [key]: ipcMessage(err) }));
    });
  }, [configTokens, markLive, rememberConfirmedPreset]);

  const browse = useCallback((preferredPath?: string) =>
    window.iface.pickDirectory(preferredPath), []);

  const reauthorizePromptContext = useCallback(async (
      target: PromptContextValidationTarget,
      items: readonly PromptContextItem[]): Promise<PromptContextItem[]> => {
    if (items.length === 0) return [];
    const result = await window.iface.validatePromptContext(target, items);
    if (!result.ok) {
      throw new Error(result.invalid[0]?.message
        ?? "Prompt context could not be reauthorized and was not sent.");
    }
    // The main process canonicalizes path values and replaces Diff bytes with
    // a fresh credential-filtered repository snapshot. Never serialize the
    // renderer/localStorage copies after this boundary.
    return result.items;
  }, []);

  const appendConversationEcho = useCallback((key: SessionKey, echo: PendingEcho) => {
    setConversationEchoes(current => ({
      ...current,
      [key]: [...(current[key] ?? []), echo],
    }));
  }, []);

  const failConversationEcho = useCallback((key: SessionKey, id: string, error: string) => {
    setConversationEchoes(current => ({
      ...current,
      [key]: (current[key] ?? []).map(echo => echo.id === id
        ? { ...echo, error }
        : echo),
    }));
  }, []);

  /**
   * A teammate's one-off task, started by Rust behind the agent's gate. While
   * it binds, an approval on a session not yet known may be this task's, so
   * it waits for the policy as a thread start's does (agentStartsPending).
   * Resolves with the new session's key; rejects with Rust's reason.
   */
  const startAgentTask = useCallback(async (command: Extract<AgentRequest, { kind: "startAgentTask" }>):
      Promise<{ key: SessionKey }> => {
    agentStartsPending.current += 1;
    if (agentStartsPending.current === 1) setAgentStartKnownKeys(new Set(sessionByKeyRef.current.keys()));
    try {
      const result = await requestAgent(command);
      if (!result.ok) throw new Error(result.error);
      const key = result.response.sessionKey;
      if (key === undefined) throw new Error("The task started without a session to open. Reload the agent before trying again.");
      return { key };
    } finally {
      agentStartsPending.current -= 1;
      if (agentStartsPending.current === 0) {
        setAgentStartKnownKeys(null);
        agentReload.current();
      }
    }
  }, [requestAgent]);

  const acknowledgeCreatedLaunch = useCallback(async (key: SessionKey): Promise<void> => {
    try {
      const saved = await window.iface.recoveryControl({ version: 1, kind: "read", key });
      if (!saved.receipt) throw new Error("The started thread's launch receipt could not be read.");
      await window.iface.recoveryControl({ version: 1, kind: "acknowledgeLaunch", key, id: saved.receipt.id });
    } catch (failure) {
      setActionError(`The thread started. Its duplicate-launch guard remains saved: ${ipcMessage(failure)}`);
    } finally { setRecoveryWake(value => value + 1); }
  }, []);

  // `scope` is the composer's SECOND argument and carries every scoping control
  // it offers: settingSources (which on-disk settings load — 'project' is what
  // makes CLAUDE.md load at all), allowedTools/disallowedTools, and worktree.
  // Ignoring it would silently drop everything the user set in that panel, so
  // it is spread into the request beside the validated fields.
  // It resolves with the new session's key and the prompt the provider was
  // given, or null when it failed (the card then shows why). The Threads card
  // (`threadCard`) holds the launch lock across its first thread's prepare →
  // launch → bind sequence and starts a ⌘Enter thread in the background,
  // without switching to it; its launches leave the Code/Agent card's
  // remembered engine and folder alone. Its other models' threads (`extra`)
  // start after the lock is released, in the background: they take no lock,
  // leave the card and its pickers alone, and reject with their reason.
  const launch = useCallback((form: ComposerLaunchForm, scope: ScopeOptions,
      contextItems: readonly PromptContextItem[] = [],
      launchOptions: { background?: boolean; threadCard?: boolean; extra?: boolean } = {},
  ): Promise<{ key: SessionKey; prompt: string } | null> => {
    const extra = launchOptions.extra === true;
    const threadCard = launchOptions.threadCard === true || extra;
    const background = launchOptions.background === true || extra;
    if (launchingRef.current && !threadCard) return Promise.resolve(null);
    // A new attempt supersedes whatever the last one failed with, so the line
    // under the card always belongs to the click that produced it.
    if (!extra) setLaunchError(null);
    const opts = buildLaunchOptions(form);
    // The composer validates before calling, so this is a belt-and-braces
    // guard rather than a user-facing path.
    if ("error" in opts) {
      console.warn("[app] launch rejected:", opts.error);
      if (extra) return Promise.reject(new Error(opts.error));
      setLaunchError(opts.error);
      return Promise.resolve(null);
    }
    // Permission mode's "default" means "let the engine decide", and it must
    // not travel on the wire: Codex rejects it outright and the launch
    // fails on its first round-trip. The rule and the measured evidence live in
    // the view-model, where they are tested; the tidier long-term home is
    // buildLaunchOptions, beside the model and effort cases it already handles.
    const baseRequest = withoutDefaultPermissionMode({ ...opts, ...scope });
    // The agent as it stands at the click. Provider creation is asynchronous and
    // the menu stays live throughout, so the task must be recorded against the
    // agent that launched it and not against whatever is selected when the
    // provider answers.
    const agent = opts.kind === "agent" ? activeAgent : undefined;
    const attempt = extra ? launchAttempt.current : launchAttempt.current + 1;
    launchAttempt.current = attempt;
    const capturedContextIds = new Set(contextItems.map(item => item.id));
    const releaseLaunchContext = outbound.captureContext(contextItems);
    if (!extra) {
      launchingRef.current = true;
      setLaunching(true);
      // Freeze this exact form/context snapshot. Any native folder picker that
      // resolves after the click is stale, and no covered context modal remains
      // interactive while a provider session is being created.
      contextRequestEpoch.current += 1;
      closeQuickOpen();
      setContextValueTarget(null);
      setComposerContextOpen(false);
    }
    // `request` carries the chosen engine alongside the LaunchOptions fields;
    // the bridge routes on it and answers with a key prefixed by that engine.
    // A working directory that is not there is the launch failure this app hits
    // most often and reported worst: `spawn` returns ENOENT against the BINARY
    // when it is the cwd that is missing, and the Claude SDK — finding its
    // binary present — then blames the system's libc. The path is still a path
    // here, and the composer is still open to correct it.
    const workingDirectoryReady: Promise<unknown> = opts.kind === "chat"
      ? Promise.resolve()
      : window.iface.listDir(opts.cwd).catch(() => {
        throw new Error(`Working directory does not exist: ${opts.cwd}. Check the path, `
          + "including a trailing space in a folder name (this field is trimmed).");
      });
    return workingDirectoryReady
      .then(() => reauthorizePromptContext({ root: opts.cwd }, contextItems))
      .then(validatedContext => {
        // Sent AND echoed, so the transcript the human reads is the prompt the
        // engine was given. A teammate's task goes through its own gated start
        // (protocol 38): Rust adds its job, home, folders and permission mode,
        // and files the task under it. The composer's permission and scoping
        // choices do not apply to a teammate; its own settings do.
        const launchPrompt = opts.prompt;
        const request = {
          ...baseRequest,
          prompt: serializePromptWithContext(launchPrompt, validatedContext),
          recovery: {
            text: launchPrompt,
            context: validatedContext.map(({ previewUrl: _preview, ...descriptor }) => descriptor),
          },
        };
        const created: Promise<{ key: SessionKey }> = agent === undefined
          ? window.iface.createSession(request)
          : startAgentTask({ ...agentTaskCommand(agent, opts, opts.prompt, request.prompt,
            crypto.randomUUID()), recovery: request.recovery });
        return created.then(({ key }) => {
          // Chat has no project choice of its own, and its engine is forced.
          // Do not let either seed the next Code or Agent launch card.
          // A Threads card launch keeps its own memory (thread-card.ts).
          if (opts.kind !== "chat") {
            if (!threadCard) {
              writeStored(LAST_ENGINE, opts.engine);
              writeStored(LAST_CWD, opts.cwd);
            }
          }
          // This session's whole transcript arrives on the live stream, so its
          // tile must never read the same history back and draw every turn twice.
          historyPaging.current.markStartedHere(key);
          // Live engines never emit the user's turn, and this session will not
          // load history, so the launch prompt has to be echoed here or it
          // exists only as the sidebar title.
          appendConversationEcho(key, launchPendingEcho(
            launchPrompt, promptTurnImages(validatedContext), promptTurnAttachments(validatedContext)));
          // Nothing on the wire reports permission mode either, so the launch
          // value is the only thing the status-bar picker can open at. Read off
          // `request`, not `opts`: a launch that sent no mode at all must leave
          // the picker showing "default" — the engine's own setting — rather than
          // recording a mode this session was never given.
          const launchedMode = agent === undefined ? request.permissionMode : undefined;
          if (launchedMode !== undefined) {
            setLaunchPermission(map => ({ ...map, [key]: launchedMode }));
          }
          // Nothing on the wire will tell us this again: LaunchOptions.effort is
          // not a SessionSummary field and the engine emits no meta event for it,
          // so the pill would read "default" until the first change without this.
          setLaunchEffort(m => ({ ...m, [key]: opts.effort ?? null }));
          // createSession adopts the handle before it returns, so the session is
          // live from this moment — no Resume button on a brand-new session.
          if (!closedKeys.current.has(key)) markLive(key);
          void acknowledgeCreatedLaunch(key);
          if (extra) return { key, prompt: request.prompt };
          setComposerContext(current =>
            current.filter(item => !capturedContextIds.has(item.id)));
          setComposerContextOpen(false);
          if (background) return { key, prompt: request.prompt };
          dispatch({ type: "select", key });
          pendingTileFocus.current = key;
          // A split asked for another pane; every other launch replaces the
          // focused one, which is what New Chat has always done. The flag is
          // read into a local BEFORE the update is queued, because React may run
          // the updater long after this line: read from inside it, the ref would
          // already be back to `false` and every split would replace the focused
          // pane instead of appending one.
          const asNewPane = pendingNewPane.current;
          pendingNewPane.current = false;
          setWorkspace(current => asNewPane
            ? openWorkspaceKey(current, key) : switchWorkspaceKey(current, key));
          showSessionWorkspace();
          dispatch({ type: "composer", open: false });
          return { key, prompt: request.prompt };
        });
      })
      .catch((err: unknown) => {
        console.warn("[app] createSession failed:", err);
        // An extra model's thread: the card it came from is gone, so its
        // caller reports the reason with the model it was for.
        if (extra) throw err;
        // The composer is still open — it is only closed on success, above —
        // so the message lands directly under the Launch button that produced
        // it, and the form the user filled in is still there to correct.
        setLaunchError(ipcMessage(err));
        return null;
      })
      .finally(() => {
        releaseLaunchContext();
        setRecoveryWake(value => value + 1);
        // The Threads card releases the lock it holds itself.
        if (launchAttempt.current === attempt && !threadCard) {
          launchingRef.current = false;
          setLaunching(false);
        }
      });
  }, [activeAgent, acknowledgeCreatedLaunch, appendConversationEcho, closeQuickOpen, markLive, outbound, reauthorizePromptContext,
    showSessionWorkspace, startAgentTask]);

  // Turn-bound checkpoints: capture, re-key, restore (use-turn-restore.ts).
  const { checkpointTurn, restoreTurnCheckpoint } = useTurnRestore(window.iface, {
    state, sessionsRef, conversationEchoes, historyByKey, settledTurnWake, setSettledTurnWake,
    turnRestorePoints, setTurnRestorePoints, restoringTurnRef, setRestoringTurn, setTurnRestoreNotice,
  });

  const dismissComposer = useCallback((restoreFocus: boolean) => {
    if (launchingRef.current) return;
    pendingNewPane.current = false;
    dispatch({ type: "composer", open: false });
    resetComposerContext();
    if (!restoreFocus) return;
    window.requestAnimationFrame(() => {
      const opener = composerOpenerRef.current;
      if (opener !== null && opener.isConnected) {
        opener.focus();
        return;
      }
      document.querySelector<HTMLButtonElement>(".sidebar-nav-button")?.focus();
    });
  }, [resetComposerContext]);

  /**
   * Leave the Settings page. NOT `closeSettings`: that restores focus to
   * whatever opened Settings, which is exactly wrong here — the click that
   * navigated has already moved focus, and yanking it back to the toolbar gear
   * would undo the navigation the user just made. The chrome lock follows
   * `settingsOpen` in its own effect, so this one line is the whole close.
   */
  const leaveSettings = useCallback(() => { setSettingsOpen(false); }, []);

  /** Switch surfaces while retaining the last selected Code workspace. */
  const showSurface = useCallback((kind: SessionKind) => {
    // Launch is deliberately non-cancellable. The async success arm routes the
    // new key through the active surface ref, so changing that ref while the
    // provider is starting would put (for example) a Chat key in Code's slot.
    if (launchingRef.current) return;
    // Settings is a destination. Navigating away from it leaves it, rather
    // than moving the surface underneath a card that hides the result. AFTER
    // the guard above: a click this function refuses stays a no-op.
    leaveSettings();
    if (kind !== surfaceRef.current) {
      writeStored(SURFACE_KEY, kind);
      surfaceRef.current = kind;
      setSurfaceState(kind);
    }
    setCenterView(kind === "code" ? codeDestinationRef.current : "sessions");
    dismissComposer(false);
  }, [dismissComposer, leaveSettings]);

  /** Session navigation uses the same category as the sidebar. A key the
      listing does not hold yet stays on Chat or Agent, and leaves Code (which
      holds no conversations) for Chat. */
  const ensureSurfaceFor = useCallback((key: SessionKey) => {
    const summary = sessionByKeyRef.current.get(key);
    if (summary !== undefined || surfaceRef.current === "code") {
      showSurface(conversationSurfaceFor(summary, surfaceRef.current));
    }
  }, [showSurface]);
  ensureSurfaceForRef.current = ensureSurfaceFor;

  /* ---- the session callbacks ---------------------------------------------
     EVERY one of these must be referentially stable: memo(Sidebar),
     memo(AgentCanvas) and memo(IntegratedTerminalCenter) compare their props
     by identity, so an inline arrow handed to one repaints it on every engine
     event. They take the key as their first argument for exactly this reason:
     none of them may close over the current selection. Session panes reach
     them through sessionTileActions, one committed object whose members never
     change identity. */

  // Opening a thread is visiting it: Rust clears its unread mark. Agent tasks
  // are never threads, so they are not asked about. Automatic, so a refusal
  // is not the owner's to read.
  const visitThread = useCallback((key: SessionKey) => {
    const summary = sessionByKeyRef.current.get(key);
    if (summary === undefined || sessionSurfaceOf(summary) !== "chat") return;
    void threadRequest({ kind: "visit", key }).catch(() => undefined);
  }, [threadRequest]);
  // The Threads surface's focused pane as last visited for it (see the
  // effect after selectSession).
  const visitedFocus = useRef<SessionKey | null>(null);

  const selectSession = useCallback((key: SessionKey) => {
    // Launch is deliberately non-cancellable: do not let sidebar navigation
    // hide its only progress/error surface while provider creation continues.
    if (launchingRef.current) return;
    leaveSettings();
    setActionError(null);
    ensureSurfaceFor(key);
    dispatch({ type: "select", key });
    visitThread(key);
    // Visited here, so the focus effect below does not send it again.
    visitedFocus.current = key;
    setWorkspace(current => switchWorkspaceKey(current, key));
    showSessionWorkspace();
    // Selecting an agent is also an explicit dismissal of the launch sheet.
    dispatch({ type: "composer", open: false });
    closeMobileSidebar();
  }, [closeMobileSidebar, ensureSurfaceFor, leaveSettings, showSessionWorkspace, visitThread]);

  // Every other way a thread becomes the focused pane of the Threads surface
  // visits it too, under selectSession's rules: a pane clicked or tabbed into
  // (focusWorkspaceTile), ⌃Tab and ⌘⌥←/→ (cycleWorkspace), maximize
  // (toggleAgentMaximized), a neighbour closing, coming back to the surface.
  // Watched here rather than in each handler, so a new path cannot miss it.
  // Before the first registry listing the layout is only being restored,
  // which nobody chose: it is recorded, not visited.
  const focusedThreadPane = surface === "chat" && centerView === "sessions"
    ? workspace.focusedKey : null;
  useEffect(() => {
    const visit = threadFocusVisit(visitedFocus.current, focusedThreadPane, registryReady);
    visitedFocus.current = focusedThreadPane;
    if (visit !== null) visitThread(visit);
  }, [focusedThreadPane, registryReady, visitThread]);

  /* ---- Voice assistant ----------------------------------------------------
     The Rust host owns the helper process, the voice session and every state
     change; this holds the last frame it pushed and draws it. `null` is "the
     host has said nothing yet", which renders exactly as off. */
  // The project the assistant should work in: whatever the open surface is
  // pointed at, else the folder the composer would launch into.
  const voiceCwd = surfaceSession?.cwd ?? composerDirectory.path;
  const { voice, voiceState, voiceBusy, toggleVoice } = useVoiceAssistant({
    api: window.iface, cwd: voiceCwd, selectSession, reportError: setActionError,
  });
  const voiceLevelBars = voiceMeterBars(voice?.level ?? null);
  /**
   * Point the Agent surface at one agent. Its most recent task is a destination
   * too — that is what makes the card a place to go — and an agent that has not
   * run anything yet shows its own zero-state rather than another agent's task
   * under its header.
   */
  const selectAgent = useCallback((agentId: string) => {
    setSelectedAgentId(agentId);
    // An agent with a thread opens on it; its page mounts that conversation.
    const thread = agentStore.threads?.[agentId]?.current;
    if (thread !== undefined) { selectSession(thread.sessionKey); return; }
    let newest: SessionSummary | undefined;
    for (const summary of state.sessions) {
      if (sessionSurfaceOf(summary) !== "agent"
        || agentStore.assignments[sessionKey(summary.engine, summary.sessionId)] !== agentId) {
        continue;
      }
      if (newest === undefined || summary.lastActivity > newest.lastActivity) newest = summary;
    }
    if (newest !== undefined) selectSession(sessionKey(newest.engine, newest.sessionId));
    else setWorkspace(() => EMPTY_WORKSPACE_STATE);
  }, [agentStore, selectSession, setWorkspace, state.sessions]);

  // Identity is minted here, once. Minting it inside agentDialogInitial gave the
  // open form a new id every time the agent store changed underneath it.
  const openNewAgentDialog = useCallback(() => {
    if (agentReady) setAgentDialog({ mode: "new", id: newAgentId() });
  }, [agentReady]);

  const submitAgentProfile = useCallback((profile: AgentProfile, permission: AgentPermissionMode) => {
    const change = permissionModeChange(profile.id, agentPermissionMode(agentStateRef.current, profile.id), permission);
    void mutateAgent({ kind: "saveAgent", profile }).then(async saved => {
      if (!saved) return;
      // The mode is stored beside the roster, so it saves once the agent exists.
      // A refusal keeps the dialog open over the saved profile to try again.
      if (change !== null) {
        const result = await requestAgent(change);
        if (!result.ok) { setAgentError(result.error); return; }
      }
      setSelectedAgentId(profile.id);
      if (agentDialog?.mode === "new") setWorkspace(() => EMPTY_WORKSPACE_STATE);
      setAgentDialog(null);
    });
  }, [agentDialog?.mode, mutateAgent, requestAgent, setWorkspace]);

  /**
   * The agent page's requests. A thread the host just created is launched here
   * exactly like a new session: its transcript arrives live, so it never reads
   * history, and its first message is echoed because live engines never emit
   * the user's turn. It is then focused, which is what mounts it on the page.
   */
  const requestAgentPage = useCallback(async (command: AgentRequest): Promise<AgentRequestResult> => {
    const starting = command.kind === "startAgentThread";
    if (starting) {
      agentStartsPending.current += 1;
      if (agentStartsPending.current === 1) setAgentStartKnownKeys(new Set(sessionByKeyRef.current.keys()));
    }
    try {
      const result = await requestAgent(command);
      if (result.ok && command.kind === "startAgentThread" && result.response.sessionKey !== undefined) {
        const key = result.response.sessionKey;
        historyPaging.current.markStartedHere(key);
        appendConversationEcho(key, launchPendingEcho(command.text, []));
        if (!closedKeys.current.has(key)) markLive(key);
        dispatch({ type: "select", key });
        // The Agent surface's own slot, even if the owner moved to Code meanwhile.
        setWorkspaceFor("agent", current => switchWorkspaceKey(current, key));
      }
      return result;
    } finally {
      if (starting) {
        agentStartsPending.current -= 1;
        if (agentStartsPending.current === 0) {
          setAgentStartKnownKeys(null);
          // An approval that reached Rust after the answer was built is only in
          // the next read; take it now rather than at the next poll.
          agentReload.current();
        }
      }
    }
  }, [appendConversationEcho, markLive, requestAgent, setWorkspaceFor]);

  /** The owner asked for an agent's thread: its current session, or the first-message form. */
  const showAgentThread = useCallback((agentId: string) => {
    if (launchingRef.current) return;
    setSelectedAgentId(agentId);
    const current = agentStateRef.current.threads?.[agentId]?.current;
    if (current !== undefined) { selectSession(current.sessionKey); return; }
    showSurface("agent");
    setWorkspaceFor("agent", () => EMPTY_WORKSPACE_STATE);
  }, [selectSession, setWorkspaceFor, showSurface]);

  const openSessionInNewPane = useCallback((key: SessionKey) => {
    if (launchingRef.current) return;
    ensureSurfaceFor(key);
    if (!mayOpenWorkspaceKey(key)) return;
    setActionError(null);
    dispatch({ type: "select", key });
    setWorkspace(current => openWorkspaceKey(current, key));
    showSessionWorkspace();
    dispatch({ type: "composer", open: false });
    closeMobileSidebar();
  }, [closeMobileSidebar, ensureSurfaceFor, mayOpenWorkspaceKey, showSessionWorkspace]);

  const reserveAdversarialReviewPanes = useCallback((count: number): boolean => {
    if (launchingRef.current || !Number.isInteger(count) || count < 1) return false;
    const total = workspacePaneReservation.current + count;
    if (!hasWorkspacePaneCapacity(workspacesRef.current.chat, total)) return false;
    workspacePaneReservation.current = total;
    return true;
  }, []);

  const releaseAdversarialReviewPanes = useCallback((count: number): void => {
    if (!Number.isInteger(count) || count < 1) return;
    workspacePaneReservation.current = Math.max(0, workspacePaneReservation.current - count);
  }, []);

  const openAdversarialReviewSessions = useCallback((
      builder: SessionKey, reviewer: SessionKey, objective: string): boolean => {
    // The terminal launch resolves asynchronously. A composer launch may have
    // started after its pane reservation; returning false lets the terminal
    // release that reservation and report the still-durable sessions.
    if (launchingRef.current) return false;
    if (workspacePaneReservation.current < 2
        || !hasWorkspacePaneCapacity(workspacesRef.current.chat, 2)) {
      setActionError("Close an agent pane before opening the builder and reviewer together.");
      return false;
    }
    workspacePaneReservation.current -= 2;
    setActionError(null);
    // Orchestration owns the sole provider stream and forwards it live. Reading
    // persisted history as well would duplicate prose that cannot be keyed.
    historyPaging.current.markStartedHere(builder);
    historyPaging.current.markStartedHere(reviewer);
    appendConversationEcho(builder, launchPendingEcho(objective, []));
    appendConversationEcho(reviewer, launchPendingEcho(
      `Review this objective after the builder finishes:\n\n${objective}`, []));
    // The host starts both without a session kind, so Chat owns them; its
    // layout is the one the reservation held two panes in.
    showSurface("chat");
    dispatch({ type: "select", key: reviewer });
    setWorkspaceFor("chat", state => openWorkspaceKey(openWorkspaceKey(state, builder), reviewer));
    showSessionWorkspace();
    dispatch({ type: "composer", open: false });
    closeMobileSidebar();
    return true;
  }, [closeMobileSidebar, setWorkspaceFor, showSessionWorkspace, showSurface]);

  const focusWorkspaceTile = useCallback((key: SessionKey) => {
    dispatch({ type: "select", key });
    setWorkspace(current => openWorkspaceKey(current, key));
    showSessionWorkspace();
    dispatch({ type: "composer", open: false });
  }, [showSessionWorkspace]);

  const closeWorkspaceTile = useCallback((key: SessionKey) => {
    // Focus follows a closing pane to the next one only when it was in that
    // pane, or nowhere. An Archive from a sidebar menu keeps the sidebar's
    // focus, and closing a key no pane shows moves nothing.
    const active = document.activeElement;
    const focusLeaving = active === null || active === document.body
      || workspaceTile(key)?.contains(active) === true;
    setWorkspace(current => {
      const next = closeWorkspaceKey(current, key);
      if (focusLeaving && current.openKeys.includes(key)) pendingTileFocus.current = next.focusedKey;
      return next;
    });
    // A closed pane's stored page is dead weight for the rest of the run, and
    // reopening re-reads it. A key the live stream has already frozen can never
    // be re-read, so that one keeps its page rather than losing it for good.
    // The freeze/startedHere fences themselves are untouched either way.
    if (historyPaging.current.isFrozen(key)) return;
    // invalidate(), not just historyAsked: it clears the SETTLED LIMIT too, so a
    // reopen re-reads the page the user had expanded with "load older" instead
    // of silently dropping back to HISTORY_LIMIT. It also retires the active
    // generation, so a read still in flight cannot repopulate what we just
    // freed. frozen/started are untouched.
    historyPaging.current.invalidate(key);
    delete historyPages.current[key];
    historyAsked.current.delete(key);
    setHistoryByKey(map => withoutKey(map, key));
  }, []);

  // The subagent detail column's selection, routes and data (use-subagent-detail.ts).
  const {
    subagentDetail, setSubagentDetail, subagentDetailRef, subagentNameRef,
    openSubagentDetail, closeSubagentDetail, retargetSubagentDetail, revealSubagentDetail,
    detailView, detailSession,
  } = useSubagentDetail({
    state, workspace, historyByKey, historyPages, liveKeys, sessionByKey, settingsOpen, centerView,
    focusWorkspaceTile,
  });

  const cycleWorkspace = useCallback((direction: "next" | "previous") => {
    setWorkspace(current => {
      const next = cycleWorkspaceFocus(current, direction);
      pendingTileFocus.current = next.focusedKey;
      return next;
    });
    showSessionWorkspace();
  }, [showSessionWorkspace]);

  const toggleAgentMaximized = useCallback((key: SessionKey) => {
    setWorkspace(current => toggleWorkspaceMaximized(current, key));
    dispatch({ type: "select", key });
    showSessionWorkspace();
  }, [showSessionWorkspace]);

  const {
    sessionLibraryOpen, sessionLibraryTab, setSessionLibraryTab,
    sessionSearchQuery, setSessionSearchQuery, sessionSearchHits, sessionSearchLoading,
    sessionSearchError, sessionSearchIncomplete, archivedSessions, archivedSessionsLoading,
    managedWorktrees, managedWorktreesLoading, sessionLibraryNotice, setSessionLibraryNotice,
    refreshSessionLibrary, searchSessionLibrary, openSessionLibrary, searchTranscriptsFor,
    restoreSessionFromLibrary, openSessionFromLibrary, openWorktreeFromLibrary,
    openTerminalForWorktree, cleanupWorktreeFromLibrary, closeSessionLibrary,
  } = useSessionLibrary({
    api: window.iface, refreshAll, selectSession, closeQuickOpen, dismissComposer,
    launchingRef, pendingTileFocus,
  });

  // Resolves whether the session was archived, so the Threads inbox offers
  // Undo only for an archive that happened.
  const hideSessionLocally = useCallback((key: SessionKey): Promise<boolean> => {
    setActionError(null);
    return closeLiveSession(key)
      .then(() => window.iface.archive(key, true))
      .then(() => {
        closeWorkspaceTile(key);
        if (sessionLibraryOpen) void refreshSessionLibrary();
        return true;
      })
      .catch((err: unknown) => {
        failAction(err);
        return false;
      });
  }, [closeLiveSession, closeWorkspaceTile, failAction, refreshSessionLibrary,
    sessionLibraryOpen]);

  const addContextFor = useCallback((target: string, item: PromptContextItem) => {
    if (target === COMPOSER_CONTEXT_TARGET) {
      setComposerContext(current => addPromptContextItem(current, item).items);
      return;
    }
    setSessionContext(current => ({
      ...current,
      [target]: addPromptContextItem(current[target] ?? [], item).items,
    }));
  }, []);

  const contextAttachError = useCallback((target: string, message: string) => {
    if (target === COMPOSER_CONTEXT_TARGET) setComposerContextError(message);
    else failAction(new Error(message));
  }, [failAction]);

  /** The same split for a rejected request, with ipcMessage applied once. */
  const contextAttachFailure = useCallback((target: string, err: unknown) => {
    if (target === COMPOSER_CONTEXT_TARGET) setComposerContextError(ipcMessage(err));
    else failAction(err);
  }, [failAction]);

  /** Draw chips for authorized paths, pictures included. See prompt-attach.ts. */
  const attachGrantedPaths = useCallback(async (target: string, epoch: number,
      granted: readonly GrantedAttachment[]): Promise<void> => {
    const drawn = await attachmentContextItems({
      readThumbnail: path => window.iface.readPromptContextThumbnail(path),
      mintId: rendererItemId,
      leaf: pathLeaf,
    }, granted);
    if (contextRequestEpoch.current !== epoch) return;
    for (const item of drawn) addContextFor(target, item);
  }, [addContextFor]);

  const attachDroppedPaths = useCallback((target: string, paths: readonly PromptDropPath[]) => {
    if (target === COMPOSER_CONTEXT_TARGET) setComposerContextError(null);
    if (paths.length === 0) {
      contextAttachError(target,
        "Those files have no local path. Save them to disk and drop them again.");
      return;
    }
    const requestEpoch = contextRequestEpoch.current;
    void window.iface.grantPromptContextPaths(paths.map(entry => entry.path)).then(
      async result => {
        if (contextRequestEpoch.current !== requestEpoch) return;
        await attachGrantedPaths(target, requestEpoch, result.granted);
        if (result.rejected.length === 0) return;
        contextAttachError(target,
          result.rejected[0]?.message ?? "Those files could not be attached.");
      }, (err: unknown) => {
        if (contextRequestEpoch.current !== requestEpoch) return;
        contextAttachFailure(target, err);
      });
  }, [attachGrantedPaths, contextAttachError, contextAttachFailure]);

  /**
   * A pasted screenshot, which is the only attachment with no path: the bytes
   * go to the main process, which writes a real file and grants it. From the
   * chip down, nothing can tell a paste from a drop — and that is the point,
   * because a path is what the agent can actually open.
   */
  const attachPastedImages = useCallback(async (target: string,
      files: readonly PastedFile[]): Promise<readonly string[]> => {
    if (target === COMPOSER_CONTEXT_TARGET) setComposerContextError(null);
    if (files.length === 0) return [];
    const requestEpoch = contextRequestEpoch.current;
    try {
      const payloads = await pastedImagePayloads(files);
      if (payloads.length === 0) {
        contextAttachError(target, "That pasted image could not be read.");
        return [];
      }
      await retainDraftAttachments();
      const result = await window.iface.savePastedImages(payloads);
      if (contextRequestEpoch.current !== requestEpoch) return [];
      await attachGrantedPaths(target, requestEpoch,
        result.saved.map(saved => ({ ...saved, kind: "file" as const })));
      if (result.rejected.length > 0) {
        contextAttachError(target,
          result.rejected[0]?.message ?? "That image could not be attached.");
      }
      return result.saved.map(saved => saved.path);
    } catch (err: unknown) {
      if (contextRequestEpoch.current !== requestEpoch) return [];
      contextAttachFailure(target, err);
      return [];
    }
  }, [attachGrantedPaths, contextAttachError, contextAttachFailure, retainDraftAttachments]);

  /**
   * The Browser pane's "Add to draft" (point and fix): the note goes after
   * the thread's draft text, as a diff quote does, and the captured PNG
   * becomes an image attachment, as a pasted screenshot does. Nothing is
   * sent; the owner reviews the draft. A chat with no project takes no
   * attachments, so it gets the note alone.
   */
  const addBrowserNote = useCallback((key: string, note: string, png: Uint8Array | null) =>
    addBrowserNoteToDraft({
      getDraft: target => conversationDrafts.get(target),
      setDraft: (target, text) => { conversationDrafts.set(target, text); },
      canAttach: target => {
        const summary = sessionByKeyRef.current.get(target);
        return summary !== undefined && sessionHasWorkspaceContext(summary);
      },
      attachImage: (target, bytes) => attachPastedImages(target, [{ type: "image/png", name: "browser-capture.png",
        arrayBuffer: () => Promise.resolve(new Uint8Array(bytes).buffer) }]),
    }, key, note, png), [attachPastedImages, conversationDrafts]);

  /* Stash (T3 section 9, protocol 63): a thread's drafts parked in Rust. The
     composer's words live in the draft store and its chips in sessionContext;
     a restored draft's paths are granted again, as a recovered prompt's are. */
  const draftStash = useDraftStash({
    api: window.iface,
    getDraft: key => conversationDrafts.get(key),
    setDraft: (key, text) => { conversationDrafts.set(key, text); },
    getChips: key => draftAttachmentRefs.current.sessionContext[key] ?? [],
    dropChips: (key, ids) => {
      setSessionContext(current => {
        const remaining = (current[key] ?? []).filter(item => !ids.has(item.id));
        return remaining.length === 0 ? withoutKey(current, key) : { ...current, [key]: remaining };
      });
    },
    restoreChips: async (key, items) => {
      for (const item of stashOtherItems(items)) addContextFor(key, item);
      const paths = stashPathItems(items);
      if (paths.length === 0) return null;
      const epoch = contextRequestEpoch.current;
      const grants = await window.iface.grantPromptContextPaths(paths.map(item => item.value));
      await attachGrantedPaths(key, epoch, grants.granted);
      return grants.rejected.length === 0 ? null
        : `${grants.rejected.length === 1 ? "An attachment" : "Some attachments"} of the stashed draft could not be attached again: ${grants.rejected[0]?.message ?? "it is missing"}`;
    },
    reportError: message => { setActionError(message); },
  });

  /**
   * A terminal pane's "Send selection to thread…" (T3 section 9): a fenced
   * block headed by the pane's title goes after the thread's draft, as a diff
   * quote does. Nothing is sent; the owner reviews the draft.
   */
  const addTerminalSelection = useCallback((key: SessionKey, title: string, selection: string): boolean => {
    const write = appendTerminalExcerpt(conversationDrafts.get(key), title, selection);
    if (write === null) return false;
    conversationDrafts.set(key, write.text);
    return true;
  }, [conversationDrafts]);

  const removeContextFor = useCallback((target: string, id: string) => {
    if (target === COMPOSER_CONTEXT_TARGET) {
      setComposerContext(current => removePromptContextItem(current, id));
      return;
    }
    setSessionContext(current => ({
      ...current,
      [target]: removePromptContextItem(current[target] ?? [], id),
    }));
  }, []);

  const requestPromptContext = useCallback((target: string, kind: PromptContextKind,
      cwd: string) => {
    if (target === COMPOSER_CONTEXT_TARGET) setComposerContextError(null);
    if (kind === "file") {
      openContextQuickOpen(cwd, target);
      return;
    }
    if (kind === "diff") {
      if (cwd.trim() === "") return;
      const requestEpoch = contextRequestEpoch.current;
      window.iface.readRepositoryReview(cwd).then(result => {
        if (contextRequestEpoch.current !== requestEpoch) return;
        const safeContent = repositoryReviewPromptContent(result);
        if (safeContent === null) {
          throw new Error(result.error?.message
            ?? "The selected folder is not a Git repository with a review snapshot.");
        }
        addContextFor(target, {
          id: rendererItemId("diff"),
          kind: "diff",
          label: "Reviewed changes",
          value: result.root,
          detail: `Credential-filtered Review snapshot for ${result.root}`,
          safeContent,
          estimatedTokens: Math.ceil(safeContent.length / 4),
        });
      }).catch((err: unknown) => {
        if (contextRequestEpoch.current !== requestEpoch) return;
        contextAttachFailure(target, err);
      });
      return;
    }
    if (kind === "url") {
      setContextValueTarget(target);
      return;
    }
    if (kind === "chat") {
      // T3 section 9: the project's other threads; Rust checks again at send.
      if (target !== COMPOSER_CONTEXT_TARGET) setThreadReferenceTarget(target);
      return;
    }
    if (kind === "folder") {
      const requestEpoch = contextRequestEpoch.current;
      window.iface.pickDirectory(cwd || undefined).then(path => {
        if (path === null || contextRequestEpoch.current !== requestEpoch) return;
        addContextFor(target, {
          id: rendererItemId("folder"),
          kind: "folder",
          label: pathLeaf(path),
          value: path,
          detail: path,
        });
      }).catch((err: unknown) => {
        if (contextRequestEpoch.current !== requestEpoch) return;
        contextAttachFailure(target, err);
      });
    }
  }, [addContextFor, contextAttachFailure, openContextQuickOpen]);

  const openQuickOpenFile = useCallback((file: ProjectFileEntry) => {
    const target = quickOpen.contextTarget;
    if (quickOpen.root === null || target === null) return;
    if (!quickOpen.files.some(entry => entry.path === file.path)) return;
    addContextFor(target, {
      id: rendererItemId("file"),
      kind: "file",
      label: file.relativePath,
      value: file.path,
      detail: file.relativePath,
    });
    closeQuickOpen();
  }, [addContextFor, closeQuickOpen, quickOpen.contextTarget, quickOpen.files, quickOpen.root]);

  // A result belongs to the cwd that was indexed. If selection changes through
  // an external registry update while the modal is open, close and invalidate
  // the in-flight request rather than letting a late file cross workspaces.
  useEffect(() => {
    if (!quickOpen.open) return;
    if (quickOpen.contextTarget === COMPOSER_CONTEXT_TARGET && state.composerOpen) return;
    const target = quickOpen.contextTarget === null
      ? undefined : sessionByKey.get(quickOpen.contextTarget);
    if (target?.cwd === quickOpen.root) return;
    closeQuickOpen();
  }, [closeQuickOpen, quickOpen.contextTarget, quickOpen.open,
    quickOpen.root, sessionByKey, state.composerOpen]);

  // Default Cursor-style queue policy (OutboundLane.autoDeliver): after the
  // provider reports the current turn idle, submit exactly one item and wait
  // for that turn's terminal edge before considering the next.
  useEffect(() => {
    outbound.autoDeliver(steeringQueues, queueErrors, key => sessionByKey.get(key)?.status);
  }, [configWake, liveVersion, outbound, outboundSnapshot.wake, queueErrors, sessionByKey,
    steeringQueues]);

  const consumeSessionContext = useCallback((key: SessionKey,
      consumed: readonly PromptContextItem[]) => {
    const ids = new Set(consumed.map(item => item.id));
    setSessionContext(current => {
      const remaining = (current[key] ?? []).filter(item => !ids.has(item.id));
      return remaining.length === 0
        ? withoutKey(current, key)
        : { ...current, [key]: remaining };
    });
  }, []);

  const restoreSessionContext = useCallback((key: SessionKey,
      restored: readonly PromptContextItem[]) => {
    if (restored.length === 0) return;
    const retained = draftAttachmentRefs.current;
    draftAttachmentRefs.current = { ...retained, sessionContext: { ...retained.sessionContext,
      [key]: normalizePromptContextItems([...restored, ...(retained.sessionContext[key] ?? [])]) } };
    setSessionContext(current => ({
      ...current,
      // Captured next-prompt context wins the bounded merge; items attached
      // after the send started are retained whenever capacity permits.
      [key]: normalizePromptContextItems([...restored, ...(current[key] ?? [])]),
    }));
  }, []);

  // The outbound lane's App callbacks (see `outboundApp` above).
  outboundApp.current = useCommittedActions<OutboundAppHooks>({
    validateContext: reauthorizePromptContext,
    // After restart the picker can show a remembered access mode before a
    // handle exists. Apply it on this first send so the session does not
    // resume at the provider default while the footer still says otherwise.
    restoreAccessMode: key => {
      const summary = state.sessions.find(session =>
        sessionKey(session.engine, session.sessionId) === key);
      if (summary === undefined) return undefined;
      const model = summary.model || latestModelPresetFor(modelPresets, summary.engine)?.model;
      // Protocol 68: never for a session the host runs in a mode of its own.
      const mode = modeToRestore(summary, {
        live: liveKeys.current.has(key),
        chosen: permissionMode[key],
        launched: launchPermission[key],
        saved: model !== undefined && model !== "" && model !== "default"
          ? modelPresetFor(modelPresets, summary.engine, model)?.permissionMode : undefined,
      });
      if (mode === undefined) return undefined;
      return window.iface.setPermissionMode(key, mode).then(() => {
        closedKeys.current.delete(key);
        markLive(key);
      });
    },
    publishDraft: key => {
      publishCollaborationDraftState(key, conversationDrafts.get(key));
    },
    checkpoint: key => { checkpointTurn(key, null); },
    markDelivered: key => {
      closedKeys.current.delete(key);
      markLive(key);
    },
    restoreContext: restoreSessionContext,
  });

  /* ANSWERING AN APPROVAL — the only two functions that may. Both surfaces call
     these, so both read the same verdict back out of `verdicts` and cannot tell
     different stories about one request.

     The verdict is recorded BEFORE `respond` is called: the click has to settle
     the card in the same commit, or a slow round-trip leaves Allow/Deny live
     and the user answers twice. A rejection then adds the reason to that same
     entry, which is what puts the controls back on both surfaces — the request
     is demonstrably still outstanding, and a console warning is not something
     the packaged app's user can see. */
  const answerApproval = useCallback(
    (key: SessionKey, approvalId: string, verdict: "allow" | "deny", note?: string,
      answers?: Record<string, string>) => {
      const id = approvalVerdictKey(key, approvalId);
      setVerdicts(m => ({ ...m, [id]: { verdict } }));
      window.iface.respond(key, approvalId, verdict, note, answers)
        .catch((err: unknown) => {
          setVerdicts(m => ({ ...m, [id]: { verdict, error: ipcMessage(err) } }));
        });
    }, []);

  const approveFor = useCallback((key: SessionKey, approvalId: string,
      answers?: Record<string, string>) => {
    answerApproval(key, approvalId, "allow", undefined, answers);
  }, [answerApproval]);

  const denyFor = useCallback((key: SessionKey, approvalId: string, note?: string) => {
    answerApproval(key, approvalId, "deny", note);
  }, [answerApproval]);

  const showCanvas = useCallback(() => {
    if (launchingRef.current) return;
    showSurface("code");
    dismissComposer(false);
    setCenterView("canvas");
  }, [dismissComposer, showSurface]);

  const openWorkspaceTool = useCallback((tool: WorkspaceTool, cwd: string): void => {
    void window.iface.openWorkspaceTool({ version: 1, root: cwd, tool })
      .catch((error: unknown) => { setActionError(ipcMessage(error)); });
  }, []);

  const showTerminals = useCallback(() => {
    if (launchingRef.current) return;
    showSurface("code");
    dismissComposer(false);
    setCenterView("terminal");
  }, [dismissComposer, showSurface]);

  /* Several accounts per provider (protocol 60): read on mount and whenever
     Settings → Providers is shown; each change answers the whole view. */
  const providerAccounts = useProviderAccounts(window.iface);
  const accountsView = providerAccounts.state.view;

  /**
   * Sign in (Settings, or a turn that failed signed out): a Code terminal in
   * the default folder running the provider CLI's own login, which Rust
   * picks. Refresh afterwards reads the new state.
   */
  const openSignIn = useCallback((engine: EngineId) => {
    const profile = signInProfile(engine);
    const label = engineLabels[engine] ?? providerLabel(engine);
    if (profile === null) {
      setActionError(`Run ${label} in Terminal and sign in, then press Refresh in Models & providers.`);
      return;
    }
    const cwd = approvedDefaultDirectory;
    if (cwd === null) {
      setActionError(`${productLabel()} is still finding its default folder. Try Sign in again in a moment.`);
      return;
    }
    setSettingsOpen(false);
    showTerminals();
    terminalCenter.current?.openSignIn(profile, label, cwd);
  }, [approvedDefaultDirectory, engineLabels, showTerminals]);

  /** Sign in to one account (Settings → Providers' accounts): its CLI's login, with its directory. */
  const openAccountSignIn = useCallback((engine: string, account: string) => {
    const profile = signInProfile(engine as EngineId);
    const label = engineLabels[engine] ?? providerLabel(engine);
    const cwd = approvedDefaultDirectory;
    if (profile === null || cwd === null) {
      setActionError(profile === null ? `${label} has no sign-in ${productLabel()} can open.`
        : `${productLabel()} is still finding its default folder. Try Sign in again in a moment.`);
      return;
    }
    const group = accountGroupFor(providerAccounts.viewRef.current, engine);
    const named = group !== undefined && group.accounts.length > 1
      ? group.accounts.find(candidate => candidate.id === account)?.label ?? null : null;
    setSettingsOpen(false);
    showTerminals();
    terminalCenter.current?.openSignIn(profile, label, cwd, account, accountSignInTitle(label, named));
  }, [approvedDefaultDirectory, engineLabels, providerAccounts.viewRef, showTerminals]);

  const activateTerminalNotification = useCallback((terminalId: string) => {
    if (launchingRef.current) return;
    showTerminals();
    terminalCenter.current?.focusTerminal(terminalId);
  }, [showTerminals]);

  /**
   * A switched thread (protocol 59) runs on `to` now: the pane that showed it,
   * its half-written draft and its attachments go with it.
   */
  const carryThread = useCallback((from: SessionKey, to: SessionKey, launched: string | undefined,
      switchedAt: number) => {
    const draft = conversationDrafts.get(from);
    if (draft !== "") {
      conversationDrafts.set(to, draft);
      conversationDrafts.set(from, "");
    }
    setSessionContext(current => carryKeyed(current, from, to));
    setSessionContextOpen(current => carryKeyed(current, from, to));
    setWorkspace(current => replaceWorkspaceKey(current, from, to));
    // An ACP session started with its handoff; no send echoed it.
    if (launched !== undefined) appendConversationEcho(to, launchPendingEcho(launched, []));
    // The old session is an earlier part of the thread now; nothing of it
    // stays in this window, so a switch back starts from its history.
    dropSessionClientState(from);
    // The new head is live before its pane opens, which shuts its history
    // fence: the thread's history from before the switch is read once here
    // and drawn before the live stream, which carries the head's own.
    void window.iface.readHistory(to, { limit: HISTORY_LIMIT }).then(page => {
      if (page.availability === "unavailable") return;
      const events = switchSeed(page.events, switchedAt, parseKey(to).engine);
      if (events.length === 0) return;
      const items = historyPageItems(events, undefined);
      historyPages.current[to] = { events, items };
      setHistoryByKey(map => ({
        ...map,
        [to]: {
          status: "ready", items, plan: latestPlanEntries(events),
          olderMessages: page.olderMessages, truncated: page.olderMessages > 0, canLoadOlder: false,
        },
      }));
    }).catch((err: unknown) => {
      console.warn("[threads] a switched thread's earlier conversation was not read:", err);
    });
  }, [appendConversationEcho, conversationDrafts, dropSessionClientState]);
  // Threads ↔ Code ↔ Agent, Second opinion and Switch to (use-thread-bridges.ts).
  const {
    openThreadInTerminal, continueAsThread, providerFacts, threadOffersFor, askSecondOpinion,
    switchThreadProvider, continueThreadOnAccount, opinionPending, opinionAction, threadOpinions,
    threadBridges,
  } = useThreadBridges({
    state, threads, threadRequest, agentStore,
    launchingRef, sessionByKeyRef, closedKeys, terminalCenter,
    setActionError, failAction, refreshAll, showTerminals, showSurface, selectSession, selectAgent,
    markLive, carryThread, startAgentTask, engineLabels, modelPresets, choicesFor,
    accountsRef: providerAccounts.viewRef, launch,
    collaborations, noteCollaboration, approveCollaboration, denyCollaboration,
    acceptCollaborationPeer, discardCollaborationPeer, cancelCollaboration,
  });
  const recordTerminalCompletion = useCallback((event: Omit<AttentionRow, "live">) => {
    setAttention(current => recordEvent(current, event));
    persistAttention(event);
  }, [persistAttention]);
  useTerminalNotifications({
    preferences: notificationSettings.current,
    terminals: terminalSummaries,
    visible: surface === "code" && centerView === "terminal" && !state.composerOpen && !settingsOpen,
    focusedTerminalId,
    workspaces: terminalSwitcher?.records,
    onEvent: recordTerminalCompletion,
    onActivate: activateTerminalNotification,
  });

  const selectTerminalWorkspace = useCallback((workspaceId: string): void => {
    if (launchingRef.current) return;
    terminalSwitcher?.onSelect(workspaceId);
    showTerminals();
    if (mobileLayout) closeMobileSidebar();
  }, [closeMobileSidebar, mobileLayout, showTerminals, terminalSwitcher]);

  const startCodeWorkspace = useCallback((projectPath?: string) => {
    if (launchingRef.current) return;
    showTerminals();
    terminalSwitcher?.onCreate(projectPath);
    if (mobileLayout) {
      composerOpenerRef.current = sidebarToggleRef.current;
      setSidebarOpen(false);
    }
  }, [mobileLayout, showTerminals, terminalSwitcher]);

  const sidebarTerminalWorkspaces = useMemo(() => (
    terminalSwitcher === null ? null : {
      records: terminalSwitcher.records,
      activeId: terminalSwitcher.activeId,
      onSelect: selectTerminalWorkspace,
      onCreate: () => { startCodeWorkspace(); },
      onRename: terminalSwitcher.onRename,
      onDelete: terminalSwitcher.onDelete,
      busy: terminalSwitcher.busy,
      canCreate: terminalSwitcher.canCreate,
    }
  ), [selectTerminalWorkspace, startCodeWorkspace, terminalSwitcher]);

  // Opening a node is ordinary navigation on the session's own surface (Chat
  // or Agent; the canvas lives in Code, which holds no conversations): it
  // replaces the focused pane there. Shift asks for another pane and obeys the
  // four-pane ceiling, exactly like openSessionInNewPane.
  const openCanvasSession = useCallback((key: SessionKey, newPane: boolean) => {
    if (launchingRef.current) return;
    const owner = conversationSurfaceFor(sessionByKeyRef.current.get(key), surfaceRef.current);
    // The four-pane ceiling is checked before anything moves, so a Shift+Enter
    // that cannot open a pane leaves the selection and the center view alone.
    if (newPane && !mayOpenWorkspaceKey(key, owner)) return;
    setActionError(null);
    showSurface(owner);
    dispatch({ type: "select", key });
    if (newPane) {
      setWorkspaceFor(owner, current => openWorkspaceKey(current, key));
    } else {
      setWorkspaceFor(owner, current => switchWorkspaceKey(current, key));
    }
    showSessionWorkspace();
    dispatch({ type: "composer", open: false });
  }, [mayOpenWorkspaceKey, setWorkspaceFor, showSessionWorkspace, showSurface]);

  /**
   * Where the attention pill goes: the first agent that is blocked on the user.
   *
   * It used to open the board, on a section holding every agent the pill had
   * counted. With the board gone, the smallest honest destination is the
   * session at the head of THE attention selection — the same array the pill
   * counted and the dock badge counts, so the pill can never land on an agent
   * it was not counting.
   *
   * `setQuery("")` stays: the search box is the one filter that drops rows the
   * user has not archived, and arriving mid-search would leave the sidebar
   * unable to show the agent this click just selected.
   */
  /* ---- the attention tray --------------------------------------------------
     THE live needs, from the same array the badge counts and the sidebar pins
     (attentionListing), plus the panes ringing for input — the one place the
     terminals join the count. Each row names where it is: the project for a
     chat, the workspace for a pane. */
  const attentionNeeds = useMemo<LiveNeed[]>(() => [
    ...attentionListing.map(session => {
      const key = sessionKey(session.engine, session.sessionId);
      return {
        id: `session:${key}`, target: { kind: "session" as const, key },
        title: session.title, where: attentionPlace(session.cwd),
      };
    }),
    ...terminalSummaries
      .filter(terminal => terminal.activity === "waiting"
        && (terminal.state === "running" || terminal.state === "starting"))
      .map(terminal => ({
        id: `terminal:${terminal.id}:${terminal.generation}`,
        target: { kind: "terminal" as const, terminalId: terminal.id, generation: terminal.generation },
        title: terminal.title || attentionPlace(terminal.cwd),
        where: terminalSwitcher?.records.find(record =>
          record.terminalIds.includes(terminal.id))?.name ?? attentionPlace(terminal.cwd),
      })),
  ], [attentionListing, terminalSummaries, terminalSwitcher]);
  // trackNeeds returns the same state when nothing changed, so this settles.
  useEffect(() => {
    setAttention(current => trackNeeds(current, attentionNeeds, Date.now()));
  }, [attentionNeeds]);
  const attentionRowList = useMemo(
    () => attentionRows(attention, attentionNeeds), [attention, attentionNeeds]);
  const attentionUnread = attentionUnreadCount(harnessView, attentionRowList, attention.openedAt);
  const openAttention = useCallback((open: boolean) => {
    setAttentionOpen(open);
    if (open) {
      setAttention(current => markOpened(current, Date.now()));
      void harnessRequest({ kind: "markSeen", ids: attentionRowList.map(row => row.id) }).catch(() => undefined);
    }
  }, [attentionRowList, harnessRequest]);
  // Navigation acknowledges viewing; resolving an event is a separate action.
  const activateAttention = useCallback((row: AttentionRow) => {
    if (!row.live) void harnessRequest({ kind: "markSeen", ids: [row.id] }).catch(() => undefined);
    if (row.target.kind === "session") {
      setQuery("");
      selectSession(row.target.key);
      return;
    }
    const terminalId = row.target.terminalId;
    const terminal = terminalSummaries.find(item => item.id === terminalId);
    if (!terminal || row.target.generation === undefined || terminal.generation !== row.target.generation) {
      setActionError("This notification belongs to an earlier terminal process. Its history is preserved; the replacement was not selected."); return;
    }
    showTerminals();
    terminalCenter.current?.focusTerminal(terminalId);
  }, [selectSession, showTerminals, terminalSummaries, harnessRequest]);
  const clearAttention = useCallback(() => {
    // Without a harness read there is nothing durable to resolve, but the tray
    // still shows the events recorded here; Clear all must clear those. The
    // Rust store stays canonical and the next successful read replaces them.
    if (harnessView === null) { setAttention(clearEvents); return; }
    for (const item of harnessView.state.attention) {
      if (!item.resolved && item.kind !== "needs-input") void harnessRequest({ kind: "resolve", id: item.id, target: item.target }).catch(() => undefined);
    }
  }, [harnessView, harnessRequest]);

  const dismissNotice = useCallback((at: number) => {
    setNotices(list => list.filter((_, i) => i !== at));
    focusAfterNoticeDismissal();
  }, [focusAfterNoticeDismissal]);
  const dismissKeptWorktree = useCallback((key: SessionKey) => {
    setKeptWorktrees(list => list.filter(notice => notice.key !== key));
    focusAfterNoticeDismissal();
  }, [focusAfterNoticeDismissal]);
  const dismissStoreNotice = useCallback((id: string) => {
    setStoreNotices(list => list.filter(notice => notice.id !== id));
    focusAfterNoticeDismissal();
    // Rust forgets it too, so a reloaded window does not show it again.
    window.iface.diagnostics({ kind: "dismissNotice", id })
      .then(response => { setStoreNotices(noticesOf(response)); })
      .catch(() => undefined);
  }, [focusAfterNoticeDismissal]);
  const revealStoreNotice = useCallback((id: string) => {
    window.iface.diagnostics({ kind: "reveal", target: "notice", id }).catch(failAction);
  }, [failAction]);
  // Settings → Reveal data folder: Rust shows it in Finder.
  const revealDataFolder = useCallback(() => {
    window.iface.diagnostics({ kind: "reveal", target: "dataFolder" }).catch(failAction);
  }, [failAction]);
  useEffect(() => {
    let live = true;
    window.iface.diagnostics({ kind: "notices" })
      .then(response => { if (live) setStoreNotices(noticesOf(response)); })
      .catch(() => undefined);
    return () => { live = false; };
  }, []);

  /* ---- what the sidebar's row menu may offer ------------------------------
     One object rather than eight props, and stable, because it is a bundle of
     one feature. `can` closes over the feature-detected map: every action is
     hidden for an engine whose adapter does not implement it. */
  const canDo = useCallback(
    (engine: string, op: Parameters<typeof engineCan>[2]) => engineCan(engineOps, engine, op),
    [engineOps],
  );

  const lifecycle = useMemo<SidebarLifecycle>(() => ({
    can: canDo,
    liveKeys: liveKeys.current,
    onRename: renameSession,
    onDelete: deleteSession,
    onFork: forkSession,
    onEngineArchive: engineArchive,
    onAdopt: adoptSession,
    onStop: stopSession,
    error: actionError,
    onDismissError: dismissActionError,
  }), [canDo, renameSession, deleteSession, forkSession, engineArchive, adoptSession,
    stopSession, actionError, dismissActionError, liveVersion]);

  const importer = useMemo<SidebarImporter>(() => ({
    onImportAll: importAll,
    running: importing,
    done: importProgress?.done ?? 0,
    total: importProgress?.total ?? 0,
    adopt: importProgress?.adopt ?? true,
    note: importNote,
    onDismissNote: dismissImportNote,
  }), [importAll, importing, importProgress, importNote, dismissImportNote]);

  // The Threads inbox: the listing the session sidebar would show before its
  // search (the inbox searches project names too), joined to Rust's thread
  // view. Its chords act only while the Threads surface shows uncovered.
  // A thread Rust records is never hidden as a small transcript: pinned,
  // snoozed and unread threads must survive a restart (threadListingRevealed).
  // Sessions a Code pane or the CLI started are listed too, under From
  // terminals (review C1): the origin filter keeps them out of the session
  // sidebar, not out of the inbox, which folds them away by default.
  const threadListing = useMemo(() => surface !== "chat" ? [] : buildSidebarView(
    sidebarSessions, selectedKey, threadListingRevealed(revealed, sidebarSessions, threads.view),
    hideTrivial, false).visible,
  [hideTrivial, revealed, selectedKey, sidebarSessions, surface, threads.view]);
  const threadNeedsYou = useMemo(() => new Set(attentionListing.map(session =>
    sessionKey(session.engine, session.sessionId))), [attentionListing]);
  const unhideSession = useCallback((key: SessionKey) => {
    setActionError(null);
    void window.iface.archive(key, false).then(() => refreshAll()).catch(failAction);
  }, [failAction, refreshAll]);
  const threadsUncovered = surface === "chat" && !state.composerOpen && !settingsOpen
    && !commandPaletteOpen && !sessionLibraryOpen;
  const threadInbox = useThreadInbox({
    threads, sessions: threadListing, query, selectedKey, needsYou: threadNeedsYou,
    watchingKey: threadsUncovered && centerView === "sessions" ? workspace.focusedKey : null,
    shortcuts: threadsUncovered, onError: failAction, onUnarchive: unhideSession,
    searchMessages: searchThreadMessages,
  });

  // Memoized like every other Sidebar prop: that pane is memo()'d, and a fresh
  // element here on each render would re-draw the whole listing behind it.
  const agentRosterTasks = useMemo(() => agentLatestTasks(agentStore.assignments, state.sessions),
    [agentStore.assignments, state.sessions]);
  // Each current thread's latest line: what it last said or did, else its
  // title. Kept by identity while no line changes, so the roster does not
  // redraw on every streamed token of a thread whose line stayed put.
  const agentThreadPreviewsRef = useRef<Readonly<Record<SessionKey, string>>>({});
  const agentThreadPreviews = useMemo(() => {
    const next: Record<SessionKey, string> = {};
    for (const thread of Object.values(agentStore.threads ?? {})) {
      const key = thread.current?.sessionKey;
      if (key === undefined) continue;
      const line = sidebarPeeks[key]?.activity || sessionByKey.get(key)?.title;
      if (line) next[key] = line;
    }
    if (!equalValue(agentThreadPreviewsRef.current, next)) agentThreadPreviewsRef.current = next;
    return agentThreadPreviewsRef.current;
  }, [agentStore.threads, sessionByKey, sidebarPeeks]);
  const agentMenu = useMemo(() => surface !== "agent" ? undefined : (
    <AgentMenu
      agents={agentStore.agents}
      selectedId={activeAgent?.id ?? null}
      latestTasks={agentRosterTasks}
      statuses={agentStore.statuses}
      threads={agentStore.threads}
      threadPreviews={agentThreadPreviews}
      now={now}
      onSelect={selectAgent}
      onCreate={openNewAgentDialog}
      createDisabled={!agentReady}
    />
  ), [activeAgent, agentReady, agentStore.agents, agentStore.statuses, agentStore.threads, agentRosterTasks,
    agentThreadPreviews, now, openNewAgentDialog, selectAgent, surface]);

  const persistPreferences = useCallback((next: AppPreferences) => {
    applyAppearancePreferences(next);
    setAppPreferences(next);
    setPreferencesError(writeAppPreferences(localStorage, next)
      ? null
      : "This change is active for this window but could not be saved on this Mac.");
  }, []);

  const changeAppPreferences = useCallback((next: AppPreferences) => {
    const previous = appPreferences;
    if (next.uiChrome === previous.uiChrome) {
      persistPreferences(next);
      return;
    }
    const paired = pairThemeWithChrome(
      next.uiChrome,
      appearanceRef.current.preference,
      previous.lastCliTheme,
      previous.lastWorkspaceTheme,
    );
    persistPreferences({
      ...next,
      lastCliTheme: paired.lastCliTheme,
      lastWorkspaceTheme: paired.lastWorkspaceTheme,
    });
    if (paired.theme !== appearanceRef.current.preference) {
      setAppearancePreference(paired.theme);
    }
  }, [appPreferences, persistPreferences, setAppearancePreference]);

  const changeAppearancePreference = useCallback((preference: ThemeId) => {
    setAppearancePreference(preference);
    persistPreferences({
      ...appPreferences,
      ...(appPreferences.uiChrome === "cli" && isCliThemePreference(preference)
        ? { lastCliTheme: preference }
        : { lastWorkspaceTheme: preference }),
    });
  }, [appPreferences, persistPreferences, setAppearancePreference]);

  const rememberLaunchModelPreset = useCallback((preset: ModelPreset) => {
    setModelPresets(current => rememberModelPreset(current, preset));
  }, []);

  const closeSettings = useCallback(() => {
    applySettingsChromeLock(false);
    setSettingsOpen(false);
    window.requestAnimationFrame(() => {
      const opener = settingsOpenerRef.current;
      if (opener?.isConnected && opener.getClientRects().length > 0) opener.focus();
      else if (commandPaletteButtonRef.current?.isConnected) {
        commandPaletteButtonRef.current.focus();
      } else sidebarToggleRef.current?.focus();
    });
  }, []);

  const openSettingsAt = useCallback((section?: SettingsSectionId) => {
    if (launchingRef.current || settingsOpen) return;
    settingsOpenerRef.current = document.activeElement instanceof HTMLElement
      ? document.activeElement : null;
    // Settings is a navigation surface, not a destructive composer dismissal.
    // Close transient popovers but retain the launch form, context chips,
    // conversation drafts, editor models, and queue state mounted underneath.
    closeQuickOpen();
    window.dispatchEvent(new Event(APP_SELECT_DISMISS_EVENT));
    setComposerContextOpen(false);
    setSessionContextOpen({});
    if (mobileLayout) setSidebarOpen(false);
    applySettingsChromeLock(true);
    setSettingsSection(section);
    setSettingsOpen(true);
  }, [closeQuickOpen, mobileLayout, settingsOpen]);
  const openSettings = useCallback(() => { openSettingsAt(); }, [openSettingsAt]);
  // ⌘W (and the Window menu's Close Pane): close what is in front. In Code
  // that is the selected pane's active tab, exactly as its × closes it.
  const closeSelectedPane = useCallback(() => {
    const target = closeChordTarget({
      foregroundDialog: commandPaletteOpen || sessionLibraryOpen
        || document.querySelector(FOREGROUND_DIALOG_SELECTOR) !== null,
      settingsOpen, composerOpen: state.composerOpen,
      terminalsShown: surface === "code" && centerView === "terminal",
      sessionsShown: centerView === "sessions",
      hasFocusedTile: workspace.focusedKey !== null,
    });
    if (target === "settings") { if (!launchingRef.current) closeSettings(); }
    else if (target === "terminal-pane") terminalCenter.current?.closeFocusedPane();
    else if (target === "tile" && workspace.focusedKey !== null) closeWorkspaceTile(workspace.focusedKey);
  }, [centerView, closeSettings, closeWorkspaceTile, commandPaletteOpen, sessionLibraryOpen,
    settingsOpen, state.composerOpen, surface, workspace.focusedKey]);
  // The application menu (main/app-menu.ts): Settings…, Close Pane and what a
  // Help item reports.
  useEffect(() => window.iface.onMenuCommand(raw => {
    const command = parseMenuCommand(raw);
    if (command === null) return;
    if (command.command === "settings") openSettings();
    else if (command.command === "close-pane") closeSelectedPane();
    else setActionError(command.text);
  }), [closeSelectedPane, openSettings]);
  // What main says about Browser pages (main/browser-pane.ts): a link that
  // wants a new tab, ⌘F / ⌘L pressed in a page, a download's progress.
  useEffect(() => window.iface.onBrowserEvent(raw => {
    const event = parseBrowserEvent(raw);
    if (event !== null) terminalCenter.current?.browserEvent(event);
  }), []);
  // The sidebar's provider alert lands where the fix is, not on Appearance.
  const openProviderSettings = useCallback(() => { openSettingsAt("providers"); }, [openSettingsAt]);

  // One fresh-start primitive for the global New Chat surfaces and each
  // project header. A project path is a one-draft seed, not a last-used write;
  // LAST_CWD changes only after createSession succeeds.
  const openFreshComposer = useCallback((projectPath: string | null, prompt = "") => {
    if (launchingRef.current) return;
    // Every entry into the sheet starts clean. dismissComposer is not the only
    // way out of it — selectSession, openSessionInNewPane and focusWorkspaceTile
    // all close it with a bare `composer: false` — so an abandoned split is
    // cleared HERE, where every route back in must pass, rather than chased
    // through each of the exits.
    pendingNewPane.current = false;
    composerOpenerRef.current = document.activeElement instanceof HTMLElement
      ? document.activeElement : null;
    setLaunchError(null);
    resetComposerContext();
    const cwd = projectPath ?? readStored(LAST_CWD) ?? composerDirectory.path;
    setComposerSeed(current => ({
      revision: current.revision + 1, cwd: projectPath, prompt,
    }));
    setComposerContextRoot(cwd);
    dispatch({ type: "composer", open: true });
  }, [composerDirectory.path, resetComposerContext]);

  // A pane's split button. It lives here, below openFreshComposer, because it
  // calls it: there is no "create a session in project X" primitive to call
  // instead — buildLaunchOptions refuses a launch with no prompt — so a split
  // is the ordinary seeded New Chat sheet plus a mark on the launch it produces.
  // True when the sheet opened.
  const splitWorkspaceTile = useCallback((projectPath: string | null): boolean => {
    if (launchingRef.current) return false;
    // Honor the orchestration reservation the same way every other append does.
    // There is no session key yet, so the sentinel cannot already be open.
    if (!mayOpenWorkspaceKeyWithReservation(
        layoutFor(workspacesRef.current, surfaceRef.current), "pending-split",
        surfaceRef.current === "chat" ? workspacePaneReservation.current : 0)) return false;
    // The mark goes on AFTER the sheet opens: openFreshComposer clears the flag
    // as its first act, so setting it first would erase the split itself.
    openFreshComposer(projectPath);
    pendingNewPane.current = true;
    return true;
  }, [openFreshComposer]);

  const openComposer = useCallback(() => {
    openFreshComposer(null);
  }, [openFreshComposer]);

  /* ---- Threads: the New thread card, a thread pane's turn controls and its
     diff panel. Rust owns every decision (`threads.view`); these hooks send
     its commands and hand the components what to draw. */
  const threadIndex = useThreadIndex(threads.view);
  /** Each thread's "Started threads" line (team tools, protocol 57). */
  const startedThreads = useMemo(() => startedThreadsByParent(threads.view,
    key => sessionByKey.get(key)), [threads.view, sessionByKey]);
  /** Scheduled prompts and pull-request watch (protocol 62). */
  const scheduleTitle = useCallback((key: string) => sessionByKey.get(key)?.title ?? key, [sessionByKey]);
  const schedules = useThreadSchedules(threads.view, threadRequest, scheduleTitle);
  // Protocol 67: the private secret card's channel; a value is only an argument to Give.
  const threadSecrets = useThreadSecrets(useCallback(
    (command: ThreadSecretCommand) => window.iface.threadSecret(command), []));
  const threadIndexRef = useRef(threadIndex);
  threadIndexRef.current = threadIndex;
  /** A thread's provider hit its limit: the offer to continue elsewhere (review E). */
  const limitOffersRef = useRef<Readonly<Record<SessionKey, ThreadLimitOffer>>>({});
  const threadLimitOffers = useMemo(() => {
    const next: Record<SessionKey, ThreadLimitOffer> = {};
    const notices = threadLimitNotices(rateLimits, threads.view?.threads ?? [], Date.now());
    for (const [key, limit] of Object.entries(notices)) {
      const thread = threadIndex.get(key);
      if (thread === undefined || !isProjectThread(thread)) continue;
      const engine = parseKey(key).engine;
      const offer = threadLimitOffer(limit, engine, threadProviderOffers(engine, providerFacts, true),
        headrooms, Date.now(), { view: accountsView,
          ...(thread.account === undefined ? {} : { account: thread.account }) }, thread.limit?.resetsAt);
      if (offer === null) continue;
      const held = limitOffersRef.current[key];
      next[key] = held !== undefined && equalValue(held, offer) ? held : offer;
    }
    limitOffersRef.current = next;
    return next;
  }, [accountsView, headrooms, providerFacts, rateLimits, threadIndex, threads.view]);
  /** Resume at reset and Snooze until reset, once Rust knows when a thread's limit resets (protocol 66). */
  const limitResetsRef = useRef<Readonly<Record<SessionKey, ThreadLimitReset>>>({});
  const threadLimitResets = useMemo(() => {
    const next: Record<SessionKey, ThreadLimitReset> = {};
    for (const thread of threads.view?.threads ?? []) {
      if (thread.limit === undefined) continue;
      const reset = threadLimitReset(thread.limit, parseKey(thread.key).engine, thread.schedules,
        thread.snoozedUntil, Date.now());
      if (reset === null) continue;
      const held = limitResetsRef.current[thread.key];
      next[thread.key] = held !== undefined && equalValue(held, reset) ? held : reset;
    }
    limitResetsRef.current = next;
    return next;
  }, [threads.view]);

  // Project Away's panel, project, provider check and loop threads (use-away-controller.ts).
  const {
    awayOpen, setAwayOpen, setAwayOpenRequest, awayOpenRequest,
    awayFocusedProject, awayProviderReady, openAwayThread, awayThreadKeys,
  } = useAwayController({
    surface, surfaceSession, surfaceSessionKey, threads, threadIndex, engines,
    sessionByKey, showSurface, selectSession,
  });
  // An Away thread's cards: stop its loop, open a review's session, or open
  // the thread's changes to review and ship them.
  const awayThread = useCallback((key: SessionKey, action: AwayCardAction) => {
    if (action.kind === "stop") {
      void window.iface.projectAway({ version: 1, command: { kind: "stop", runId: action.runId } })
        .catch(failAction);
    } else if (action.kind === "session") {
      selectSession(action.key);
    } else {
      const thread = threadIndexRef.current.get(key);
      if (thread === undefined) return;
      setSubagentDetail(null);
      threadDiffRef.current.open(key, defaultThreadDiffChoice(thread));
    }
  }, [failAction, selectSession]);
  // The quit guard's count: main asks before a quit would end this work.
  // It also keeps the Mac awake while any of it is working (main/keep-awake.ts).
  const work = useMemo(() => workInFlight(terminalSummaries, state.sessions,
    awaySessionKeyList, awayRunning, agentStore.runs),
  [terminalSummaries, state.sessions, awaySessionKeyList, awayRunning, agentStore.runs]);
  // Keyed on the counts, not the object, so activity ticks send nothing new.
  // The Dock badge counts waiting terminal panes too (review D5); main adds
  // them to the registry sessions waiting.
  const waitingPanes = useMemo(() => dockAttention(terminalSummaries, state.sessions).waitingPanes,
    [state.sessions, terminalSummaries]);
  useEffect(() => { window.iface.reportDockAttention?.({ version: 1, waitingPanes }); },
    [waitingPanes]);
  useEffect(() => { window.iface.reportWorkInFlight?.(work); },
    [work.terminals, work.threads, work.away, work.runs, work.waiting]);
  const launchRef = useRef(launch);
  launchRef.current = launch;
  // The card holds the one launch lock for its whole prepare → launch → bind.
  const [threadLaunchPort] = useState(() => ({
    // One question at a time: a newer one closes the older as Escape would.
    askFolderTrust: (folder: string): Promise<FolderTrustChoice> => new Promise(resolve => {
      folderTrustAnswer.current?.("dismiss");
      folderTrustAnswer.current = resolve;
      setFolderTrustQuestion(folder);
    }),
    captureContext: (items: readonly PromptContextItem[]) => outbound.captureContext(items),
    hold: (): boolean => {
      if (launchingRef.current) return false;
      launchingRef.current = true;
      setLaunching(true);
      return true;
    },
    release: (): void => {
      launchingRef.current = false;
      setLaunching(false);
    },
    launch: (form: ComposerLaunchForm, scope: ScopeOptions,
        context: readonly PromptContextItem[], options: { background: boolean; extra?: boolean }) =>
      launchRef.current(form, scope, context, { ...options, threadCard: true }),
    // The session runs either way and Rust seeds its record from the
    // registry, so a lost `bind` is logged rather than raised over the card.
    warn: (message: string): void => { console.warn("[threads]", message); },
    // Another model's thread that did not start: the card is gone by then.
    report: (message: string): void => { setActionError(message); },
  }));
  const threadCard = useThreadCard({
    request: threads.request, view: threads.view, sessions: state.sessions,
    pinnedProjects: appearance.projectPresentation?.pinnedProjects ?? NO_PINNED_PROJECTS,
    open: state.composerOpen && surface === "chat",
    // A plain New thread opens on the project the inbox is scoped to.
    scopeProject: threadScopeRoot(threadInbox.model.scope),
    pickDirectory: browse,
    port: threadLaunchPort,
  });
  const answerFolderTrust = useCallback((choice: FolderTrustChoice) => {
    const answer = folderTrustAnswer.current;
    folderTrustAnswer.current = null;
    setFolderTrustQuestion(null);
    answer?.(choice);
  }, []);
  // An open thread nobody was asked about (a schedule, team tools or an
  // outside agent started it) asks once in this window.
  const folderTrustAsked = useRef(new Set<string>());
  const openThreadFolderTrust = focusedThreadPane === null
    ? undefined : threadIndex.get(focusedThreadPane)?.folderTrust;
  const openTrustFolder = openThreadFolderTrust?.folder;
  const openTrustAsked = openThreadFolderTrust?.asked;
  useEffect(() => {
    const trust = openTrustFolder === undefined || openTrustAsked === undefined
      ? undefined : { folder: openTrustFolder, asked: openTrustAsked };
    if (trust === undefined || !openThreadAsksFolderTrust(trust, threadsUncovered, folderTrustAsked.current)) return;
    folderTrustAsked.current.add(trust.folder);
    void threadLaunchPort.askFolderTrust(trust.folder).then(choice => {
      const command = folderTrustCommand(choice, trust.folder);
      if (command !== null) threadRequest(command).catch((err: unknown) => { setActionError(ipcMessage(err)); });
    });
  }, [openTrustAsked, openTrustFolder, threadLaunchPort, threadRequest, threadsUncovered]);
  /** Each untrusted thread's line (protocol 72), one stable object per thread. */
  const folderTrustRowsRef = useRef<Readonly<Record<SessionKey, FolderTrustRow>>>({});
  const threadFolderTrustRows = useMemo(() => {
    const next: Record<SessionKey, FolderTrustRow> = {};
    for (const thread of threads.view?.threads ?? []) {
      const row = folderTrustRow(thread.folderTrust);
      if (row === null) continue;
      const held = folderTrustRowsRef.current[thread.key];
      next[thread.key] = held !== undefined && equalValue(held, row) ? held : row;
    }
    folderTrustRowsRef.current = next;
    return next;
  }, [threads.view]);
  /** The New thread card with each provider's accounts, for its account control (protocol 60). */
  const threadCardOnAccounts = useMemo(() => ({ ...threadCard.card, accounts: accountsView }),
    [accountsView, threadCard.card]);
  const threadDiff = useThreadDiff(threads.request, useCallback((key: string) =>
    threadDiffSignal(threadIndexRef.current.get(key)), []));
  const threadDiffRef = useRef(threadDiff);
  threadDiffRef.current = threadDiff;
  /*
   * SHIPPING A THREAD'S WORK (protocol 37): the Changes panel's foot and the
   * latest changes card's row, over one Rust-read state per thread. Fix
   * failing checks asks the thread's own pane to send Rust's prompt (its echo,
   * its refusal, its queue); review comments reach the draft the way a diff
   * line comment does.
   */
  const [threadSends, setThreadSends] = useState<Readonly<Record<SessionKey, { id: number; text: string }>>>(
    () => ({}));
  const threadSendId = useRef(0);
  const threadShipPort = useMemo<ThreadShipPort>(() => ({
    request: threads.request,
    send: (key, text) => {
      threadSendId.current += 1;
      const id = threadSendId.current;
      setThreadSends(current => ({ ...current, [key]: { id, text } }));
      return Promise.resolve();
    },
    quote: (key, comments) => {
      conversationDrafts.set(key, appendReviewComments(conversationDrafts.get(key), comments));
    },
  }), [conversationDrafts, threads.request]);
  const threadShip = useThreadShip(threadShipPort);
  const threadShipRef = useRef(threadShip);
  threadShipRef.current = threadShip;
  const [shipCommitRequest, setShipCommitRequest] = useState(0);
  // The open panel reads its thread's checkout and pull request when it
  // opens and again when a turn there ends.
  const shipPanelKey = threadDiff.target?.key ?? null;
  const shipPanelSignal = shipPanelKey === null ? "" : threadDiffSignal(threadIndex.get(shipPanelKey));
  useEffect(() => {
    if (shipPanelKey !== null) threadShipRef.current.refresh(shipPanelKey, true);
  }, [shipPanelKey, shipPanelSignal]);
  /** The latest changes card's ship step: run it, or open the panel where its form is. */
  const shipThread = useCallback((key: SessionKey, step: ShipNext | "read") => {
    const ship = threadShipRef.current;
    if (step === "read") ship.ensure(key);
    else if (step === "check") ship.refresh(key, true);
    else if (step === "push") void ship.openPullRequest(key);
    else if (step === "fixChecks") void ship.fixChecks(key);
    else if (step !== "none") {
      const thread = threadIndexRef.current.get(key);
      if (thread === undefined) return;
      setSubagentDetail(null);
      const current = threadDiffRef.current.target;
      threadDiffRef.current.open(key, current?.key === key ? current.choice : defaultThreadDiffChoice(thread));
      if (step === "commit") setShipCommitRequest(n => n + 1);
    }
  }, []);
  /**
   * ⌘D and the pane's Diff button. False when the pane is no project thread.
   * A panel hidden behind a subagent detail is shown, not closed; one closed
   * while it holds focus hands focus back to the pane's toggle.
   */
  const toggleThreadDiff = useCallback((key: SessionKey, viaKeyboard = false): boolean => {
    const thread = threadIndexRef.current.get(key);
    if (thread === undefined || !isProjectThread(thread)) return false;
    const current = threadDiffRef.current.target;
    const action = threadDiffToggleAction(current?.key ?? null, key, subagentDetailRef.current !== null);
    if (action === "close") {
      const focusInPanel = document.activeElement?.closest(".tdiff") != null;
      threadDiffRef.current.close();
      if (focusInPanel) focusThreadDiffToggle(key);
    } else {
      // One right column: the diff takes the subagent detail's slot.
      setSubagentDetail(null);
      threadDiffRef.current.open(key, action === "show" && current !== null ? current.choice
        : defaultThreadDiffChoice(thread), viaKeyboard);
    }
    return true;
  }, []);
  /** The panel's own Close and Esc: focus goes back to the pane's Diff toggle. */
  const closeThreadDiffPanel = useCallback(() => {
    const key = threadDiffRef.current.target?.key;
    threadDiffRef.current.close();
    if (key !== undefined) focusThreadDiffToggle(key);
  }, []);
  // The thread output preview's state and its openers (use-output-preview-state.ts).
  const {
    threadPreview, previewThreadFile, previewAttachment, closeThreadPreview, threadPreviewApi,
    openPreviewFolder,
  } = useOutputPreviewState(window.iface, {
    state, threadDiff, subagentDetail, sessionByKeyRef, threadDiffRef,
    setActionError, setSubagentDetail, openWorkspaceTool,
  });
  const outputFeedbackTargets = useMemo(() => {
    const rows = outputFeedbackTargetRows(
      previousByEventsRef.current.outputFeedbackTargets,
      state.sessions,
    );
    previousByEventsRef.current.outputFeedbackTargets = rows;
    return rows;
  }, [state.sessions]);
  const promptTranscript = useRef({ events: state.events, historyByKey, conversationEchoes });
  useLayoutEffect(() => { promptTranscript.current = { events: state.events, historyByKey, conversationEchoes }; });
  const sendVisiblePrompt = useCallback((key: SessionKey, text: string, context: readonly PromptContextItem[],
      source?: RecoverySource): Promise<void> =>
    sendWithVisibleTurn(() => {
      const current = promptTranscript.current;
      const live = conversationItems(current.events[key] ?? NO_EVENTS);
      const history = historyPaging.current.startedHere(key)
        ? IDLE_HISTORY : current.historyByKey[key] ?? LOADING_HISTORY;
      const past = history.status === "ready" ? history.items : NO_CONVERSATION_ITEMS;
      return newPendingEcho({
        id: rendererItemId("prompt"), text,
        seen: echoSeen(text, past, live, current.conversationEchoes[key] ?? NO_PENDING_ECHOES),
        at: echoPosition(live),
        historyKnown: history.status === "ready" || history.status === "idle" || history.status === "unavailable",
        images: promptTurnImages(context),
        attachments: promptTurnAttachments(context),
      });
    }, {
      send: (before, accepted) => outbound.sendWithContext(key, text, context, before, accepted, source),
      append: echo => { appendConversationEcho(key, echo); },
      checkpoint: turn => { checkpointTurn(key, turn); },
      delivered: () => { closedKeys.current.delete(key); markLive(key); },
      fail: (id, message) => { failConversationEcho(key, id, message); },
    }), [outbound, appendConversationEcho, checkpointTurn, markLive, failConversationEcho]);
  const sendOutputFeedback = useCallback(async (key: string, text: string): Promise<"sent" | "unknown" | "unsent"> => {
    try { await sendVisiblePrompt(key, text, []); return "sent"; }
    catch (failure) { setActionError(ipcMessage(failure)); return deliveryOutcomeUnknown(failure) ? "unknown" : "unsent"; }
  }, [sendVisiblePrompt]);
  // Quick Open over a project thread's folder can open a file in its preview.
  const quickOpenThread = quickOpen.open && quickOpen.contextTarget !== null && surface === "chat"
    ? threadIndex.get(quickOpen.contextTarget) : undefined;
  const quickOpenPreviewKey = quickOpenThread !== undefined && isProjectThread(quickOpenThread)
    ? quickOpenThread.key : null;
  const previewQuickOpenFile = useCallback((file: ProjectFileEntry) => {
    if (quickOpenPreviewKey === null) return;
    closeQuickOpen();
    previewThreadFile(quickOpenPreviewKey, file.path);
  }, [closeQuickOpen, previewThreadFile, quickOpenPreviewKey]);
  const openThreadTurnDiff = useCallback((key: SessionKey, ordinal: number) => {
    setSubagentDetail(null);
    threadDiffRef.current.open(key, { scope: "turn", ordinal });
  }, []);
  const threadPanePort = useMemo<ThreadPanePort>(() => ({
    request: threads.request,
    // The new thread's pane takes focus, as a launched one's does.
    select: key => {
      if (launchingRef.current) return;
      pendingTileFocus.current = key;
      selectSession(key);
    },
    setDraft: (key, text) => { conversationDrafts.set(key, text); },
    openCard: (project, workspace, prompt, note) => {
      threadCard.seed(project, workspace, note);
      openFreshComposer(null, prompt);
    },
    permissionModeOf: key => givenPermissionMode(sessionByKeyRef.current.get(key),
      permissionModeRef.current[key], launchPermissionRef.current[key]),
    setPermissionMode: changePermissionMode,
    working: key => sessionBusy(sessionByKeyRef.current.get(key)),
    closeSettled: key => {
      if (sessionBusy(sessionByKeyRef.current.get(key))) return;
      closeLiveSession(key).catch((err: unknown) => {
        console.warn("[threads] a settled thread's session stayed open:", err);
      });
    },
    notify: message => { setTurnRestoreNotice({ message, error: false }); },
  }), [changePermissionMode, closeLiveSession, conversationDrafts, openFreshComposer, selectSession,
    threadCard.seed, threads.request]);
  // One Fork or Edit per pane at a time: a double click makes one thread, and
  // the waiting pane draws its Fork and Edit controls off until the answer.
  const [threadPaneBusy, setThreadPaneBusy] = useState<ReadonlySet<SessionKey>>(() => new Set());
  const [threadPaneLock] = useState(() => new ThreadPaneLock(setThreadPaneBusy));
  const editThreadFromTurn = useCallback((key: SessionKey, ordinal: number,
      restoreFiles: boolean, prompt: string): Promise<void> => {
    const thread = threadIndexRef.current.get(key);
    if (thread === undefined) return Promise.reject(new Error("This thread is no longer listed."));
    return threadPaneLock.run(key, () => editThreadFrom(threadPanePort, thread, ordinal,
      restoreFiles, prompt)) ?? Promise.reject(new Error(THREAD_PANE_BUSY));
  }, [threadPaneLock, threadPanePort]);
  // The Changes panel's Undo on a worktree turn: Rust restores the worktree's
  // files to the turn's start checkpoint and keeps a recovery point; the
  // notice (or Rust's refusal) is the turn restore notice, as a local Undo's is.
  const restoreThreadTurnFiles = useCallback((key: SessionKey, ordinal: number) => {
    const thread = threadIndexRef.current.get(key);
    if (thread === undefined) {
      setTurnRestoreNotice({ message: "This thread is no longer listed.", error: true });
      return;
    }
    const restoring = threadPaneLock.run(key, () => restoreThreadFiles(threadPanePort, thread, ordinal));
    if (restoring === null) {
      setTurnRestoreNotice({ message: THREAD_PANE_BUSY, error: true });
      return;
    }
    restoring.catch((err: unknown) => {
      setTurnRestoreNotice({ message: ipcMessage(err), error: true });
    });
  }, [threadPaneLock, threadPanePort]);
  const forkThreadAtTurn = useCallback((key: SessionKey, keepTurns: number) => {
    const forking = threadPaneLock.run(key, () => forkThreadAt(threadPanePort, key, keepTurns));
    if (forking === null) return;
    setActionError(null);
    forking.catch((err: unknown) => {
      setActionError(ipcMessage(err));
    });
  }, [threadPaneLock, threadPanePort]);
  /**
   * A Threads pane's split +: the card opens on that pane's project — a
   * worktree thread's is the project it was made from, not its worktree, and
   * a plain chat's is No project — in the workspace last used there.
   */
  const threadCardSeed = threadCard.seed;
  const splitThreadTile = useCallback((summary: SessionSummary) => {
    const thread = threadIndexRef.current.get(sessionKey(summary.engine, summary.sessionId));
    const project = threadProjectRoot(summary, thread);
    // Seeded only once the sheet is opening, so a refused split leaves no seed behind.
    if (splitWorkspaceTile(project)) threadCardSeed(project);
  }, [splitWorkspaceTile, threadCardSeed]);
  /** A Threads pane's head: its branch chip and lineage, and its Diff toggle. */
  const threadTileHead = (key: SessionKey):
      Pick<ComponentProps<typeof AgentTile>, "headDetail" | "headActions"> => {
    const thread = threadIndex.get(key);
    if (thread === undefined) return {};
    const parentKey = thread.lineage?.parent;
    const parent = parentKey === undefined ? undefined : sessionByKey.get(parentKey);
    const switchedFrom = threadSwitchedFrom(thread.chain);
    return {
      headDetail: <ThreadPaneHead workspace={thread.workspace} pullRequest={thread.pullRequest}
        checkoutBranch={threadShip.stateOf(key).ship?.branch}
        account={threadAccountLabel(accountsView, parseKey(key).engine, thread.account)}
        lineage={thread.lineage === undefined || parent === undefined || parentKey === undefined
          ? undefined : { relation: thread.lineage.relation, title: parent.title,
            onSelect: () => { selectSession(parentKey); } }}
        {...(switchedFrom === null ? {} : { switchedFrom })}
        {...(thread.startedFromOutside === undefined ? {}
          : { startedFromOutside: thread.startedFromOutside })} />,
      headActions: isProjectThread(thread)
        ? <ThreadDiffToggle open={threadDiff.target?.key === key && subagentDetail === null}
          totals={threadChangeTotals(thread)}
          onToggle={() => { toggleThreadDiff(key); }} />
        : undefined,
    };
  };
  /**
   * A line comment from the Changes panel lands in that thread's own draft,
   * after anything already typed there, for the owner to edit and send. The
   * draft store notifies the pane, so it appears in the composer at once.
   */
  // Protocol 67: side questions. `/btw` asks; the panel shows the side
  // thread's answer; Bring back and Close end it (Rust removes the side thread).
  const [sidePanelKey, setSidePanelKey] = useState<SessionKey | null>(null);
  const [sideClosing, setSideClosing] = useState(false);
  const askSideQuestion = useCallback(async (key: SessionKey, question: string): Promise<string | null> => {
    try {
      await threadRequest({ kind: "sideAsk", key, question });
      threadDiffRef.current.close();
      setSubagentDetail(null);
      setSidePanelKey(key);
      return null;
    } catch (err: unknown) {
      return ipcMessage(err);
    }
  }, [threadRequest]);
  const closeSideQuestion = useCallback((key: SessionKey) => {
    setSideClosing(true);
    threadRequest({ kind: "sideClose", key })
      .then(() => { setSidePanelKey(current => (current === key ? null : current)); },
        (err: unknown) => { setActionError(ipcMessage(err)); })
      .finally(() => { setSideClosing(false); });
  }, [threadRequest]);
  const quoteDiffIntoDraft = (key: SessionKey, quote: ThreadDiffQuote): void => {
    conversationDrafts.set(key, appendDiffQuote(conversationDrafts.get(key), quote));
  };
  /** The project a Threads pane's breadcrumb names; none for a plain chat. */
  const threadTileProject = (key: SessionKey, summary: SessionSummary): string | undefined => {
    const root = threadProjectRoot(summary, threadIndex.get(key));
    return root === null ? undefined : pathLeaf(root);
  };

  // Agent mode with no agent has nothing to hand a task to, so New makes the
  // agent first: its name, what it does, and its icon.
  const agentFirst = surface === "agent" && activeAgent === undefined;
  const surfaceNewLabel = agentFirst ? "New agent" : SURFACE_NEW_LABEL[surface];
  const openComposerOrAgent = useCallback(() => {
    if (agentFirst) openNewAgentDialog(); else openComposer();
  }, [agentFirst, openComposer, openNewAgentDialog]);

  const newCodeWorkspace = useCallback((projectPath?: string) => {
    startCodeWorkspace(projectPath);
  }, [startCodeWorkspace]);

  const [newProjectOpen, setNewProjectOpen] = useState(false);
  const newProjectReturnFocusRef = useRef<HTMLElement | null>(null);
  const openNewProject = useCallback(() => {
    if (launchingRef.current) return;
    newProjectReturnFocusRef.current = mobileLayout ? sidebarToggleRef.current
      : document.activeElement instanceof HTMLElement ? document.activeElement : null;
    if (mobileLayout) setSidebarOpen(false);
    setNewProjectOpen(true);
  }, [mobileLayout]);
  const createProject = useCallback(async (name: string): Promise<void> => {
    const path = await window.iface.createProject(name);
    newCodeWorkspace(path);
    setNewProjectOpen(false);
  }, [newCodeWorkspace]);
  // Where New project creates: Rust's Projects folder, read each time the
  // dialog opens (the folder can appear or vanish between openings).
  const [projectsFolder, setProjectsFolder] = useState<ProjectsFolderView | null>(null);
  useEffect(() => {
    if (!newProjectOpen) return;
    let live = true;
    void window.iface.projectsFolder({ projectsFolder: "read" })
      .then(view => { if (live) setProjectsFolder(view); })
      .catch(() => { if (live) setProjectsFolder(null); });
    return () => { live = false; };
  }, [newProjectOpen]);
  const changeProjectsFolder = useCallback(async (): Promise<void> => {
    setProjectsFolder(await window.iface.projectsFolder({ projectsFolder: "choose" }));
  }, []);

  const openComposerFromSidebar = useCallback(() => {
    // The guard first, like showSurface and selectSession: both callees refuse
    // during a launch, and leaving Settings for a click that then does nothing
    // is a visible half-navigation.
    if (launchingRef.current) return;
    leaveSettings();
    if (surfaceRef.current === "code") {
      newCodeWorkspace();
    } else openComposerOrAgent();
    if (mobileLayout) {
      // The New button becomes hidden with the modal sidebar. Restore to the
      // visible toolbar toggle when the composer later closes.
      composerOpenerRef.current = sidebarToggleRef.current;
      setSidebarOpen(false);
    }
  }, [leaveSettings, mobileLayout, newCodeWorkspace, openComposerOrAgent]);

  const openComposerInProjectFromSidebar = useCallback((projectPath: string) => {
    if (surfaceRef.current === "code") {
      newCodeWorkspace(projectPath);
    } else {
      // A project's own + opens the New thread card on that project.
      if (surfaceRef.current === "chat" && !launchingRef.current) threadCardSeed(projectPath);
      openFreshComposer(projectPath);
    }
    if (mobileLayout) {
      composerOpenerRef.current = sidebarToggleRef.current;
      setSidebarOpen(false);
    }
  }, [mobileLayout, newCodeWorkspace, openFreshComposer, threadCardSeed]);

  // Both filters move together, because the sidebar offers them as one control:
  // if anything is being held back, the click shows everything; if nothing is,
  // it puts the decluttered view back. Two independent switches would need two
  // rows of chrome above the list to stay honest about which one is hiding what
  // — see Sidebar's `onToggleFilters`.
  //
  // Write then set, the same order Sidebar's collapse toggle uses: the store is
  // updated from the handler rather than from inside the state updater, which
  // React may call twice.
  const toggleFilters = useCallback(() => {
    const next = !(hideTrivial || appOnly);
    writeStored(HIDE_TRIVIAL, next ? "1" : "0");
    writeStored(APP_ONLY, next ? "1" : "0");
    setHideTrivial(next);
    setAppOnly(next);
  }, [hideTrivial, appOnly]);

  // The rate-limit note for the selected session, if its latest usage.updated
  // carried one. Same map the sidebar badges read.
  const selectedLimit = surfaceSessionKey !== null
    ? rateLimits[surfaceSessionKey] : undefined;
  // Only engines that actually meter a plan. The panel drops the others
  // server-side, and a chip for an engine with no number to ever show is the
  // same furniture in the trigger's tooltip.
  const usageEngines = useMemo<UsageEngineRef[]>(() => engines
    // Qwen remains a launchable local agent, but its local token accounting is
    // not a provider quota and does not belong in the plan-usage menu.
    .filter(engine => engine.id !== "acp:qwen"
      && engineCan(engineOps, engine.id, "readUsage"))
    .map(engine => ({
      id: engine.id,
      ...(engine.label === undefined ? {} : { label: engine.label }),
    })), [engineOps, engines]);

  taskInspectorHost.current = useCommittedActions<TaskInspectorHost>({
    replaceAttentionEvents: events => {
      setAttention(current => equalValue(current.events, events) ? current : { ...current, events });
    },
    reportActionError: message => { setActionError(message); },
    focusedTarget: () => {
      const focusedKey = focusedKeyRef.current;
      const focusedSession = focusedKey ? sessionByKeyRef.current.get(focusedKey) : undefined;
      const terminal = surface === "code" && centerView === "terminal" ? terminalSummaries.find(item => item.id === focusedTerminalId) : undefined;
      return terminal ? { root: terminal.cwd, target: { kind: "terminal" as const, terminalId: terminal.id, generation: terminal.generation } }
        : focusedSession && focusedKey ? { root: focusedSession.cwd, target: { kind: "session" as const, key: focusedKey } } : null;
    },
    terminalRunning: (terminalId, generation) =>
      terminalSummaries.some(item => item.id === terminalId && item.generation === generation),
    selectSession,
    appendConversationDraft: (key, text) => {
      conversationDrafts.set(key, [conversationDrafts.get(key), text].filter(Boolean).join("\n\n"));
    },
    openIsolatedComposer: root => {
      openFreshComposer(root, "");
      setComposerSeed(current => ({ ...current, worktree: true }));
    },
    openBrowserPane: (terminalId, url) => {
      const center = terminalCenter.current;
      if (center === null) return false;
      showTerminals();
      return center.openBrowser(url, terminalId);
    },
  });
  const openTaskInspector = taskInspector.openInspector;
  // memo(IntegratedTerminalCenter) compares props by identity, so its callbacks
  // must not be inline arrows: one would re-render the whole terminal tree on
  // every App render. The center only forwards these to click handlers.
  const openUsageFromTerminals = useCommittedCallback(() => { setUsageOpen(true); });
  const openTasksFromTerminals = useCommittedCallback(() => { showSurface("agent"); });
  const openSavedFromTerminals = useCommittedCallback(() => { openSessionLibrary("search"); });
  // The ··· menu's palette destinations; the menu closes itself first.
  const showCanvasFromMenu = useCommittedCallback(() => { showCanvas(); });
  const openWorktreesFromMenu = useCommittedCallback(() => { openSessionLibrary("worktrees"); });
  const openContextInventoryFromMenu = useCommittedCallback(() => { openTaskInspector("context"); });
  const inspectTerminalTask = useCommittedCallback((id: string) => {
    const terminal = terminalSummaries.find(item => item.id === id);
    if (terminal) openTaskInspector("read", { root: terminal.cwd, target: { kind: "terminal", terminalId: id, generation: terminal.generation } });
  });

  // The shell's own commands (palette-commands.ts); the rows below them name
  // sessions. Open in external terminal is enabled while Code's terminals show.
  const terminalsShown = surface === "code" && centerView === "terminal";
  const paletteThemeRows = useMemo(() => paletteThemes(appearance.preference,
    appearance.customThemes ?? []), [appearance.customThemes, appearance.preference]);
  const paletteCommands = useMemo(() => shellPaletteCommands({
    newLabel: surfaceNewLabel,
    newDescription: agentFirst ? AGENT_FIRST_DESCRIPTION : SURFACE_INTRO[surface].description,
    attentionItems: attentionRowList.length,
    terminalsShown,
    openPanes: workspace.openKeys.length,
    focusedPane: workspace.focusedKey !== null,
    maximized: workspace.maximizedKey !== null,
    sidebarOpen, launching, refreshing,
    voiceOn: voiceState !== "off", voiceBusy,
    overviewOpen: terminalMenu?.overview?.open === true,
    themes: paletteThemeRows,
  }), [agentFirst, attentionRowList.length, launching, paletteThemeRows, refreshing, sidebarOpen,
    surface, surfaceNewLabel, terminalMenu?.overview?.open, terminalsShown, voiceBusy, voiceState,
    workspace.focusedKey, workspace.maximizedKey, workspace.openKeys.length]);
  // One row per session (about 1,100 of them) is built only while the palette
  // is open: closed, it draws nothing, and every registry push rebuilt them.
  const paletteSessionCommands = useMemo<readonly CommandPaletteCommand[]>(() => !commandPaletteOpen
    ? NO_PALETTE_COMMANDS : state.sessions.map(summary => {
      const key = sessionKey(summary.engine, summary.sessionId);
      const provider = providerLabel(summary.engine, engines.find(engine => engine.id === summary.engine)?.label);
      return {
        id: `agent:${key}`,
        label: summary.title,
        group: "Agents",
        description: `${provider} · ${statusWord(summary.status)}`,
        keywords: [summary.engine, summary.status, summary.cwd, summary.sessionId],
      } satisfies CommandPaletteCommand;
    }), [commandPaletteOpen, engines, state.sessions]);
  const paletteAllCommands = useMemo(() => paletteSessionCommands.length === 0
    ? paletteCommands : [...paletteCommands, ...paletteSessionCommands],
  [paletteCommands, paletteSessionCommands]);

  /**
   * ⌘P. The picker needs an authorized index and something to attach a pick
   * to, and both come from the focused session; with no session focused the
   * chord still answers, with the ordinary palette, rather than doing what it
   * did for a year — nothing at all, no picker, no beep, no message.
   */
  const openFilePalette = useCallback(() => {
    const target = layoutFor(workspacesRef.current, surfaceRef.current).focusedKey;
    const cwd = target === null ? "" : sessionByKeyRef.current.get(target)?.cwd ?? "";
    if (target !== null && cwd !== "") {
      openContextQuickOpen(cwd, target);
      return;
    }
    setCommandQuery("");
    setCommandPaletteOpen(true);
  }, [openContextQuickOpen]);

  const runPaletteCommand = useCallback((command: CommandPaletteCommand) => {
    closeQuickOpen();
    if (command.id.startsWith("agent:")) {
      selectSession(command.id.slice("agent:".length));
      return;
    }
    // Commands that move or reshape the workspace must reveal that workspace;
    // otherwise the command succeeds behind the full-surface new-session prompt
    // and appears to do nothing. New keeps the composer by definition, while a
    // sidebar/refresh action is useful without leaving it.
    if (!paletteCommandKeepsComposer(command.id)) dismissComposer(false);
    if (command.id.startsWith(THEME_COMMAND_PREFIX)) {
      changeAppearancePreference(command.id.slice(THEME_COMMAND_PREFIX.length) as ThemeId);
      return;
    }
    switch (command.id) {
      case "harness.results": openTaskInspector("read"); break;
      case "harness.next": {
        const live = attentionRowList.filter(item => item.live);
        const candidates = live.length ? live : attentionRowList;
        const index = candidates.findIndex(item => item.id === nextAttentionId.current);
        const row = candidates[(index + 1) % candidates.length];
        if (row) { nextAttentionId.current = row.id; activateAttention(row); } break;
      }
      case "workspace.new":
        if (surfaceRef.current === "code") newCodeWorkspace();
        else openComposerOrAgent();
        break;
      case "surface.chat": showSurface("chat"); break;
      case "surface.code": showSurface("code"); break;
      case "surface.agent": showSurface("agent"); break;
      case "workspace.canvas": showCanvas(); break;
      case "workspace.newProject": openNewProject(); break;
      case "workspace.overview":
        showTerminals();
        terminalMenu?.overview?.onToggle();
        break;
      // Files is the output preview beside Code's terminals; Browser a tab in
      // the focused pane. Both need a project in Code.
      case "tool.files": case "tool.browser":
        showTerminals();
        if (terminalCenter.current?.openTool(command.id === "tool.files" ? "files" : "browser") !== true) {
          setActionError("Open a project in Code first.");
        }
        break;
      case "away.open": setAwayOpenRequest(request => request + 1); break;
      case "voice.toggle": toggleVoice(); break;
      case "settings.remote": openSettingsAt("remote"); break;
      // The review dialog portals to the body, so this does not first move the
      // user to the terminal surface.
      case "terminals.review": terminalCenter.current?.openReviewDialog(); break;
      case "workspace.library": openSessionLibrary("search"); break;
      case "workspace.archives": openSessionLibrary("archives"); break;
      case "workspace.worktrees": openSessionLibrary("worktrees"); break;
      case "workspace.externalTerminal": {
        const project = terminalCenter.current?.controlProject() ?? null;
        if (project === null || project === "") {
          setActionError("Focus a terminal in a project first.");
          break;
        }
        setActionError(null);
        window.iface.openTerminal(project).catch(failAction);
        break;
      }
      case "workspace.next": cycleWorkspace("next"); break;
      case "workspace.previous": cycleWorkspace("previous"); break;
      case "workspace.maximize":
        if (workspace.focusedKey !== null) toggleAgentMaximized(workspace.focusedKey);
        break;
      case "workspace.close":
        if (workspace.focusedKey !== null) closeWorkspaceTile(workspace.focusedKey);
        break;
      case "workspace.sidebar": toggleSidebar(); break;
      case "workspace.settings": openSettings(); break;
      case "workspace.usage": setUsageOpen(true); break;
      case "workspace.refresh": void refreshAll("explicit"); break;
    }
  }, [openTaskInspector, attentionRowList, activateAttention, closeQuickOpen, closeWorkspaceTile, cycleWorkspace, dismissComposer,
    failAction, openComposerOrAgent, openSessionLibrary, openSettings, refreshAll, selectSession,
    showCanvas, showSurface, showTerminals,
    newCodeWorkspace, toggleAgentMaximized, changeAppearancePreference, openNewProject,
    openSettingsAt, terminalMenu, toggleSidebar, toggleVoice,
    workspace.focusedKey]);

  useEffect(() => {
    if (!sidebarOpen) return;
    const closeSidebarOnEscape = (event: KeyboardEvent): void => {
      if (event.key !== "Escape" || !mobileLayout) return;
      // A dialog layered over the sidebar owns Escape first. Closing both the
      // dialog and the navigation behind it with one key press loses context.
      if (document.querySelector(FOREGROUND_DIALOG_SELECTOR) !== null) return;
      event.preventDefault();
      closeMobileSidebar();
    };
    window.addEventListener("keydown", closeSidebarOnEscape);
    return () => { window.removeEventListener("keydown", closeSidebarOnEscape); };
  }, [closeMobileSidebar, mobileLayout, sidebarOpen]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent): void => {
      const key = event.key.toLowerCase();
      const modifier = event.metaKey || event.ctrlKey;
      const foregroundDialog = commandPaletteOpen || sessionLibraryOpen
        || document.querySelector(FOREGROUND_DIALOG_SELECTOR) !== null;

      // The narrow detail pane is a non-modal side sheet. Escape still closes
      // it from anywhere in the visible workspace; an open modal or mobile
      // sidebar keeps first claim on the key.
      if (event.key === "Escape" && subagentDetailRef.current !== null
          && !foregroundDialog && !state.composerOpen && !settingsOpen
          && !(mobileLayout && sidebarOpen)
          && document.querySelector(TRANSCRIPT_FIND_SELECTOR) === null) {
        event.preventDefault();
        closeSubagentDetail();
        return;
      }

      if (modifier && !event.shiftKey && !event.altKey && key === ",") {
        event.preventDefault();
        if (foregroundDialog || launchingRef.current) return;
        if (settingsOpen) closeSettings(); else openSettings();
        return;
      }

      // ⌘W closes the selected pane. The chord is always taken here, so the
      // Window menu's Close Pane never acts on the same press, and a held
      // chord closes one pane, not every pane.
      if (isClosePaneChord(event)) {
        event.preventDefault();
        if (!event.repeat) closeSelectedPane();
        return;
      }

      const modalOpen = foregroundDialog || state.composerOpen || settingsOpen;
      const overlayShortcut = workspaceOverlayShortcut(event, {
        composerOpen: state.composerOpen, settingsOpen, foregroundDialog,
      });
      if (overlayShortcut === "palette") {
        event.preventDefault();
        setCommandQuery("");
        setCommandPaletteOpen(true);
        return;
      }

      // The open overlay owns these keystrokes. In particular Cmd/Ctrl+P in
      // the palette's files mode must not fall through to Electron's Print
      // action or open a second modal behind the first one.
      if (modalOpen) {
        if (overlayShortcut === "prevent-browser"
            || modifier && (key === "p" || key === "n" || key === "f")) {
          event.preventDefault();
        }
        return;
      }

      if (event.ctrlKey && !event.metaKey && !event.shiftKey && !event.altKey
          && event.code === "Backquote") {
        event.preventDefault();
        showTerminals();
        return;
      }

      // A focused shell keeps readline's Ctrl+P / Ctrl+N and Ctrl+Tab; the
      // ⌘ spellings below stay the app's, and Ctrl+` above still opens the
      // terminals.
      if (shellOwnsChord(event, event.target instanceof Element
          && event.target.closest(".terminal-center") !== null)) return;

      // ⌘P is go-to-file — the chord every editor this app mirrors binds, and
      // which used to be captured only so it could never reach Electron Print.
      if (modifier && !event.shiftKey && !event.altKey && key === "p") {
        event.preventDefault();
        openFilePalette();
        return;
      }
      if (modifier && event.shiftKey && key === "p") {
        event.preventDefault();
        setCommandQuery("");
        setCommandPaletteOpen(true);
        return;
      }
      if (modifier && !event.shiftKey && key === "n") {
        event.preventDefault();
        if (surfaceRef.current === "code") newCodeWorkspace();
        else openComposerOrAgent();
        return;
      }
      // ⌘D: the focused thread's diff panel (Threads only; Code's ⌘D splits
      // a terminal and never reaches here from a focused shell).
      if (event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey && key === "d"
          && surfaceRef.current === "chat" && workspace.focusedKey !== null
          && toggleThreadDiff(workspace.focusedKey, true)) {
        event.preventDefault();
        return;
      }
      if (event.ctrlKey && event.key === "Tab") {
        event.preventDefault();
        cycleWorkspace(event.shiftKey ? "previous" : "next");
        return;
      }
      if (event.metaKey && event.altKey && (event.key === "ArrowLeft"
          || event.key === "ArrowRight")) {
        event.preventDefault();
        cycleWorkspace(event.key === "ArrowLeft" ? "previous" : "next");
        return;
      }
      if (event.metaKey && event.shiftKey && key === "m") {
        event.preventDefault();
        if (workspace.focusedKey !== null) toggleAgentMaximized(workspace.focusedKey);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => { window.removeEventListener("keydown", onKeyDown); };
  }, [closeSelectedPane, closeSettings, commandPaletteOpen, cycleWorkspace,
    closeSubagentDetail, mobileLayout, openComposerOrAgent, openFilePalette,
    openSettings, sessionLibraryOpen,
    showTerminals, newCodeWorkspace,
    settingsOpen, sidebarOpen, state.composerOpen, toggleAgentMaximized,
    toggleThreadDiff, workspace.focusedKey]);

  // One committed object for every pane: its members never change identity, so
  // memo(SessionTile) compares only the pane's own values.
  const recoverPrompt = async (key: SessionKey, mode: RecoveryMode, id?: string, expectedPrompt?: string): Promise<void> => {
    const current = await window.iface.recoveryControl({ version: 1, kind: "read", key });
    const receipt = current.receipt;
    if (!receipt || (id !== undefined && receipt.id !== id)) throw new Error("No current recovery receipt exists for this prompt. Inspect the thread and write a new instruction.");
    if (expectedPrompt !== undefined && normalizeThreadPrompt(expectedPrompt) !== normalizeThreadPrompt(receipt.input.text)
        && normalizeThreadPrompt(expectedPrompt) !== normalizeThreadPrompt(receipt.serialized)) {
      throw new Error("The selected turn is not the preserved prompt. Use the current recovery card or write a new instruction.");
    }
    const response = await window.iface.recoveryControl({ version: 1, kind: "prepare", key, id: receipt.id, mode });
    const prepared = response.prepared;
    if (!prepared) throw new Error("Recovery could not be prepared; nothing was sent.");
    const releaseRecoveryContext = outbound.captureContext(prepared.context);
    try {
      const paths = prepared.context.filter(item => item.kind === "file" || item.kind === "folder").map(item => item.value);
      if (paths.length > 0) {
        const grants = await window.iface.grantPromptContextPaths(paths);
        if (grants.rejected.length > 0) throw new Error("An original attachment is missing or no longer safe to read. Reattach it in the composer; nothing was sent.");
      }
      await sendVisiblePrompt(key, prepared.text, prepared.context, prepared.source);
    } catch (failure) {
      if (deliveryOutcomeUnknown(failure)) { setActionError(`${ipcMessage(failure)} Delivery is uncertain; inspect the thread before continuing.`); return; }
      throw failure;
    } finally { releaseRecoveryContext(); setRecoveryWake(value => value + 1); }
  };

  const readPendingRecovery = useCallback((key: string, list = false) =>
    window.iface.recoveryControl({ version: 1, key, kind: list ? "listPending" : "read" }), []);
  const forgetPromptRecovery = useCallback(async (key: string, id: string): Promise<void> => {
    await window.iface.recoveryControl({ version: 1, kind: "forget", key, id });
    setRecoveryWake(value => value + 1);
  }, []);

  // Visual replies (T3 section 8): Rust's page, wrapped and served by main
  // into a sandboxed frame. The only place either call is made.
  const visualFrames = useMemo(() => visualFrameLoader({
    threadJournal: command => window.iface.threadJournal(command),
    visualFrame: request => window.iface.visualFrame(request),
  }), []);
  const sessionTileActions = useCommittedActions<SessionTileActions>({
    readPromptRecovery: key => window.iface.recoveryControl({ version: 1, kind: "read", key }),
    recoverPrompt, forgetPromptRecovery,
    dismissRecap, appendConversationEcho, checkpointTurn,
    failConversationEcho, retryHistoryFor, loadOlderHistoryFor, changeModelFor, changeEffortFor,
    changeFastModeFor, openSubagentDetail, setSessionContextOpen, requestPromptContext,
    removeContextFor, consumeSessionContext, attachDroppedPaths, attachPastedImages,
    changePermissionModeFor: (key, mode) => { changePermissionMode(key, mode); },
    enqueue: (key, text, context) => outbound.enqueue(key, text, context),
    applyQueueUpdate: (key, update) => outbound.applyQueueUpdate(key, update),
    sendQueuedNow: (key, id) => { outbound.sendQueuedNow(key, id); },
    setQueueEditing: (key, id, editing) => { outbound.setQueueEditing(key, id, editing); },
    submitPrompt: (key, text, context) => outbound.submitPrompt(key, text, context),
    runCommand: (key, text) => outbound.runCommand(key, text),
    stopSession, showAgentThread, approveFor, denyFor, restoreTurnCheckpoint,
    openThreadTurnDiff, editThreadFromTurn, forkThreadAtTurn, reportThreadTurnRows, shipThread,
    awayThread,
    // A turn that failed signed out signs in the account its thread runs on
    // (protocol 60), not whichever account is the provider's default now.
    signInFor: key => {
      const engine = parseKey(key).engine;
      if (accountGroupFor(providerAccounts.viewRef.current, engine) === undefined) {
        openSignIn(engine);
        return;
      }
      openAccountSignIn(engine, threadIndexRef.current.get(key)?.account ?? "default");
    },
    openStartedThread: key => { selectSession(key); },
    editSchedule: (key, schedule) => { schedules.openDialog(key, scheduleTitle(key), schedule); },
    cancelSchedule: id => { schedules.cancelSchedule(id); },
    giveSecret: (key, id, value) => threadSecrets.give(key, id, value),
    askSideQuestion: (key, question) => askSideQuestion(key, question),
    declineSecret: (key, id) => { threadSecrets.decline(key, id); },
    // Protocol 66: the owner's queued follow-up that never left goes with the
    // resume and leaves the queue once Rust holds it.
    resumeThreadAtReset: key => {
      const queued = resumeQueuedPrompt(steeringQueues[key] ?? []);
      threadRequest({ kind: "resumeAtReset", key, ...(queued === null ? {} : { prompt: queued.text }) })
        .then(() => {
          if (queued !== null) outbound.applyQueueUpdate(key, queue => removeSteeringPrompt(queue, queued.id));
        }, (err: unknown) => { setActionError(ipcMessage(err)); });
    },
    snoozeThreadUntil: (key, until) => {
      threadRequest({ kind: "snooze", key, until })
        .catch((err: unknown) => { setActionError(ipcMessage(err)); });
    },
    // Protocol 72: the line goes once the next view says the folder is trusted.
    trustThreadFolder: (_key, folder) => {
      threadRequest({ kind: "trustFolder", folder, trust: true })
        .catch((err: unknown) => { setActionError(ipcMessage(err)); });
    },
    opinionAction, switchThreadProvider, continueThreadOnAccount,
    previewThreadFile: (key, path) => { previewThreadFile(key, path); },
    previewAttachment: (key, attachment) => { previewAttachment(key, attachment); },
    loadProviderCommands: scope => window.iface.listProviderCommands(scope),
    pathForFile: file => window.iface.pathForFile(file),
    loadStash: draftStash.load,
    stashDraft: draftStash.stash,
    restoreStash: draftStash.restore,
    removeStash: draftStash.remove,
    loadVisual: (key, id, scheme) => visualFrames.load(key, id, scheme),
    releaseVisual: url => { visualFrames.release(url); },
  });

  /**
   * One session's transcript, exactly as a workspace tile draws it. Everything
   * the pane shows is derived here, during App's render — including the facts
   * App keeps in refs — and handed to the memoized SessionTile as values.
   */
  function tileConversation(
    key: SessionKey, summary: SessionSummary, focused: boolean, tileConfig?: SessionConfigView,
    threadLayout = false,
  ) {
    const log = state.events[key] ?? NO_EVENTS;
    const tileLive = liveKeys.current.has(key);
    const tileHistory = historyPaging.current.startedHere(key)
      ? IDLE_HISTORY : historyByKey[key] ?? LOADING_HISTORY;
    const inferredProgress = liveTurnProgress(log, summary.status);
    turnStartedAtByKey.current = rememberTurnStart(
      turnStartedAtByKey.current, key, inferredProgress.active, Date.now());
    const tileProgress = tileLive
      ? inferredProgress : { active: false, label: inferredProgress.label };
    const tileOutboundPending = outbound.outboundPending(key);
    const tileConfigPending = configTokens.pending(key);
    const tileContextLocked = outbound.isSending(key) || tileConfigPending;
    const folderScoped = sessionHasWorkspaceContext(summary);
    const tileThread = threadIndex.get(key);
    // ONE view of this session's model, effort and access facts: the pane's own
    // status line reads it here, and the tile header's access selector reads the
    // same call over the same inputs, so the two cannot disagree.
    if (tileConfig === undefined) {
      const tileChoices = choicesFor(summary.engine);
      tileConfig = sessionConfigFor(key, summary, log,
        tileChoices.modelOptions ?? NO_MODELS,
        engineLabels[summary.engine] ?? providerLabel(summary.engine),
        launchEffort, pending, configErrors, modelPresets,
        tileChoices.permissionModes,
        tileChoices.dangerousModes ?? DANGEROUS_MODES,
        permissionMode[key] ?? launchPermission[key] ?? "default",
        permissionPending[key] === true,
        configTokens.inFlightFor(key, "fastmode"),
        permissionMode[key]);
    }
    return (
      <SessionTile
        recoveryWake={recoveryWake}
        sessionKey={key}
        title={summary.title}
        cwd={summary.cwd}
        // Paths in the transcript are shown relative to this, so a diff row
        // reads `src/main/bridge.ts` instead of 69 characters whose first 46
        // are the same on every row. Cosmetic only — the absolute path stays
        // on each row's title and in the find index. projectRoot is the git
        // root; cwd covers a session that is not inside a repository.
        pathRoot={summary.projectRoot ?? summary.cwd}
        folderScoped={folderScoped}
        drafts={conversationDrafts}
        recap={recaps[key]}
        pendingEchoes={conversationEchoes[key] ?? NO_PENDING_ECHOES}
        events={log}
        commands={state.providerCommands[key]}
        engineVersion={state.engineHealth
          .find(probe => probe.engine === summary.engine)?.version}
        planName={usage.status === "ready"
          ? usage.views.find(view => view.engine === summary.engine)?.plan
          : undefined}
        history={tileHistory}
        historyFrozen={historyPaging.current.isFrozen(key)}
        focused={focused}
        thinking={tileProgress.active}
        workingLabel={tileProgress.label}
        turnStartedAt={turnStartedAtByKey.current[key]}
        live={tileLive}
        config={tileConfig}
        contextItems={folderScoped ? sessionContext[key] ?? NO_CONTEXT_ITEMS : NO_CONTEXT_ITEMS}
        contextLocked={tileContextLocked}
        contextOpen={sessionContextOpen[key] ?? false}
        // PER TILE, like pathRoot and for the same reason: one pane
        // is one session, and an agentKey is unique only inside one.
        // Handing every tile the bare key would paint a selected
        // cell in panes the user never clicked.
        selectedSubagentKey={subagentDetail?.sessionKey === key
          ? subagentDetail.agentKey : undefined}
        queue={steeringQueues[key] ?? NO_QUEUE}
        queueSendingId={queueSending[key] ?? null}
        queueError={queueErrors[key] ?? queueNotices[key] ?? null}
        queueControlsLocked={tileOutboundPending || tileConfigPending}
        sendLocked={tileConfigPending}
        queueWhileBusy={tileProgress.active || tileOutboundPending}
        canSteer={canSteerEngine(summary.engine)}
        approvals={verdicts}
        agentStore={agentStore}
        agentStartKnownKeys={agentStartKnownKeys}
        turnRestorePoints={turnRestorePoints[key]}
        restoringTurnKey={restoringTurn?.key === key ? restoringTurn.turnKey : null}
        thread={tileThread === undefined ? undefined
          : withShip(conversationThread(tileThread, threadPaneBusy.has(key)), isProjectThread(tileThread)
            ? shipCardView(threadShip.stateOf(key).ship, threadShip.stateOf(key).needsCheck,
              threadShip.stateOf(key).busy !== null, tileThread)
            : undefined)}
        threadLayout={threadLayout}
        threadSend={threadSends[key]}
        headroom={threadUsageHeadroom(headrooms, summary.engine, tileThread?.account)}
        opinions={threadOpinions[key]}
        opinionPending={opinionPending}
        limitOffer={threadLimitOffers[key] ?? null}
        limitReset={threadLimitResets[key] ?? null}
        folderTrust={threadFolderTrustRows[key] ?? null}
        startedThreads={startedThreads.get(key)}
        stash={threadLayout && tileThread !== undefined ? draftStash.stashes[key] ?? NO_STASH : undefined}
        canReferThreads={threadReferenceCapability(threads.view, key).available}
        scheduleLine={schedules.lines.get(key)}
        scheduleNow={schedules.now}
        secretRequest={tileThread?.secretRequest}
        actions={sessionTileActions}
      />
    );
  }

  let agentTaskPane = (
    <div className="empty workspace-hero agent-empty">
      <InterfaceMark className="empty-mark" />
      {activeAgent ? <>
        <h2 className="workspace-welcome-title">{`Start a task with ${activeAgent.name}`}</h2>
        <p className="empty-text">
          {`Give ${activeAgent.name} a job to do in a folder, then watch it work.`}
        </p>
        <div className="empty-actions">
          <button type="button" className="btn btn-primary" onClick={openComposer}>New task</button>
        </div>
      </> : <>
        <h2 className="workspace-welcome-title">
          {agentStore.agents.length === 0 ? "Create your first agent" : "Choose an agent or create one"}
        </h2>
        <p className="empty-text">{AGENT_FIRST_DESCRIPTION}</p>
        <div className="empty-actions">
          <button type="button" className="btn btn-primary" disabled={!agentReady}
            onClick={openNewAgentDialog}>New agent</button>
        </div>
      </>}
    </div>
  );
  if (agentTaskSession !== undefined && agentTaskKey !== null) {
    const conversation = tileConversation(agentTaskKey, agentTaskSession,
      surface === "agent" && !state.composerOpen && !settingsOpen
        && !commandPaletteOpen);
    agentTaskPane = (
      <AgentTask key={agentTaskKey} session={agentTaskSession}
        events={agentTaskEvents}
        history={agentTaskHistory?.status === "ready" ? agentTaskHistory : undefined}
        commands={state.providerCommands[agentTaskKey]}
        engineLabel={engineLabels[agentTaskSession.engine]
          ?? providerLabel(agentTaskSession.engine)}
        live={liveKeys.current.has(agentTaskKey)}
        turnStartedAt={turnStartedAtByKey.current[agentTaskKey] ?? null}
        now={now}
        diffStats={agentDiffStats}
        onStop={() => { stopSession(agentTaskKey); }}
        onOpenDiff={() => {
          // Rust has no Files/diff surface, so a touched file reveals the
          // session that changed it, on the surface that owns it (this one).
          selectSession(agentTaskKey);
        }}>
        {conversation}
      </AgentTask>
    );
  }
  /** The newest message the owner sent in an agent's current thread, by echo id. */
  const agentThreadLastSendId = (agentId: string): string | undefined => {
    const key = agentStore.threads?.[agentId]?.current?.sessionKey;
    return key === undefined ? undefined : conversationEchoes[key]?.at(-1)?.id;
  };
  // The agent owns the pane, so its header sits above BOTH the zero-state and a
  // running task rather than becoming part of either.
  if (activeAgent !== undefined) {
    const agent = activeAgent;
    agentTaskPane = (
      <div className="agent-surface-stack">
        <AgentHeader profile={agent}
          engineLabel={engineLabels[agent.engine] ?? providerLabel(agent.engine)}
          workingCount={agentWorkingCount}
          onEdit={() => { setAgentDialog({ mode: "edit", id: agent.id }); }}
          onDelete={() => { setAgentDeleteId(agent.id); }} />
        <AgentWorkspacePanel key={agent.id} agent={agent} response={agentStore} pending={agentPending}
          error={agentError} onCommand={mutateAgent} onRequest={requestAgentPage}
          provider={state.engineHealth.find(probe => probe.engine === agent.engine)}
          shownSessionKey={workspaces.agent.focusedKey}
          visible={surface === "agent" && centerView === "sessions" && !settingsOpen && documentVisible}
          threadLastSendId={agentThreadLastSendId(agent.id)}
          onOpenTask={selectSession} onOpenThread={selectSession}
          onShowThread={() => { showAgentThread(agent.id); }}
          onPickDirectory={() => window.iface.pickDirectory()}>
          {agentTaskPane}
        </AgentWorkspacePanel>
      </div>
    );
  }

  // The canvas surface's coordination slot, hoisted out of its JSX so the
  // memo'd panel can hit: an inline engines.map defeated it on every token.
  const refreshCollaborations = useCallback(() => { void loadCollaborations(); },
    [loadCollaborations]);
  const coordinationPanel = useMemo(() => <CollaborationPanel
    statuses={collaborations}
    sessions={collaborationSessions}
    loading={collaborationsLoading}
    error={collaborationsError}
    onRefresh={refreshCollaborations}
    onSubmit={submitCollaboration}
    onApprove={approveCollaboration}
    onDeny={denyCollaboration}
    onAcceptPeer={acceptCollaborationPeer}
    onDiscardPeer={discardCollaborationPeer}
    onCancel={cancelCollaboration}
    onOpenAgent={selectSession}
  />, [acceptCollaborationPeer, approveCollaboration, cancelCollaboration,
    collaborations, collaborationsError, collaborationsLoading, collaborationSessions,
    denyCollaboration, discardCollaborationPeer, refreshCollaborations, selectSession,
    submitCollaboration]);
  const overlayDialogOpen = commandPaletteOpen || sessionLibraryOpen || taskInspector.isOpen
    || contextValueTarget !== null || threadReferenceTarget !== null
    || agentDialog !== null || agentDeleteId !== null
    || awayOpen || newProjectOpen || updateRestartConfirmOpen || updateRestartPending;
  // Everything the shell owns — toolbar, main, status bar, detail pane — is
  // taken out of the tree behind a foreground dialog, and behind the narrow
  // sessions overlay unless Settings is the surface holding it open.
  const shellInert = overlayDialogOpen || (!settingsOpen && sidebarOpen && mobileLayout);
  // Settings covers the sidebar, the work area and the status bar, so they
  // leave the tree under it; the toolbar row above it stays live.
  const behindSettings = shellInert || settingsOpen;
  // The inspector belongs to a visible transcript, never to an app-level
  // destination. The effect above clears the selection on a context switch;
  // this gate also prevents a one-frame flash beside that destination.
  const detailOpen = subagentDetail !== null && !settingsOpen && !state.composerOpen
    && centerView === "sessions" && !(mobileLayout && sidebarOpen)
    && (workspace.maximizedKey === null
      || workspace.maximizedKey === subagentDetail.sessionKey);
  // It slides out still showing its agent; the track closes after it.
  const detail = usePresence(detailOpen);
  // The thread diff panel takes the same column, under the same rules, while
  // no subagent detail holds it, and only beside its own thread's open pane.
  const threadDiffKey = threadDiff.target?.key ?? null;
  const threadDiffThread = threadDiffKey === null ? undefined : threadIndex.get(threadDiffKey);
  const threadDiffOpen = threadDiffKey !== null && threadDiffThread !== undefined
    && subagentDetail === null && surface === "chat" && !settingsOpen && !state.composerOpen
    && centerView === "sessions" && !(mobileLayout && sidebarOpen)
    && gridKeys.includes(threadDiffKey)
    && (workspace.maximizedKey === null || workspace.maximizedKey === threadDiffKey);
  const threadDiffPresence = usePresence(threadDiffOpen);
  // Protocol 67: the side question panel takes the column after the diff
  // panel and the subagent detail, beside its own thread's open pane: the one
  // it was asked from, else the focused pane when its thread has one open.
  const sideParentKey = [sidePanelKey, workspace.focusedKey]
    .find(key => key !== null && threadIndex.get(key)?.side !== undefined) ?? null;
  const sideParent = sideParentKey === null ? undefined : threadIndex.get(sideParentKey);
  const sideOpen = sideParent?.side !== undefined && sideParentKey !== null && !threadDiffOpen
    && subagentDetail === null && surface === "chat" && !settingsOpen && !state.composerOpen
    && centerView === "sessions" && !(mobileLayout && sidebarOpen) && gridKeys.includes(sideParentKey)
    && (workspace.maximizedKey === null || workspace.maximizedKey === sideParentKey)
    && !(threadPreview !== null && threadPreview.shown);
  const sidePresence = usePresence(sideOpen);
  const threadPreviewOpen = threadPreview !== null && threadPreview.shown && !threadDiffOpen && subagentDetail === null
    && !settingsOpen && !(mobileLayout && sidebarOpen)
    && (threadPreview.key === null
      ? state.composerOpen
      : !state.composerOpen && centerView === "sessions"
        && (surface === "chat"
          ? gridKeys.includes(threadPreview.key)
            && (workspace.maximizedKey === null || workspace.maximizedKey === threadPreview.key)
          : surface === "agent" && agentTaskKey === threadPreview.key));
  const threadPreviewPresence = usePresence(threadPreviewOpen);
  // Narrow windows only: the drawer and its scrim glide out before the overlay
  // class drops. The desktop column's slide is the grid transition above.
  const drawer = usePresence(sidebarOpen);
  const drawerLeaving = mobileLayout && drawer.phase === "leaving";
  // Hidden on a desktop-width window, the sidebar slides back over the work
  // while the pointer rests on the window's left edge.
  const sidebarPeek = useSidebarPeek(!sidebarOpen && !mobileLayout && !overlayDialogOpen);
  // The corner notices: the action error, a rollback's result and each thing
  // the last refresh could not do. Only a refresh has a retry.
  const cornerNotices = useMemo<CornerNoticeItem[]>(() => {
    const items: CornerNoticeItem[] = [];
    if (actionError !== null) {
      items.push({ id: `action:${actionError}`, message: actionError, tone: "error",
        onExpire: () => { setActionError(null); },
        dismissLabel: "Dismiss error", onDismiss: dismissActionError });
    }
    if (turnRestoreNotice !== null) {
      items.push({ id: `restore:${turnRestoreNotice.message}`, message: turnRestoreNotice.message,
        tone: turnRestoreNotice.error ? "error" : "status",
        onExpire: () => { setTurnRestoreNotice(null); },
        dismissLabel: "Dismiss restore result",
        onDismiss: () => {
          setTurnRestoreNotice(null);
          focusAfterNoticeDismissal();
        } });
    }
    const name = (engine: string) => engineLabels[engine] ?? providerLabel(engine);
    for (const failure of refreshFailures) {
      const drop = () => { setRefreshFailures(current => current.filter(entry => entry !== failure)); };
      items.push({ id: refreshFailureId(failure),
        message: refreshFailureSentence(failure, name, productLabel()),
        detail: refreshFailureDetail(failure), tone: "error",
        onExpire: drop,
        onRetry: () => { void refreshAll("explicit"); },
        dismissLabel: "Dismiss refresh error",
        onDismiss: () => {
          drop();
          focusAfterNoticeDismissal();
        } });
    }
    // Once per profile: what leaves this Mac on its own. A status card, so it
    // waits for an answer, and either answer marks it seen.
    if (showPrivacySummary(appPreferences)) {
      const seen = () => { persistPreferences(privacySummarySeen(appPreferences)); };
      items.push({ id: PRIVACY_SUMMARY_ID, title: PRIVACY_SUMMARY_TITLE, message: PRIVACY_SUMMARY_BODY,
        tone: "status",
        action: { label: PRIVACY_SUMMARY_OPEN, onClick: () => { seen(); openSettingsAt("privacy"); } },
        dismissLabel: `${PRIVACY_SUMMARY_DONE}: hide the privacy summary`, dismissText: PRIVACY_SUMMARY_DONE,
        onDismiss: () => {
          seen();
          focusAfterNoticeDismissal();
        } });
    }
    return items;
  }, [actionError, appPreferences, dismissActionError, engineLabels, focusAfterNoticeDismissal, openSettingsAt,
    persistPreferences, refreshAll, refreshFailures, turnRestoreNotice]);

  // Hoisted so the breadcrumb's callback closes over a value the compiler
  // has already narrowed, rather than re-reading an optional chain.
  const detailParent = detailView?.parent;
  const updateVersion = updateState?.availableVersion ?? "the downloaded update";

  return (
    <div ref={appGridRef} className={(sidebarOpen || drawerLeaving ? "app-grid sidebar-overlay" : "app-grid no-sidebar")
      + (drawerLeaving ? " is-drawer-leaving" : "")
      + (sidebarPeek.shown ? " sidebar-peek" : "")
      // The third track exists only while there is something in it. No
      // persistent empty inspector: a 340px column reading "nothing selected"
      // is 340px of nothing on every screen with no subagents on it.
      + (threadDiffPresence.present && !detail.present ? " has-detail has-thread-diff" : "")
      + (threadPreviewPresence.present && !detail.present && !threadDiffPresence.present ? " has-detail has-thread-diff" : "")
      + (sidePresence.present && !detail.present && !threadDiffPresence.present ? " has-detail" : "")
      + (detail.present ? " has-detail" : "")}
      onAnimationEnd={event => { if (event.animationName === "sidebar-drawer-out") drawer.settle(); }}>
      <style>{APP_SELECT_CSS}</style>
      <style>{RECAP_CSS}</style>
      {newProjectOpen && <CreateProjectDialog onCreate={createProject}
        folder={projectsFolder} onChangeFolder={changeProjectsFolder}
        onCancel={() => { setNewProjectOpen(false); }} returnFocusRef={newProjectReturnFocusRef} />}
      {taskInspector.isOpen && <TaskInspector {...taskInspector.inspectorProps}
        sessionTitle={key => sessionByKey.get(key)?.title} />}
      {schedules.dialog !== null && (
        <ThreadScheduleDialog key={`${schedules.dialog.key}:${schedules.dialog.editing?.id ?? "new"}`}
          threadTitle={schedules.dialog.title} editing={schedules.dialog.editing}
          onSave={schedules.save} onCancel={schedules.closeDialog} />
      )}

      <ConfirmDialog
        open={folderTrustQuestion !== null}
        title={FOLDER_TRUST_TITLE}
        description={folderTrustBody(folderTrustQuestion ?? "")}
        confirmLabel={FOLDER_TRUST_CONFIRM}
        cancelLabel={FOLDER_TRUST_CANCEL}
        tone="default"
        onConfirm={() => { answerFolderTrust("trust"); }}
        onCancel={() => { answerFolderTrust("notNow"); }}
        onDismiss={() => { answerFolderTrust("dismiss"); }}
      />
      <ConfirmDialog
        key={updateRestartPending ? "update-pending" : "update-confirm"}
        open={updateRestartConfirmOpen || updateRestartPending}
        pending={updateRestartPending}
        title={updateRestartPending ? "Preparing the update" : `Restart to install ${updateVersion}?`}
        description={updateRestartPending
          ? `${productLabel()} will restart automatically as soon as macOS is ready.`
          : `Save any editor changes first. Continuing allows ${productLabel()} to discard unsaved changes during the restart.`}
        confirmLabel="Restart to update"
        cancelLabel="Not now"
        tone="default"
        returnFocusRef={updateRestartOpenerRef}
        onConfirm={confirmRestartToUpdate}
        onCancel={cancelRestartToUpdate}
      />
      {/* Keyed per editing session so the form always opens on the agent it was
          opened for, never on the last one's leftovers. */}
      {agentError && <div className="agent-workspace-error" role="alert">{agentError}</div>}
      <AgentProfileDialog
        key={agentDialog === null || agentDialog.mode === "new"
          ? "agent-new" : `agent-${agentDialog.id}`}
        open={agentDialog !== null}
        initial={agentDialogInitial}
        engines={agentEngines}
        effortLevelsFor={agentEffortLevels}
        modelsFor={engine => choicesFor(engine).modelOptions ?? []}
        initialPermission={agentDialog?.mode === "edit" ? agentPermissionMode(agentStore, agentDialog.id) : "gated"}
        onSubmit={submitAgentProfile}
        onCancel={() => { setAgentDialog(null); }}
      />
      <ConfirmDialog
        open={agentBeingDeleted !== undefined}
        title={`Delete agent “${agentBeingDeleted?.name ?? ""}”?`}
        description={agentBeingDeleted?.home
          ? `Its past tasks stay in the list. Its folder stays at ${agentBeingDeleted.home}.`
          : "Its past tasks stay in the list."}
        confirmLabel="Delete agent"
        tone="danger"
        onConfirm={() => {
          if (agentDeleteId !== null) {
            void mutateAgent({ kind: "removeAgent", id: agentDeleteId });
          }
          setAgentDeleteId(null);
        }}
        onCancel={() => { setAgentDeleteId(null); }}
      />
      {/* THE attention live region. Deliberately a sibling of the shell rather
          than a child of the toolbar: the whole <main> takes aria-hidden while
          a palette or dialog is open, and an alarm that goes quiet because the
          user opened the command palette is not an alarm.

          ITS CONTENT DEPENDS ON THE COUNT AND ON NOTHING ELSE. It used to
          blank itself on the destination that listed the same agents, which
          bought a duplicate-free reading at the cost of the two things this
          region exists for: 0 -> 1 there was announced NOT AT ALL, and merely
          navigating away changed "" to "3 agents need you" — an alarm fired by
          a click, when nothing had changed. A live region whose text is a
          function of the fact alone cannot do either: it speaks on every real
          transition, in every view, and stays silent through navigation.
          Mounted at every count, including zero: a region announces changes
          only once it is already in the tree. */}
      <div className="terminal-sr-only" role="status" aria-live="polite">
        {attentionAnnouncement(waitingSessions)}
      </div>
      <header className="workspace-toolbar" aria-label="Workspace controls"
        inert={shellInert || undefined}
        aria-hidden={shellInert || undefined}>
        <div className="workspace-toolbar-leading">
        <span className="workspace-toolbar-traffic" aria-hidden="true" />
        {/* One toggle, just right of the traffic lights in every state, so it
            never moves when the sidebar slides away (owner, 2026-09-17), and
            sits over the panel it brings back: while the sidebar is hidden,
            resting the pointer on it slides the panel out underneath (owner,
            2026-10-20, after Codex). */}
        <button ref={sidebarToggleRef} type="button"
          data-sidebar-peek-keep
          className="workspace-icon-btn workspace-sidebar-toggle"
          aria-label={sidebarOpen ? "Hide sidebar" : "Show sidebar"}
          aria-pressed={sidebarOpen}
          aria-description={!sidebarOpen && providerAlert !== undefined ? providerAlert.label : undefined}
          title={!sidebarOpen && providerAlert !== undefined ? `Toggle sidebar (${providerAlert.label})` : "Toggle sidebar"}
          onClick={toggleSidebar}
          {...sidebarPeek.trigger}>
          {/* The sidebar is on the left, so its glyph fills the left column;
              the outline twin says it is hidden. */}
          <span className={sidebarOpen ? "codicon codicon-layout-sidebar-left"
            : "codicon codicon-layout-sidebar-left-off"} aria-hidden="true" />
          {/* The footer's setup notice is out of sight with the sidebar: its
              amber dot rides on the button that brings the sidebar back. */}
          {!sidebarOpen && providerAlert !== undefined && (
            <span className="sidebar-toggle-setup-dot" aria-hidden="true" />
          )}
        </button>
        <span className="workspace-brand" aria-label={productLabel()}>
          <InterfaceMark className="workspace-brand-mark" />
          <span className="workspace-brand-name"><ProductName /></span>
        </span>
        </div>
        <SurfaceSwitch surface={surface} onChange={showSurface} />
        <div className="workspace-toolbar-actions">
        {surface === "code" && centerView === "terminal" && (
          <span className="workspace-toolbar-terminal" ref={setTerminalToolbarSlot} />
        )}
        {/* Ready restarts through the existing confirm; anything earlier opens Settings. */}
        {updateLabel !== null && updateState !== null && (
          <UpdateButton state={updateState} label={updateLabel} title={updateTitle}
            disabled={updateState.phase === "ready" && updateRestartPending}
            onClick={updateState.phase === "ready" ? requestRestartToUpdate : openSettings} />
        )}
        {railUsage !== null && (
          <button type="button" className={`workspace-usage-readout is-${railUsage.tone}`}
            aria-label={`Usage: ${railUsage.provider} ${railUsage.window}, ${railUsage.text} used`}
            title={`${railUsage.provider}: ${railUsage.text} of the ${railUsage.window} limit used. Open usage.`}
            onClick={() => { setUsageOpen(true); }}>
            <ProviderLogo engine={railUsage.engine} label={railUsage.provider} size={12} />
            <span className="workspace-usage-readout-text">
              <span className="workspace-usage-readout-value">{railUsage.window} {railUsage.text}</span>
              <span className="workspace-usage-readout-bar" aria-hidden="true">
                <span style={{ width: `${railUsage.percent.toString()}%` }} />
              </span>
            </span>
          </button>
        )}
        <button type="button"
          className={voiceState === "off"
            ? "workspace-icon-btn workspace-toolbar-core"
            : `workspace-icon-btn workspace-toolbar-core voice-toggle is-${voiceState}`}
          aria-label="Voice assistant"
          aria-pressed={voiceState !== "off"}
          disabled={voiceBusy}
          title={voiceState === "off"
            ? "Talk to the voice assistant"
            : `Voice assistant: ${VOICE_STATE_TEXT[voiceState]}`}
          onClick={toggleVoice}>
          <span className={voiceState === "off"
            ? "codicon codicon-mic" : "codicon codicon-mic-filled"} aria-hidden="true" />
        </button>
        <AttentionTray
          anchorRef={commandPaletteButtonRef}
          rows={attentionRowList}
          onInspect={row => {
            const target = row.target;
            const root = target.kind === "session" ? sessionByKey.get(target.key)?.cwd
              : terminalSummaries.find(terminal => terminal.id === target.terminalId && terminal.generation === target.generation)?.cwd;
            if (!root || (target.kind === "terminal" && target.generation === undefined)) { setActionError("The original checkout is unavailable. Open task history from the command palette."); return; }
            setAttentionOpen(false); openTaskInspector("read", { root, target: target as DurableAttentionTarget });
          }}
          unread={attentionUnread}
          open={attentionOpen}
          onOpenChange={openAttention}
          onActivate={activateAttention}
          onClearAll={clearAttention}
          now={now}
        />
        <AwayControl currentProject={() => awayProject(surface, awayFocusedProject,
            () => terminalCenter.current?.controlProject() ?? voiceCwd)}
          providerReady={awayProviderReady}
          request={window.iface.projectAway} pickDirectory={window.iface.pickDirectory}
          onInspectSession={selectSession} onSessionKeys={reportAwaySessionKeys}
          onOpenThread={openAwayThread} threadKeys={awayThreadKeys} engines={engines} choicesFor={choicesFor}
          onRunningChange={setAwayRunning} openRequest={awayOpenRequest}
          onOpenChange={setAwayOpen} inert={shellInert} />
        {/* Usage opens from the ··· menu; its panel hangs from ··· (theme.css). */}
        <span className="workspace-more-anchor">
          <WorkspaceMoreMenu buttonRef={commandPaletteButtonRef} inert={shellInert}
            attention={{ rows: attentionRowList, unread: attentionUnread,
              onOpen: () => { openAttention(true); } }}
            onUsage={openUsageFromTerminals}
            layout={surface === "code" && centerView === "terminal" ? terminalMenu : null}
            onOpenTasks={openTasksFromTerminals}
            onOpenSavedSessions={openSavedFromTerminals}
            onCanvas={showCanvasFromMenu}
            onWorktrees={openWorktreesFromMenu}
            onContextInventory={openContextInventoryFromMenu}
            onCommands={() => { setCommandQuery(""); setCommandPaletteOpen(true); }} />
          <UsageMenu
            anchorRef={commandPaletteButtonRef}
            state={usage}
            now={now}
            open={usageOpen}
            engines={usageEngines}
            onOpenChange={setUsageOpen}
            onRefresh={refreshUsage}
          />
        </span>
        {/* The agent overview's own toggle at the window's right edge, where
            the column opens (owner, 2026-10-01); ··· keeps its row too. */}
        {surface === "code" && centerView === "terminal" && terminalMenu?.overview !== undefined && (
          <button type="button" className="workspace-icon-btn workspace-overview-toggle"
            aria-label="Live panes" aria-pressed={terminalMenu.overview.open}
            title={terminalMenu.overview.open ? "Hide live panes" : "Show live panes"}
            onClick={terminalMenu.overview.onToggle}>
            <span className="codicon codicon-layout-sidebar-right" aria-hidden="true" />
          </button>
        )}
        {settingsOpen && (
          <button type="button" className="workspace-icon-btn"
            aria-label="Close settings" title="Close settings (Esc)"
            onClick={closeSettings}>
            <span className="codicon codicon-close" aria-hidden="true" />
          </button>
        )}
        </div>
      </header>
      {/* Rendered unconditionally and hidden in CSS (.no-sidebar), NOT unmounted.
          The sidebar force-expands the group holding the selection once per
          mount, which is correct exactly once per app run — it is what stops a
          restored selection from sitting invisible under a persisted collapse.
          Unmounting on the toggle turned that into once per show, so hiding and
          re-showing the pane silently erased a collapse the user had chosen by
          hand. Keeping the element mounted makes the toggle purely visual. */}
      <Sidebar
        focusOnMobileOpen={!settingsOpen && sidebarOpen && mobileLayout}
        backgroundInert={overlayDialogOpen || settingsOpen}
        onCloseMobile={closeMobileSidebar}
        newChatActive={state.composerOpen && !settingsOpen}
        settingsActive={settingsOpen}
        onSettings={openSettings}
        footerAlert={providerAlert}
        onFooterAlert={openProviderSettings}
        settingsDisabled={launching}
        groups={groups}
        protectedProjects={view.protectedProjects}
        terminalProjects={view.terminalProjects}
        pinnedProjects={appearance.projectPresentation?.pinnedProjects}
        onPinProject={appearance.projectPresentation === undefined ? undefined : pinProject}
        pinnedWorkspaces={appearance.workspacePresentation?.pinnedWorkspaces}
        onPinWorkspace={appearance.workspacePresentation === undefined ? undefined : pinWorkspace}
        glass={(appearance.translucency?.sidebar ?? 0) > 0}
        codeWorkspaces={surface === "code"}
        loading={sidebarLoading}
        projectCount={view.projectCount}
        rateLimits={rateLimits}
        diffStats={diffStats}
        peeks={sidebarPeeks}
        hidden={view.hidden}
        hideable={view.hideable}
        importable={view.importable}
        onToggleFilters={toggleFilters}
        query={query}
        onQuery={setQuery}
        searching={view.searching}
        hiddenMatches={view.hiddenMatches}
        lifecycle={lifecycle}
        importer={importer}
        onSelect={selectSession}
        agentMenu={agentMenu}
        terminalsActive={surface === "code" && centerView === "terminal"
          && !state.composerOpen && !settingsOpen}
        {...surface !== "code" || terminalSwitcher === null ? {} : {
          terminalWorkspaces: sidebarTerminalWorkspaces!,
        }}
        onOpenInNewPane={openSessionInNewPane}
        onNew={openComposerFromSidebar}
        onNewProject={surface === "code" ? openNewProject : undefined}
        newLabel={surfaceNewLabel}
        sectionLabel={SURFACE_SECTION_LABEL[surface]}
        zeroStateText={SURFACE_ZERO_TEXT[surface]}
        onNewInProject={openComposerInProjectFromSidebar}
        onSearchTranscripts={searchTranscriptsFor}
        onArchive={hideSessionLocally}
        threadInbox={surface === "chat" ? threadInbox : undefined}
        threadBridges={threadBridges}
        threadLive={threadLive}
      />

      {/* A real button, rather than a click handler on an anonymous scrim: on
          narrow windows it is both the outside-click target and an accessible
          named way to dismiss the navigation. CSS removes it entirely on
          desktop, where the sidebar remains a normal persistent column. */}
      {!settingsOpen && (sidebarOpen || drawerLeaving) && (
        <div className="sidebar-backdrop" aria-hidden="true"
          onClick={closeMobileSidebar} />
      )}

      <main className="workspace-shell"
        inert={behindSettings || undefined}
        aria-hidden={behindSettings || undefined}>
        <div className="workspace-content">
        <div className="workspace-overlay-stack"
          inert={state.composerOpen || settingsOpen ? true : undefined}
          aria-hidden={state.composerOpen || settingsOpen || undefined}>
        {/* A worktree that was KEPT because it still holds uncommitted work
            (or, for a thread's own worktree, ignored files or another session).
            This is the only place its path is ever stated, so it is dismissed
            by hand and never on a timer. */}
        {/* A store Rust set aside at startup because it could not read it.
            Shown once, until dismissed; Reveal shows the kept bytes. */}
        {storeNotices.length > 0
          && <div className="notice-region" role="group" aria-label="Data notices">
        {storeNotices.map(notice => (
          <div key={notice.id} className="notice" role="status" data-store-notice={notice.id}>
            <span className="codicon codicon-sm codicon-warning notice-glyph" aria-hidden="true" />
            <span className="notice-text">{storeNoticeText(notice, productLabel())}</span>
            <button type="button" className="notice-action"
              onClick={() => { revealStoreNotice(notice.id); }}>
              Reveal
            </button>
            <button type="button" className="notice-close" aria-label="Dismiss"
              onClick={() => { dismissStoreNotice(notice.id); }}>
              <span className="codicon codicon-sm codicon-close" aria-hidden="true" />
            </button>
          </div>
        ))}
        </div>}
        {(notices.length > 0 || keptWorktrees.length > 0)
          && <div className="notice-region" role="group" aria-label="Worktree notices">
        {notices.map((n, i) => (
          // Index in the key: the same worktree can be reported twice, and two
          // identical keys is a React warning plus a row that will not dismiss.
          <div key={`${i}:${n.key}:${n.path}`} className="notice" role="status">
            <span className="codicon codicon-sm codicon-warning notice-glyph" aria-hidden="true" />
            <span className="notice-text">
              {n.state === "kept"
                ? "Kept the worktree for this session — it still has uncommitted work."
                : "Started from the last commit — this isolated agent cannot see your uncommitted source changes."}
              {" "}<span className="notice-path">{n.path}</span>
              {" "}on branch <span className="notice-path">{n.branch}</span>.
              {n.detail !== undefined && ` ${n.detail}`}
            </span>
            <button
              type="button"
              className="notice-close"
              aria-label="Dismiss"
              onClick={() => { dismissNotice(i); }}
            >
              <span className="codicon codicon-sm codicon-close" aria-hidden="true" />
            </button>
          </div>
        ))}
        {keptWorktrees.map(notice => (
          <div key={`thread:${notice.key}`} className="notice" role="status" data-thread-kept={notice.kept}>
            <span className="codicon codicon-sm codicon-warning notice-glyph" aria-hidden="true" />
            <span className="notice-text">
              Kept the worktree <span className="notice-path">{notice.path}</span>
              : {threadKeptReason(notice.kept)}. Remove it when you&rsquo;re done.
            </span>
            <button type="button" className="notice-close" aria-label="Dismiss"
              onClick={() => { dismissKeptWorktree(notice.key); }}>
              <span className="codicon codicon-sm codicon-close" aria-hidden="true" />
            </button>
          </div>
        ))}
        </div>}

        </div>
        {/* Errors and a rollback's result: small notices in the bottom-right
            corner (owner, 2026-11-15; a full-width bar across the top before).
            An error still leaves by itself: the element clears its own state
            when the CSS linger ends, and a new message is a new notice with a
            fresh countdown. Under the composer and Settings like the stack
            above. */}
        <CornerNotices items={cornerNotices}
          inert={state.composerOpen || settingsOpen} />

        {state.composerOpen && (
          <div className="workspace-composer-panel"
            onInputCapture={() => { composerEditEpoch.current += 1; }}
            inert={settingsOpen ? true : undefined} aria-hidden={settingsOpen || undefined}
            aria-busy={!composerReady}>
            {!composerReady && (
              <div className="workspace-empty composer-loading-state" role="status"
                aria-live="polite" aria-label={preparingLabel(surface)}>
                <div className="workspace-empty-inner">
                  <span className="codicon codicon-loading codicon-modifier-spin"
                    aria-hidden="true" />
                  <h2>{preparingLabel(surface)}</h2>
                  <p>Finding your default Projects folder and available models…</p>
                </div>
              </div>
            )}
            {composerReady && <Composer
              key={composerSeed.revision}
              open={state.composerOpen}
              kind={surface}
              defaults={composerDefaults}
              engines={engines}
              choicesFor={choicesFor}
              loadProviderCommands={scope => window.iface.listProviderCommands(scope)}
              modelPresets={surface === "agent" && activeAgent !== undefined ? undefined : modelPresets}
              teammate={surface === "agent" ? activeAgent?.name : undefined}
              onModelPresetChange={rememberLaunchModelPreset}
              headroom={headrooms}
              onLaunch={launch}
              launching={launching}
              onBrowse={browse}
              contextItems={composerContext}
              onRecipeContext={setComposerContext}
              contextError={composerContextError}
              contextCapabilities={promptContextCapabilities(composerContextRoot)}
              contextOpen={composerContextOpen}
              onContextOpenChange={setComposerContextOpen}
              onRepositoryChange={root => {
                if (root !== composerContextRoot) resetComposerContext();
                setComposerContextRoot(root);
              }}
              onContextRequest={(kind, cwd) => {
                requestPromptContext(COMPOSER_CONTEXT_TARGET, kind, cwd);
              }}
              onContextRemove={id => { removeContextFor(COMPOSER_CONTEXT_TARGET, id); }}
              onContextPreview={item => { previewAttachment(null, item, composerContextRoot); }}
              onDroppedPaths={paths => { attachDroppedPaths(COMPOSER_CONTEXT_TARGET, paths); }}
              onPastedImages={files => attachPastedImages(COMPOSER_CONTEXT_TARGET, files)}
              pathForFile={file => window.iface.pathForFile(file)}
              onClose={() => { dismissComposer(true); }}
              thread={surface === "chat" ? threadCardOnAccounts : undefined}
              notice={<>
                {surface !== "chat" && composerDirectory.error !== null && (
                  <div className="launch-error composer-directory-error" role="alert">
                    <span>{composerDirectory.error} You can choose a folder below or try again.</span>
                    <button type="button" className="btn" onClick={retryComposerDirectory}>
                      Try again
                    </button>
                  </div>
                )}
                {launchError !== null && (
                  <div className="launch-error" role="alert">{launchError}</div>
                )}
                <PendingLaunchRecoveryPanel wake={recoveryWake} read={readPendingRecovery} forget={forgetPromptRecovery}
                  openSession={selectSession}
                  preview={item => { previewAttachment(null, item, composerContextRoot); }}
                  restore={async receipt => {
                    if (receipt.delivery !== "unsent") return;
                    const epoch = contextRequestEpoch.current;
                    const editEpoch = composerEditEpoch.current;
                    const priorContext = draftAttachmentRefs.current.composerContext;
                    const paths = receipt.input.context.filter(item => item.kind === "file" || item.kind === "folder").map(item => item.value);
                    if (paths.length > 0) {
                      const grants = await window.iface.grantPromptContextPaths(paths);
                      if (grants.rejected.length > 0) throw new Error("An original attachment is missing or no longer safe to read. Reattach it before launching; nothing was sent.");
                    }
                    if (epoch !== contextRequestEpoch.current || editEpoch !== composerEditEpoch.current
                        || priorContext !== draftAttachmentRefs.current.composerContext) throw new Error("The composer changed while attachments were being restored. Inspect the saved launch again; nothing was sent.");
                    setComposerSeed(current => ({ ...current, revision: current.revision + 1, prompt: receipt.input.text }));
                    setComposerContext(receipt.input.context);
                  }} />
              </>}
            />}
          </div>
        )}

        <ConversationPaneSurface
          visible={surface === "chat" && centerView === "sessions"}
          blocked={state.composerOpen || settingsOpen}>
          <div className={`workspace-grid workspace-count-${Math.max(1,
            gridKeys.length)}${
              workspace.maximizedKey !== null ? " has-maximized" : ""}`}>
            {gridKeys.map(key => {
              const summary = sessionByKey.get(key);
              if (summary === undefined) return null;
              const tileLive = liveKeys.current.has(key);
              const log = state.events[key] ?? NO_EVENTS;
              // ONE view of this session's model, effort and access facts, read
              // by the pane AND by the pane header's access selector: the same
              // call over the same inputs tileConversation makes for the pane,
              // so the header cannot disagree with the status line under it
              // about what is running.
              const tileChoices = choicesFor(summary.engine);
              const paneThread = threadIndex.get(key);
              const tileConfig = sessionConfigFor(key, summary, log,
                tileChoices.modelOptions ?? NO_MODELS,
                engineLabels[summary.engine] ?? providerLabel(summary.engine),
                launchEffort, pending, configErrors, modelPresets,
                tileChoices.permissionModes,
                tileChoices.dangerousModes ?? DANGEROUS_MODES,
                permissionMode[key] ?? launchPermission[key] ?? "default",
                permissionPending[key] === true,
                configTokens.inFlightFor(key, "fastmode"),
                permissionMode[key]);
              return (
                <AgentTile key={key} session={summary} selected={key === selectedKey}
                  maximized={workspace.maximizedKey === key} live={tileLive}
                  engineLabel={engineLabels[summary.engine] ?? providerLabel(summary.engine)}
                  projectLabel={threadTileProject(key, summary)}
                  titleSource={threadIndex.get(key)?.titleSource}
                  projectRoot={threadProjectRoot(summary, threadIndex.get(key))}
                  onFocus={() => { focusWorkspaceTile(key); }}
                  onToggleMaximize={() => { toggleAgentMaximized(key); }}
                  onClose={() => { closeWorkspaceTile(key); }}
                  onSplit={() => { splitThreadTile(summary); }}
                  {...(surface === "chat" ? {
                    onOpenInTerminal: () => { openThreadInTerminal(key); },
                    terminalRefusal: threadTerminalRefusal(summary),
                    providers: {
                      offers: paneThread === undefined ? NO_THREAD_OFFERS
                        : threadOffersFor(summary.engine, isProjectThread(paneThread)),
                      refusal: threadBridgeWait(summary.status),
                      onSecondOpinion: engine => { askSecondOpinion(key, engine); },
                      onSwitchTo: engine => { switchThreadProvider(key, engine); },
                    } satisfies ThreadProviderBridges,
                    ...(paneThread === undefined ? {} : {
                      onSchedule: () => { schedules.openDialog(key, summary.title); },
                      // Protocol 67: starts `/btw ` in the thread's composer.
                      ...(sideQuestionOffered(summary.engine) ? {
                        onSideQuestion: () => {
                          const draft = conversationDrafts.get(key);
                          if (sideQuestionCommand(draft) === null) {
                            conversationDrafts.set(key, `/btw ${draft.trim()}`.trimEnd() + " ");
                          }
                        },
                      } : {}),
                    }),
                  } : {})}
                  splitDisabled={!mayOpenWorkspaceKeyWithReservation(
                    workspace, "pending-split",
                    surface === "chat" ? workspacePaneReservation.current : 0)}
                  {...threadTileHead(key)}>
                  <div className="tile-stack">
                  {tileConversation(key, summary, centerView === "sessions"
                    && !state.composerOpen && !settingsOpen && !commandPaletteOpen
                    && key === workspace.focusedKey, tileConfig, true)}
                  </div>
                </AgentTile>
              );
            })}

            {gridKeys.length === 0 && surface !== "code" && (
          // viewsWelcome: a sentence, plus a single command button for the
          // zero-session case only. THE app's welcome — the sidebar deliberately
          // renders no CTA of its own, because with zero sessions both panes
          // showed the same sentence over the same accent button, side by side,
          // as the first screen a new user ever sees. The sidebar keeps the
          // quiet status line; the primary action lives here, on the big
          // surface, and the part header's "+" is the other way in.
          // `agent-empty` is the Agent surface's centred zero-state layout
          // (theme.css); every surface's welcome sits the same way.
          <div className="empty workspace-hero agent-empty">
            {/* "Select a session" is only true while there is one to select.
                The sidebar's default view lists the chats started here, so on a
                first run it can be empty while the machine is full of terminal
                sessions — this pane must not then invite a click on a list with
                no rows in it.

                THE DEFECT THIS REPLACES: the sentence used to be chosen from
                `state.sessions.length` alone and never read the hidden counts,
                so with two short Interface-started sessions the DECLUTTER rule
                emptied the list and this pane still read "No chats started in
                Interface yet." — false, and pointing at the wrong control.
                Both panes now derive from the same two counts (and the same
                search state) in the view-model, where the sentences are tested,
                so they cannot contradict each other or the list. */}
            <InterfaceMark className="empty-mark" />
            {groups.length === 0 ? (() => {
              const empty = sidebarEmptyState(view.hidden,
                { searching: view.searching, hiddenMatches: view.hiddenMatches });
              return (
                <>
                  <h2 className="workspace-welcome-title">{empty.action === "new" ? SURFACE_INTRO[surface].title : "Find your next session"}</h2>
                  <p className="empty-text">
                    {empty.action === "new" ? SURFACE_INTRO[surface].description : empty.text}
                  </p>
                  {empty.hint !== null && <p className="empty-hint">{empty.hint}</p>}
                  <div className="empty-actions">
                    {empty.action === "show-all" && (
                      <button type="button" className="btn btn-primary" onClick={toggleFilters}>
                        Show all sessions
                      </button>
                    )}
                    {empty.action === "clear-search" && (
                      <button
                        type="button"
                        className="btn btn-primary"
                        onClick={() => { setQuery(""); }}
                      >
                        Clear search
                      </button>
                    )}
                    {/* The other door out of the commonest empty pane of all:
                        the machine is full of sessions this app has not claimed
                        yet. One click marks them all.

                        KEYED TO `importable`, NOT to `hidden.external`. That
                        count is zero the moment the origin filter is off, so
                        the button used to vanish from both this pane and the
                        filter row exactly when the user had clicked "Show all"
                        to look at those sessions before importing them —
                        leaving no bulk import anywhere in the app, reachable
                        again only by clicking "Hide 87". It also counted the
                        wrong rows: the filter never hides a non-idle or
                        already-opened session, but the import marks every one
                        of them, so the label understated its own work. */}
                    {view.importable > 0 && (
                      <button
                        type="button"
                        className="btn"
                        disabled={importing}
                        title={`Mark the ${view.importable} sessions on this machine that ${productLabel()} has not claimed yet. Nothing is copied or moved.`}
                        onClick={importAll}
                      >
                        {importing ? "Importing…" : `Import ${view.importable} existing`}
                      </button>
                    )}
                    <button
                      type="button"
                      className={empty.action === "new" ? "btn btn-primary" : "btn"}
                      onClick={openComposer}
                    >
                      {SURFACE_NEW_LABEL[surface]}
                    </button>
                  </div>
                </>
              );
            })() : (
              <>
                <p className="empty-wordmark">Sessions</p>
                <p className="empty-text">Select a session</p>
              </>
            )}
          </div>
        )}
          </div>
        </ConversationPaneSurface>

        <div className={surface === "agent" && centerView === "sessions"
          ? "workspace-surface" : "workspace-surface is-hidden"}
          inert={state.composerOpen || settingsOpen ? true : undefined}
          aria-hidden={state.composerOpen || settingsOpen || undefined}>
          <PaneVisibleContext.Provider value={surface === "agent" && centerView === "sessions"}>
            {agentTaskPane}
          </PaneVisibleContext.Provider>
        </div>

        <div className={surface === "code" && centerView === "terminal"
          ? "workspace-surface" : "workspace-surface is-hidden"}
          inert={state.composerOpen || settingsOpen ? true : undefined}
          aria-hidden={state.composerOpen || settingsOpen || undefined}>
          <IntegratedTerminalCenter
            api={window.iface}
            outputFeedbackTargets={outputFeedbackTargets}
            onSendOutputFeedback={sendOutputFeedback}
            onAddBrowserNote={addBrowserNote}
            onAddTerminalSelection={addTerminalSelection}
            ref={terminalCenter}
            headroom={headrooms}
            activeCwd={approvedDefaultDirectory}
            visible={surface === "code" && centerView === "terminal"
              && !state.composerOpen && !settingsOpen}
            interactionBlocked={overlayDialogOpen}
            onTerminalsChange={acceptTerminalSummaries}
            onFocusedTerminalChange={acceptFocusedTerminal}
            onSwitcherChange={setTerminalSwitcher}
            toolbarSlot={terminalToolbarSlot}
            onMenuChange={setTerminalMenu}
            reserveAgentSessionPanes={reserveAdversarialReviewPanes}
            releaseAgentSessionPanes={releaseAdversarialReviewPanes}
            onOpenAgentSessions={openAdversarialReviewSessions}
            onError={setActionError}
            onOpenWorkspaceTool={openWorkspaceTool}
            onInspectTask={inspectTerminalTask}
            onContinueAsThread={continueAsThread}
            onOpenTasks={openTasksFromTerminals}
          />
        </div>

        <div className={surface === "code" && centerView === "canvas"
          ? "workspace-surface canvas-stack" : "workspace-surface canvas-stack is-hidden"}
          inert={state.composerOpen || settingsOpen ? true : undefined}
          aria-hidden={state.composerOpen || settingsOpen || undefined}>
          <AgentCanvas
            error={orchestrationError}
            snapshot={orchestrationSnapshot}
            sessions={canvasSessions}
            projectRoot={activeCanvasRoot}
            projectRoots={canvasProjectRoots}
            onSelectProjectRoot={setCanvasRoot}
            onBrowseProjectRoot={browse}
            layout={canvasLayout}
            onLayoutChange={changeCanvasLayout}
            onRegisterSession={orchestration.registerSession}
            onCreateWorker={orchestration.createAgent}
            onBindParent={orchestration.bindParent}
            onSetPolicy={orchestration.setPolicy}
            onClearPolicy={orchestration.clearPolicy}
            onStopAgent={orchestration.stopAgent}
            onSubmitWorker={orchestration.submitWorker}
            onIntegrateWorker={orchestration.integrateWorker}
            onAbandonTask={orchestration.abandonTask}
            onAckObligation={orchestration.ackObligation}
            coordination={coordinationStatus}
            models={orchestratorModels}
            onOpenSession={openCanvasSession}
          />
          {/* Cross-model consult/handoff used to hang off the Plans surface.
              The canvas is the app's remaining orchestration destination, so it
              is where the coordination console lives now. */}
          <div className="canvas-coordination">{coordinationPanel}</div>
        </div>

        </div>
      </main>

      {settingsOpen && (
        /* Settings fills the window under the toolbar (owner, 2026-11-15): its
           own menu takes the sidebar's place and the content is one centred
           column. Mounted beside the shell, not inside it, so it covers the
           sidebar too; the toolbar row stays, with its ✕. Escape, that ✕ and
           Back to app are the ways out. */
        <div className="workspace-settings-overlay" data-overlay-surface=""
          inert={shellInert || undefined} aria-hidden={shellInert || undefined}>
          <SettingsPage
            initialSection={settingsSection}
            onRemoteAccess={manageRemoteAccess}
            appearance={appearance}
            appearancePending={appearancePending}
            appearanceError={appearanceError}
            onAppearanceChange={changeAppearancePreference}
            onSystemThemesChange={setSystemThemes}
            onTranslucencyChange={setTranslucency}
            onWorkspacePresentationChange={setWorkspacePresentation}
            preferences={appPreferences}
            preferencesError={preferencesError}
            onPreferencesChange={changeAppPreferences}
            engineHealth={settingsEngineHealth}
            providersRefreshing={refreshing}
            providerRefreshError={refreshError}
            onRefreshProviders={() => { void refreshAll("explicit"); }}
            acpProfiles={acpProfiles}
            acpProfilesPending={acpProfilesPending}
            acpProfilesError={acpProfilesError}
            acpProfilesRestartRequired={acpProfilesRestartRequired}
            acpProfilesSavedRevision={acpProfilesSavedRevision}
            onSaveAcpProfiles={saveAcpProfiles}
            customizationRoot={surfaceSession !== null && surface !== "chat"
              ? surfaceSession.cwd : undefined}
            customizationApi={window.iface}
            threadSettings={threads.view?.settings ?? null}
            threadSettingsPending={threadSettingsPending}
            threadSettingsError={threadSettingsError}
            onThreadSettingsChange={changeThreadSettings}
            threadSchedules={schedules.settings}
            threadSecrets={{
              rows: heldSecretRows(threadSecrets.view, scheduleTitle, Date.now()),
              pending: threadSecrets.pending, error: threadSecrets.error,
              onShown: threadSecrets.refresh,
              onForget: row => { threadSecrets.forget(row.key, row.name); },
            }}
            updateState={updateState}
            onCheckForUpdates={checkForUpdates}
            onRestartToUpdate={requestRestartToUpdate}
            onSignIn={openSignIn}
            providerAccounts={providerAccounts.state}
            onProviderAccountsChange={providerAccounts.change}
            onAccountSignIn={openAccountSignIn}
            onProvidersShown={providerAccounts.open}
            onRevealDataFolder={revealDataFolder}
            terminalTools={terminalTools}
            onPaneToolsChange={changePaneTools}
            paneToolsPending={paneToolsPending}
            paneToolsError={paneToolsError}
            onClose={closeSettings}
          />
        </div>
      )}

      {/* The strip earns its row only when a fault, folder, voice state or
          limit is on it (owner, 2026-09-12); the version sits under Settings. */}
      {(selectedLimit !== undefined
        || (voice !== null && voiceState !== "off")
        || (surface !== "chat" && surfaceSession?.cwd !== undefined && surfaceSession.cwd !== "")) && <footer className="statusbar"
        inert={behindSettings || undefined}
        aria-hidden={behindSettings || undefined}>
        <div className="statusbar-cluster">
          {/* A provider fault lives under Settings in the sidebar footer
              (owner, 2026-09-28), not alone in this strip. */}
          {surface !== "chat" && surfaceSession?.cwd !== undefined
            && surfaceSession.cwd !== "" && (
            <span className="statusbar-item statusbar-text" title={surfaceSession.cwd}>
              <span className="codicon codicon-folder" aria-hidden="true" />
              {pathLeaf(surfaceSession.cwd)}
            </span>
          )}
          {/* ONE attention indicator, and it is the toolbar's amber pill. The
              statusbar used to grow a robot-glyph twin here the moment that
              pill vanished, counting a different population (everything not
              idle, including failed) with a different glyph in a different bar
              for the same destination. Two controls for one fact taught the
              user that the indicator moves. */}
          {/* The assistant's whole state in one strip: what it is doing, how
              loud the room is while it listens, the words it thinks it heard,
              and the sentence explaining a failure. Off draws nothing — an
              indicator for a feature nobody turned on is just noise. */}
          {voice !== null && voiceState !== "off" && (
            <span className={`statusbar-item statusbar-text voice-indicator is-${voiceState}`}
              role="status"
              title={voice.message ?? undefined}>
              {/* The live mic while it listens or thinks; the speaker only while
                  it is actually talking. */}
              <span className={voiceState === "speaking" ? "codicon codicon-unmute"
                : "codicon codicon-mic-filled"} aria-hidden="true" />
              <span>{VOICE_STATE_TEXT[voiceState]}</span>
              {voiceState === "listening" && (
                <span className="voice-meter" aria-hidden="true">
                  {VOICE_METER_BARS.map(bar => (
                    <span key={bar}
                      className={bar <= voiceLevelBars ? "voice-bar is-lit" : "voice-bar"} />
                  ))}
                </span>
              )}
              {voiceState === "listening" && voice.partial !== null && voice.partial !== "" && (
                <span className="voice-partial">{voice.partial}</span>
              )}
              {/* Any sentence the host attaches, not only a failure: `notify`
                  arrives as a message on whatever state is current, and a
                  notification nobody can see is not one. The indicator's own
                  is-error colour is what marks a failure. */}
              {voice.message !== null && voice.message !== "" && (
                <span className="voice-message">{voice.message}</span>
              )}
            </span>
          )}
        </div>
        <div className="statusbar-cluster">
          {selectedLimit !== undefined && (
            <button type="button" className="statusbar-item statusbar-warning"
              aria-label="Selected provider is rate limited" title={selectedLimit}
              onClick={() => { setUsageOpen(true); }}>
              <span className="codicon codicon-warning" aria-hidden="true" />
              <span>Rate limited</span>
            </button>
          )}
        </div>
      </footer>}

      {/* THE THIRD COLUMN. Rendered after the workspace shell and outside
          .workspace-grid, so it is chrome rather than a pane: the tile cap
          stays four and ordinary navigation still never increases the pane
          count.

          Wrapped rather than rendered bare because the panel is presentational
          and owns no shell concerns — the wrapper is the grid child, carries
          the same inert/aria-hidden rules <main> carries, and is what the
          narrow-window fallback positions. */}
      <RetainedPresence open={detailOpen && subagentDetail !== null} phase={detail.phase} render={phase => subagentDetail !== null && (
        <div className={`detail-pane${presenceClass(phase)}`}
          inert={shellInert || phase === "leaving" || undefined}
          aria-hidden={shellInert || phase === "leaving" || undefined}
          onAnimationEnd={detail.onAnimationEnd}>
          <SubagentPanel
            agent={detailView?.agent}
            // Said, not silently closed: the agentKey stopped resolving in a
            // transcript that is still open, which the user is owed a
            // sentence about. A CLOSED tile clears the selection instead.
            missing={detailView?.agent === undefined}
            missingLabel={subagentNameRef.current}
            prompt={detailView?.prompt}
            parentType={detailParent?.subagentType}
            // The same root the transcript rows shorten against, for the same
            // reason: a 69-character absolute path is 46 characters of noise.
            pathRoot={detailSession?.projectRoot ?? detailSession?.cwd}
            focusOnOpen={subagentDetail.viaKeyboard}
            focusRequest={subagentDetail.focusRequest}
            onClose={closeSubagentDetail}
            onReveal={revealSubagentDetail}
            onSelectParent={detailParent === undefined ? undefined
              : () => { retargetSubagentDetail(detailParent.id); }}
            onSelectChild={retargetSubagentDetail}
          />
        </div>
      )} />
      <RetainedPresence open={threadDiffOpen} phase={threadDiffPresence.phase} render={phase =>
        threadDiffThread !== undefined && threadDiff.target !== null && (
        <div className={`detail-pane${presenceClass(phase)}`}
          inert={shellInert || phase === "leaving" || undefined}
          aria-hidden={shellInert || phase === "leaving" || undefined}
          onAnimationEnd={threadDiffPresence.onAnimationEnd}>
          <ThreadDiffPanel
            thread={threadDiffThread}
            title={sessionByKey.get(threadDiffThread.key)?.title ?? "Thread"}
            choice={threadDiff.target.choice}
            load={threadDiff.load}
            pathRoot={sessionByKey.get(threadDiffThread.key)?.cwd}
            focusOnOpen={threadDiff.target.focus}
            onChoose={threadDiff.choose}
            onRefresh={threadDiff.refresh}
            onClose={closeThreadDiffPanel}
            onPreview={path => { previewThreadFile(threadDiffThread.key, path); }}
            onBrowseOutputs={() => { previewThreadFile(threadDiffThread.key, null); }}
            onQuote={quote => { quoteDiffIntoDraft(threadDiffThread.key, quote); }}
            ship={isProjectThread(threadDiffThread) ? {
              state: threadShip.stateOf(threadDiffThread.key),
              commitRequest: shipCommitRequest,
              onRefresh: () => { threadShip.refresh(threadDiffThread.key, true); },
              onReviewCommit: () => { void threadShip.reviewCommit(threadDiffThread.key); },
              onCommit: (message, revision, choices) => { void threadShip.commitSelected(threadDiffThread.key, message, revision, choices); },
              onCreateBranch: branch => { void threadShip.createBranch(threadDiffThread.key, branch); },
              onOpenPullRequest: () => { void threadShip.openPullRequest(threadDiffThread.key); },
              // Protocol 67: Push & open PR's form, and Write with AI beside its text.
              pullRequest: {
                draft: () => threadShip.pullRequestDraft(threadDiffThread.key),
                onOpen: (title, body) => { void threadShip.openPullRequestWith(threadDiffThread.key, title, body); },
              },
              writeWithAi: {
                blocked: writeWithAiBlocked(state.engineHealth),
                write: (what, paths) => threadShip.suggestText(threadDiffThread.key, what, paths),
              },
              onFixChecks: () => { void threadShip.fixChecks(threadDiffThread.key); },
              onBringComments: () => { void threadShip.bringComments(threadDiffThread.key); },
              onMerge: (number, method) => { void threadShip.merge(threadDiffThread.key, number, method); },
              onDismiss: () => { threadShip.dismiss(threadDiffThread.key); },
              ...(watchOffered(threadDiffThread) ? {
                watch: {
                  on: threadDiffThread.schedules?.watch?.enabled === true,
                  title: watchTitle(threadDiffThread.schedules?.watch),
                  onToggle: (on: boolean) => { schedules.setWatch(threadDiffThread.key, on); },
                },
              } : {}),
            } : undefined}
            // The Undo for that turn. A local thread's is the transcript's
            // own, found through the row its pane reported: same restore
            // point, same refusal while the agent runs, and none where the
            // transcript would offer none. An owned worktree thread restores
            // its files through Rust instead, from the turn's own start
            // checkpoint; a turn Rust recorded none for offers nothing.
            turnUndo={ordinal => {
              const key = threadDiffThread.key;
              if (threadDiffThread.workspace?.mode === "worktree") {
                const turn = threadDiffThread.turns.find(each => each.ordinal === ordinal);
                return turn?.hasStart === true ? {
                  point: threadFilesRestorePoint(ordinal),
                  busy: threadPaneBusy.has(key),
                  onUndo: () => { restoreThreadTurnFiles(key, ordinal); },
                } : null;
              }
              const found = turnRestoreForOrdinal(ordinal, threadTurnRows[key], turnRestorePoints[key]);
              return found === null ? null : {
                point: found.point,
                busy: restoringTurn !== null,
                onUndo: () => { restoreTurnCheckpoint(key, found.turnKey); },
              };
            }}
          />
        </div>
      )} />

      <RetainedPresence open={sideOpen} phase={sidePresence.phase} render={phase =>
        sideParent?.side !== undefined && sideParentKey !== null && (
        <div className={`detail-pane${presenceClass(phase)}`}
          inert={shellInert || phase === "leaving" || undefined}
          aria-hidden={shellInert || phase === "leaving" || undefined}
          onAnimationEnd={sidePresence.onAnimationEnd}>
          <SideQuestionPanel side={sideParent.side}
            answer={sideAnswer(state.events[sideParent.side.key] ?? NO_SIDE_EVENTS)}
            closing={sideClosing}
            onBringBack={() => {
              const side = sideParent.side;
              if (side === undefined) return;
              const answer = sideAnswer(state.events[side.key] ?? NO_SIDE_EVENTS).text;
              const write = bringBackSide(conversationDrafts.get(sideParentKey), side, answer);
              if (write !== null) conversationDrafts.set(sideParentKey, write.text);
              closeSideQuestion(sideParentKey);
            }}
            onClose={() => { closeSideQuestion(sideParentKey); }} />
        </div>
      )} />

      {threadPreview !== null && (
        <div className={`detail-pane thread-preview-pane${presenceClass(threadPreviewPresence.phase)}`}
          style={threadPreviewPresence.present ? undefined : { display: "none" }}
          inert={shellInert || !threadPreviewOpen || undefined}
          aria-hidden={shellInert || !threadPreviewOpen || undefined}
          onAnimationEnd={threadPreviewPresence.onAnimationEnd}>
          <OutputPreviewPanel root={threadPreview.root} visible={threadPreviewOpen} api={threadPreviewApi}
            onClose={closeThreadPreview} openRequest={threadPreview.request}
            initialThreadKey={threadPreview.key ?? undefined}
            feedbackTargets={outputFeedbackTargets} onSendFeedback={sendOutputFeedback}
            onOpenFolder={openPreviewFolder} />
        </div>
      )}

      <CommandPalette
        open={commandPaletteOpen}
        query={commandQuery}
        commands={paletteAllCommands}
        mode={quickOpen.open ? "files" : "all"}
        files={quickOpen.files}
        filesLoading={quickOpen.loading}
        filesError={quickOpen.error}
        filesTruncated={quickOpen.truncated}
        onQuery={setCommandQuery}
        onRun={runPaletteCommand}
        onOpenFile={openQuickOpenFile}
        onPreviewFile={quickOpenPreviewKey === null ? undefined : previewQuickOpenFile}
        onClose={closeQuickOpen}
      />
      <SessionLibrary
        open={sessionLibraryOpen}
        tab={sessionLibraryTab}
        onTabChange={tab => {
          setSessionLibraryTab(tab);
          setSessionLibraryNotice(null);
        }}
        query={sessionSearchQuery}
        onQueryChange={setSessionSearchQuery}
        onSearch={searchSessionLibrary}
        searchHits={sessionSearchHits}
        searchLoading={sessionSearchLoading}
        searchError={sessionSearchError}
        searchIncomplete={sessionSearchIncomplete}
        archived={archivedSessions}
        archivedLoading={archivedSessionsLoading}
        worktrees={managedWorktrees}
        worktreesLoading={managedWorktreesLoading}
        notice={sessionLibraryNotice}
        onRefresh={() => {
          void refreshSessionLibrary();
          if (sessionLibraryTab === "search" && sessionSearchQuery.trim() !== "") {
            searchSessionLibrary();
          }
        }}
        onOpenSession={openSessionFromLibrary}
        onRestoreSession={restoreSessionFromLibrary}
        onOpenWorktree={openWorktreeFromLibrary}
        onOpenTerminal={openTerminalForWorktree}
        onCleanWorktree={cleanupWorktreeFromLibrary}
        onClose={closeSessionLibrary}
      />
      <ThreadReferenceDialog
        open={threadReferenceTarget !== null}
        choices={threadReferenceTarget === null ? [] : threadReferenceChoices(threads.view,
          state.sessions, threadReferenceTarget).map(choice => ({
          key: choice.key, title: choice.title,
          meta: [choice.provider, threadAgeLabel(choice.lastActivityAt, Date.now())]
            .filter(Boolean).join(" · "),
        }))}
        onCancel={() => { setThreadReferenceTarget(null); }}
        onChoose={key => {
          const target = threadReferenceTarget;
          if (target === null) return;
          const choice = threadReferenceChoices(threads.view, state.sessions, target)
            .find(candidate => candidate.key === key);
          if (choice !== undefined) addContextFor(target, threadReferenceItem(choice, rendererItemId("chat")));
          setThreadReferenceTarget(null);
        }}
      />
      <ContextValueDialog
        open={contextValueTarget !== null}
        onCancel={() => { setContextValueTarget(null); }}
        onSubmit={value => {
          const target = contextValueTarget;
          if (target === null) return;
          addContextFor(target, {
            id: rendererItemId("url"), kind: "url", label: value, value,
          });
          setContextValueTarget(null);
        }}
      />
    </div>
  );
}
