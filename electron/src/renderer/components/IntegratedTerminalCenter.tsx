import {
  OutputPreviewPanel,
  type OutputPreviewOpenRequest,
  type OutputPreviewPanelProps,
} from "./OutputPreviewPanel";
import { attachmentPreviewTarget } from "../output-preview";
import { resolveTerminalLinkTarget } from "../terminal-links";
import { BROWSER_COLUMN_ID, BrowserColumn, BrowserPane, type BrowserAddToDraft } from "./BrowserPane";
import type { AgentOverviewTab } from "./AgentOverview";
import { browserPageUrl, browserServerUrls, browserTabTitle } from "../browser-pane";
import { BrowserPage, BrowserPages, type BrowserPageEvents } from "../browser-page";
import type { BrowserEventV1 } from "../../shared/browser-events";
import { focusTerminalViewIfCurrent } from "../terminal-view-focus";
import { terminalPaneSplitOrientation, type WorkspaceTool } from "./TerminalPane";
import { BROWSER_PROFILE_ID } from "./TerminalMenu";
import { TERMINAL_FAST_MODE_KEY } from "../../shared/ipc";
import { terminalCardPreview } from "../terminal-card-preview";
import { usePresence } from "../presence";
import {
  memo, useCallback, useEffect, useImperativeHandle, useLayoutEffect, useMemo, useRef, useState,
  type ReactNode, type Ref,
} from "react";
import { equalValue, retainEqualItems } from "../stable-values";
import { useCommittedActions } from "../use-stable-value";
import type { SessionKey } from "../../shared/engine";
import type {
  CreateTerminalRequest, IfaceApi, MovedTerminalFolder,
  TerminalDataEvent, TerminalHandoffV1, TerminalLaunchProfile, TerminalLaunchProfileId,
  TerminalLaunchRequestV1, TerminalLaunchSessionV1, TerminalLifecycleEvent, TerminalSummary,
  TerminalWorkspaceRecord,
} from "../../shared/ipc";
import { newPaneChordAllowedAt, terminalCenterKeyAction } from "../terminal-keys";
import { passOffRefusal, passOffTitle, passOffTranscript } from "../pass-off";
import { continueAsThreadKey } from "../thread-terminal-bridge";
import { productLabel } from "../product";
import {
  EMPTY_TERMINAL_WORKSPACE_STATE, MAX_TERMINAL_PANES_HARD, MAX_TERMINAL_TABS_PER_PANE,
  attachTerminalsBalanced, browserTabIdsInWorkspaceState, browserTabPaneId, browserTabUrl,
  closeTerminalTab, cycleTerminalFocus, focusTerminalPane,
  focusTerminalTab, isBrowserTabId, newBrowserTabId, onceTerminalIds,
  openBrowserTabBeside, openBrowserTabInPane, openTerminalTab, parseTerminalWorkspaceState, placePassedOffTerminal, savedLayoutFacts,
  setBrowserTabUrl,
  reconcileActiveTerminalWorkspaceState, replaceTerminalTab, serializeTerminalWorkspaceState,
  setTerminalSplitRatio, setTerminalWorkspaceArrangement, splitTerminalPaneInPlace,
  terminalIdsInWorkspaceState, terminalPaneById,
  terminalPaneForTerminal, tidyTerminalWorkspace,
  toggleTerminalPaneMaximized,
  type TerminalPaneId, type TerminalSplitOrientation, type TerminalSplitRequest,
  type TerminalWorkspaceState,
} from "../terminal-workspace";
import { dropTerminalPane } from "../terminal-pane-drag";
import { splitDroppedFiles } from "../prompt-drop";
import { terminalImagePasteInput, terminalPathInsertPrefix } from "../prompt-attach";
import type { PastedFile } from "../prompt-paste";
import { clearTerminalDone, isTerminalProtocolReply, markTerminalDone, withTerminalActivity,
  type TerminalActivitySummary, type TerminalLiveActivity } from "../terminal-activity";
import { TerminalActivityTracker } from "../terminal-activity-tracker";
import {
  MAX_TERMINAL_WORKSPACES,
  reconcileSavedWorkspaceNames, forgetClosedTabs, forgetTerminal, forgetWorkspace,
  neighboringWorkspaceId, newChatBinding,
  initialTerminalWorkspaceRecords, newTerminalWorkspace,
  normalizeTerminalWorkspaces, normalizeWorkspaceName,
  reconcileLiveTerminalWorkspaceBindings,
  rememberLayout, rememberTerminal, reopenRequests,
  failedReopenTerminals, movedFolderPath, paneStartDirectory, relocateTerminalWorkspaces,
  shellFallbackRequests,
  shouldPersistAuthoritativeTerminalIdentity, terminalSwitcherRecords,
  visibleWorkspaceTerminals, workspaceById, workspaceDeleteConfirm,
  workspaceForTerminal,
} from "../terminal-workspaces";
import { reopenWithRetry } from "../terminal-reopen-retry";
import { attachedTerminalIds, sweepHiddenTerminals } from "../terminal-hidden-sweep";
import {
  applyTerminalLifecycleEvent, applyTerminalRegistrySnapshot,
  mergeTerminalSummary, pruneTerminalOutputBuffers,
  terminalOutputStreamKey, terminalRedrawKickSize,
  type TerminalRegistryState,
} from "../terminal-output-state";
import { TerminalStreamRouter } from "../terminal-stream-router";
import { placeHandedOffShell, terminalRestartOffer } from "../terminal-shell-handoff";
import { TerminalResizeQueue } from "../terminal-resize-queue";
import { TerminalRedrawPulseQueue } from "../terminal-redraw-pulse";
import {
  pruneTerminalProfileMemory, readTerminalProfileMemory, writeTerminalProfileMemory,
} from "../terminal-profile-memory";
import { TerminalWorkspaceActivityQueue } from "../terminal-workspace-activity";
import { TerminalWorkspacePersistenceQueue } from "../terminal-workspace-persistence";
import { AdversarialReviewDialog } from "./AdversarialReviewDialog";
import { ConfirmDialog } from "./ConfirmDialog";
import {
  TERMINAL_PANE_LOCATE_MS,
  type TerminalPaneLocate, type TerminalPaneRenderContext, type TerminalPaneTerminal,
} from "./TerminalPane";
import {
  agentOverviewRows, agentOverviewSince, agentOverviewStatus, agentOverviewWorkspace,
  isAgentProfile, savedAgentOverviewWorkspace,
  type AgentOverviewRow, type AgentOverviewSince, type AgentOverviewTerminal,
} from "../agent-overview";
import { providerLabel } from "../provider-labels";
import { TerminalRenameDialog } from "./TerminalRenameDialog";
import { TerminalReopenFailure } from "./TerminalReopenFailure";
import { TerminalView, type TerminalViewHandle } from "./TerminalView";
import { TerminalSelectionDialog } from "./ComposerReferenceDialogs";
import { TerminalViewPool, TerminalViewSlot } from "./TerminalViewPool";
import {
  TerminalWorkspace, terminalLauncherShowing,
  type TerminalMenuModel, type TerminalWorkspaceCallbacks, type TerminalWorkspaceSwitcher,
  type TerminalWorkspaceView,
} from "./TerminalWorkspace";
import {
  TERMINAL_LAUNCH_COUNTS, terminalLauncherChosenCwd, terminalLauncherDefaultCwd,
  type TerminalAdversarialReviewLaunchRequest,
  type TerminalLauncherNotice,
} from "./TerminalLauncher";
import { messageFor as failureText } from "./panel-text";

const TERMINAL_LAYOUT_KEY = "interface.integrated-terminals.layout.v1";
const ACTIVE_WORKSPACE_KEY = "interface.integrated-terminals.workspace.v1";
const AGENT_OVERVIEW_KEY = "interface.integrated-terminals.agent-overview.v1";
const AGENT_OVERVIEW_TAB_KEY = "interface.integrated-terminals.agent-overview-tab.v1";
/** Below this window width the overview floats over the panes (theme.css's
 *  side-sheet breakpoint), so choosing an agent also closes it. */
const AGENT_OVERVIEW_FLOATS_QUERY = "(max-width: 1080px)";
// Shared with App.tsx's composer: one last-used folder for New Chat and a new
// terminal workspace. Presentation preference, not a host grant.
const LAST_CWD_KEY = "iface.lastCwd";
/** Coalesce a burst of layout drags into one host write. */
const WORKSPACE_SAVE_DELAY_MS = 400;
// OpenTUI (OpenCode) debounces SIGWINCH for 100 ms. Animation output is not
// acknowledgement of that resize: restoring at 75 ms coalesces both sizes into
// a no-op, leaving a fresh xterm with only delta frames. Hold the kick beyond
// that debounce, with headroom, before restoring the fitted geometry.
const TERMINAL_REDRAW_MIN_DELAY_MS = 250;
const TERMINAL_REDRAW_OUTPUT_TIMEOUT_MS = 500;
// The only two option sets a pane ever has. Kept as constants because a fresh
// literal per render re-runs TerminalView's options effect and reassigns
// xterm's options on every frame of a stream.
const TERMINAL_OPTIONS_RUNNING = { disableStdin: false };
const TERMINAL_OPTIONS_STOPPED = { disableStdin: true };

/** Each inspection walks a process tree; a workspace rarely runs more servers. */
const MAX_BROWSER_SERVER_TERMINALS = 8;

export type TerminalRendererApi = Pick<IfaceApi,
  "closeTerminal" | "createTerminal" | "dismissTerminal" | "getTerminalWorkspaces"
  | "listTerminalLaunchProfiles" | "listTerminals" | "onTerminalData"
  | "onTerminalDataFailure" | "onTerminalLifecycle" | "pathForFile" | "pickDirectory"
  | "resizeTerminal" | "savePastedImages"
  | "sendTerminalInput" | "setTerminalWorkspaces" | "startAdversarialReview">
  & Partial<Pick<IfaceApi, "resolveMovedTerminalFolder"
    | "setTerminalWorkspaceActivity" | "previewOutput" | "inspectTerminalServers"
    | "openWorkspaceTool" | "restartTerminal" | "visualFrame">>;

interface IntegratedTerminalCenterProps {
  /** Each provider's plan headroom, for the launcher's tiles (review E). */
  headroom?: Readonly<Record<string, { readonly text: string; readonly title: string }>>;
  readonly outputFeedbackTargets?: OutputPreviewPanelProps["feedbackTargets"];
  readonly onSendOutputFeedback?: OutputPreviewPanelProps["onSendFeedback"];
  /** A Browser capture's note and picture into a thread's draft (point and fix). */
  readonly onAddBrowserNote?: BrowserAddToDraft;
  /**
   * A terminal selection into a thread's draft (T3 section 9): the thread,
   * the pane's title and the selected text. False when nothing was added.
   */
  readonly onAddTerminalSelection?: (key: string, title: string, selection: string) => boolean;
  /** Stable transport for this mounted surface; App owns the preload boundary. */
  readonly api: TerminalRendererApi;
  readonly activeCwd: string | null;
  readonly visible: boolean;
  /** An app-level modal owns focus even though the terminal surface remains mounted. */
  readonly interactionBlocked?: boolean;
  readonly onError: (message: string) => void;
  readonly onOpenWorkspaceTool?: (tool: WorkspaceTool, cwd: string) => void;
  readonly onInspectTask?: (terminalId: string) => void;
  /** The agent overview's Tasks tab: the Agent surface's task list. */
  readonly onOpenTasks?: () => void;
  /**
   * Continue as thread (review C1), after this surface ended the pane: bring
   * `sessionKey` (the pane's provider session) into Threads.
   */
  readonly onContinueAsThread?: (sessionKey: string) => void;
  readonly reserveAgentSessionPanes?: (count: number) => boolean;
  readonly releaseAgentSessionPanes?: (count: number) => void;
  readonly onOpenAgentSessions?: (
    builder: SessionKey, reviewer: SessionKey, objective: string,
  ) => boolean;
  readonly onTerminalsChange?: (terminals: readonly TerminalActivitySummary[]) => void;
  readonly onFocusedTerminalChange?: (terminalId: string | null) => void;
  /**
   * Publishes the saved-workspace switcher so the shell's left navigation owns
   * it. The center surface deliberately draws no workspace bar of its own —
   * workspaces are a destination in the sidebar, not chrome above the panes.
   */
  readonly onSwitcherChange?: (switcher: TerminalWorkspaceSwitcher | null) => void;
  /**
   * The window toolbar element this surface draws its own controls into (the
   * Output preview toggle). App owns the element; the surface owns the
   * controls and their state. Null while the terminals are not showing.
   */
  readonly toolbarSlot?: HTMLElement | null;
  /** Publishes the terminal layout section of the window's ··· menu. */
  readonly onMenuChange?: (menu: TerminalMenuModel | null) => void;
  /** Lets the command palette reach the one action that has no chrome of its own. */
  readonly ref?: Ref<IntegratedTerminalCenterHandle>;
}

export interface IntegratedTerminalCenterHandle {
  readonly controlProject: () => string | null;
  /** What main says about a Browser page (App's `onBrowserEvent`), handed to the page it names. */
  readonly browserEvent: (event: BrowserEventV1) => void;
  /**
   * ⌘W: close the selected pane's active tab exactly as its × does (a
   * terminal ends, a Browser page closes). False when no pane is selected.
   */
  readonly closeFocusedPane: () => boolean;
  /** Open the adversarial-review dialog from outside this surface. */
  readonly openReviewDialog: () => void;
  /**
   * Bring one terminal to the front from outside this surface: switch to the
   * workspace that holds it, then focus its tab. The attention tray's rows
   * land here.
   */
  readonly focusTerminal: (terminalId: string) => void;
  /**
   * Open `url` in a Browser pane beside this terminal's pane, in the
   * workspace that holds it. False when there is no such terminal.
   */
  readonly openBrowser: (url: string, terminalId: string) => boolean;
  /**
   * The command palette's Files and Browser: the output preview for the
   * focused project, or a Browser tab in the focused pane — what the pane
   * `+` menu opens. False when there is no project to open it for.
   */
  readonly openTool: (tool: "files" | "browser") => boolean;
  /**
   * Open in terminal (review C1): a new workspace whose one pane resumes a
   * provider session Rust has handed over (`openInTerminal`).
   */
  readonly openResumedSession: (input: {
    readonly cwd: string;
    readonly profileId: "claude" | "codex";
    readonly providerSessionId: string;
    readonly title?: string;
  }) => void;
  /**
   * A new workspace whose one pane runs `profileId`'s own sign-in command in
   * `cwd` (protocol 41; Rust owns the argv). Settings' and a failed turn's
   * Sign in land here.
   */
  /** `account` (protocol 60) signs in that provider account; absent = the provider's default. */
  readonly openSignIn: (profileId: TerminalLaunchProfileId, label: string, cwd: string,
    account?: string, title?: string) => void;
}

/** A new pane halves its source along the longer side of the pane as drawn. */
function drawnPaneSplitOrientation(paneId: TerminalPaneId | null): TerminalSplitOrientation {
  const pane = paneId === null ? null
    : document.querySelector<HTMLElement>(`[data-terminal-pane-id="${CSS.escape(paneId)}"]`);
  const bounds = pane?.getBoundingClientRect();
  return terminalPaneSplitOrientation(bounds?.width ?? 0, bounds?.height ?? 0);
}

function readLayout(): TerminalWorkspaceState {
  try {
    return parseTerminalWorkspaceState(window.localStorage.getItem(TERMINAL_LAYOUT_KEY));
  } catch {
    return EMPTY_TERMINAL_WORKSPACE_STATE;
  }
}

function readActiveWorkspaceId(): string | null {
  try {
    return window.localStorage.getItem(ACTIVE_WORKSPACE_KEY);
  } catch {
    return null;
  }
}

function writeActiveWorkspaceId(id: string): void {
  try {
    window.localStorage.setItem(ACTIVE_WORKSPACE_KEY, id);
  } catch {
    // Which workspace was last open is a convenience. A full storage area must
    // not make a running terminal unusable.
  }
}

/** Closed until first opened: the column takes width from every pane. */
/** Two tail snapshots that draw the same cards. */
function sameTails(a: ReadonlyMap<string, readonly string[]>,
    b: ReadonlyMap<string, readonly string[]>): boolean {
  if (a.size !== b.size) return false;
  for (const [id, lines] of b) {
    const before = a.get(id);
    if (before === undefined || before.length !== lines.length
      || before.some((line, at) => line !== lines[at])) return false;
  }
  return true;
}

function readAgentOverviewOpen(): boolean {
  try {
    return window.localStorage.getItem(AGENT_OVERVIEW_KEY) === "open";
  } catch {
    return false;
  }
}

function writeAgentOverviewOpen(open: boolean): void {
  try {
    window.localStorage.setItem(AGENT_OVERVIEW_KEY, open ? "open" : "closed");
  } catch {
    // A panel preference; losing it must not break the terminals.
  }
}

function readAgentOverviewTab(): AgentOverviewTab {
  try {
    return window.localStorage.getItem(AGENT_OVERVIEW_TAB_KEY) === "browser" ? "browser" : "agents";
  } catch {
    return "agents";
  }
}

function writeAgentOverviewTab(tab: AgentOverviewTab): void {
  try {
    window.localStorage.setItem(AGENT_OVERVIEW_TAB_KEY, tab);
  } catch {
    // A panel preference; losing it must not break the terminals.
  }
}

function readLastCwd(): string | null {
  try {
    const stored = window.localStorage.getItem(LAST_CWD_KEY);
    return typeof stored === "string" && stored.trim() !== "" ? stored : null;
  } catch {
    return null;
  }
}

function writeLastCwd(path: string): void {
  try {
    window.localStorage.setItem(LAST_CWD_KEY, path);
  } catch {
    // Last-used folder is a convenience. A full storage area must not block
    // launching a workspace.
  }
}

function readInheritedTerminalProfiles(): ReadonlyMap<string, TerminalLaunchProfileId> {
  try {
    return readTerminalProfileMemory(window.localStorage);
  } catch {
    return new Map();
  }
}

function writeInheritedTerminalProfiles(
    profiles: ReadonlyMap<string, TerminalLaunchProfileId>): void {
  try {
    writeTerminalProfileMemory(window.localStorage, profiles);
  } catch {
    // Accessing localStorage itself can fail in a locked-down renderer. The
    // live icon memory remains usable for the lifetime of this component.
  }
}

function stamp(): string {
  return new Date().toISOString();
}

function messageFor(error: unknown): string {
  return failureText(error, `${productLabel()} could not complete the terminal operation.`);
}

function shellQuote(path: string): string {
  return `'${path.replaceAll("'", `'\\''`)}'`;
}

/**
 * Images on the clipboard for the toolbar Paste button, which has no paste
 * event to read. Absent or refused clipboard access is not an error worth
 * reporting: the button simply has nothing to paste.
 */
async function clipboardImages(): Promise<PastedFile[]> {
  try {
    const items = await navigator.clipboard.read();
    const images: PastedFile[] = [];
    for (const item of items) {
      const type = item.types.find(candidate => candidate.startsWith("image/"));
      if (type !== undefined) images.push(await item.getType(type));
    }
    return images;
  } catch {
    return [];
  }
}

function binaryBytes(value: string): Uint8Array {
  const bytes = new Uint8Array(value.length);
  for (let index = 0; index < value.length; index += 1) bytes[index] = value.charCodeAt(index) & 0xff;
  return bytes;
}

/** The focused pane's shown tab: a terminal, or a Browser page. */
function focusedTabId(state: TerminalWorkspaceState): string | null {
  if (state.focusedPaneId === null) return null;
  return terminalPaneById(state, state.focusedPaneId)?.activeTerminalId ?? null;
}

/** The focused terminal. A focused Browser tab is not one. */
function activeTerminalId(state: TerminalWorkspaceState): string | null {
  const id = focusedTabId(state);
  return id === null || isBrowserTabId(id) ? null : id;
}

/** What a view calls back into the center with: one object for the center's
 *  life, whose members reach the current registry, queues and handles. */
type TerminalViewActions = {
  readonly sendBytes: (terminal: TerminalSummary, bytes: Uint8Array) => boolean;
  readonly scheduleResize: (terminal: TerminalSummary, size: { columns: number; rows: number }) => void;
  readonly insertPaths: (terminal: TerminalSummary, located: readonly string[],
    files: readonly PastedFile[], source: "clipboard" | "drop") => Promise<void>;
  readonly pathForFile: (file: { name: string }) => string;
  readonly clearReplay: (key: string) => void;
  readonly attachView: (terminal: TerminalSummary, handle: TerminalViewHandle | null) => void;
  readonly openTerminalLink: (terminal: TerminalSummary, link: string) => void;
};

/** A pooled view learns whether its terminal is mid-turn from the pool. */
type TerminalViewRenderContext = TerminalPaneRenderContext & { readonly working?: boolean };

const viewTextEncoder = new TextEncoder();

/**
 * One terminal generation's emulator, in the pool.
 * It redraws for its own summary, activity and visibility only: a render of
 * the center for any other terminal, a splitter frame or a focus click leaves
 * every emulator alone.
 */
const PooledTerminalView = memo(function PooledTerminalView({ terminal, active,
    working, actions }: {
  readonly terminal: TerminalSummary;
  readonly active: boolean;
  readonly working: boolean;
  readonly actions: TerminalViewActions;
}) {
  const { sendBytes, scheduleResize, insertPaths, pathForFile, clearReplay, attachView, openTerminalLink } = actions;
  const key = terminalOutputStreamKey(terminal.id, terminal.generation);
  const engine = terminal.launch?.profileId;
  return <TerminalView terminalId={key}
    cursorProfile={engine === 'codex' || engine === 'claude' ? engine : undefined}
    working={working}
    active={active}
    ariaLabel={`${terminal.title} terminal`}
    initialSize={terminal.size}
    cwd={terminal.cwd}
    onOpenLink={link => { openTerminalLink(terminal, link); }}
    options={terminal.state === "running"
      ? TERMINAL_OPTIONS_RUNNING : TERMINAL_OPTIONS_STOPPED}
    onData={data => { sendBytes(terminal, viewTextEncoder.encode(data)); }}
    onBinary={data => { sendBytes(terminal, binaryBytes(data)); }}
    onResize={size => { scheduleResize(terminal, size); }}
    onCopy={text => navigator.clipboard.writeText(text)}
    onPasteRequest={async () => {
      const text = await navigator.clipboard.readText();
      if (text !== "") return text;
      const images = await clipboardImages();
      if (images.length === 0) return null;
      void insertPaths(terminal, [], images, "clipboard");
      return null;
    }}
    onPasteFiles={files => { void insertPaths(terminal, [], files, "clipboard"); }}
    onDrop={drop => {
      const { paths, images } = splitDroppedFiles(drop.files, pathForFile);
      // Dropped text never arrives here: the view pastes it (terminal-drop.ts).
      if (paths.length === 0 && images.length === 0) return;
      void insertPaths(terminal, paths, images, "drop");
    }}
    onClear={() => { clearReplay(key); }}
    onReady={handle => { attachView(terminal, handle); }} />;
});

export function IntegratedTerminalCenter({ api, outputFeedbackTargets, onSendOutputFeedback,
    onAddBrowserNote, onAddTerminalSelection, activeCwd, visible,
    interactionBlocked = false,
    onError, onTerminalsChange, onOpenWorkspaceTool, onInspectTask, onOpenTasks, onContinueAsThread,
    onFocusedTerminalChange, reserveAgentSessionPanes, releaseAgentSessionPanes,
    onOpenAgentSessions, onSwitcherChange, toolbarSlot = null,
    onMenuChange, headroom,
    ref }: IntegratedTerminalCenterProps) {
  const [registry, setRegistry] = useState<TerminalRegistryState>({ revision: 0, terminals: [] });
  const [registryLoaded, setRegistryLoaded] = useState(false);
  const [layout, setLayout] = useState<TerminalWorkspaceState>(EMPTY_TERMINAL_WORKSPACE_STATE);
  const [inheritedProfileByTerminal, setInheritedProfileByTerminal] = useState<
    ReadonlyMap<string, TerminalLaunchProfileId>
  >(readInheritedTerminalProfiles);
  const [activityByStream, setActivityByStream] = useState<
    ReadonlyMap<string, TerminalLiveActivity>
  >(
    () => new Map());
  const [completedTurnsByStream, setCompletedTurnsByStream] = useState<ReadonlyMap<string, number>>(
    () => new Map());
  // The blue done dot: agents that finished a turn while their pane was not
  // the focused one. The focused stream is read through a ref because the
  // activity tracker's callbacks are created once.
  const [doneStreams, setDoneStreams] = useState<ReadonlySet<string>>(() => new Set());
  const focusedStreamRef = useRef<string | null>(null);
  const [launchProfiles, setLaunchProfiles] = useState<readonly TerminalLaunchProfile[]>([]);
  const [launchProfilesLoading, setLaunchProfilesLoading] = useState(true);
  const [launcherOpen, setLauncherOpen] = useState(false);
  const [launcherBusy, setLauncherBusy] = useState(false);
  const [launcherNotice, setLauncherNotice] = useState<TerminalLauncherNotice | null>(null);
  const [reviewOpen, setReviewOpen] = useState(false);
  // The folder the launcher is showing. It lives here, not in the launcher, so
  // the review dialog cannot name a different one: contextualCwd prefers the
  // FOCUSED terminal's folder, so a folder chosen with Change… was visible in
  // the launcher and invisible to the dialog beside it.
  //
  // A BROWSE BELONGS TO ONE LAUNCHER/DIALOG SESSION. This component never
  // unmounts, where the launcher's own state used to die on close, so every
  // boundary clears it by hand: opening the launcher, closing it, a successful
  // launch, a started or cancelled review, and a workspace switch. The effect
  // on contextualCwd below is not enough on its own — a focused terminal holds
  // that value byte-identical, so browsing to /project-B from a pane in
  // /project-A and closing left /project-B waiting for the next, unrelated
  // launcher.
  const [browsedCwd, setBrowsedCwd] = useState<string | null>(null);
  // The launcher unmounts on close, so the agent and the count it was last
  // launched with are remembered out here beside the folder. Session-only: this
  // is a convenience, not a durable preference.
  const [lastProfileId, setLastProfileId] = useState<TerminalLaunchProfileId | null>(null);
  const [lastCount, setLastCount] = useState(1);
  const [workspaceView, setWorkspaceView] = useState<TerminalWorkspaceView>("split");
  const [previews, setPreviews] = useState<ReadonlyMap<string, readonly string[]>>(new Map());
  // Saved terminals the host refused to reopen, by id, with its reason. Their
  // tabs stay in the layout and would otherwise sit as empty, forever-starting
  // panes; instead they read as failed, and closing the tab is the way out.
  const [reopenFailures, setReopenFailures] = useState<ReadonlyMap<string, string>>(new Map());
  /** Failed saved panes whose folder moved and left a link behind, by terminal. */
  const [movedFolders, setMovedFolders] = useState<ReadonlyMap<string, MovedTerminalFolder>>(new Map());
  const unmounted = useRef(false);
  useEffect(() => {
    unmounted.current = false;
    return () => { unmounted.current = true; };
  }, []);
  const [workspaces, setWorkspaces] = useState<readonly TerminalWorkspaceRecord[]>([]);
  const [activeWorkspaceId, setActiveWorkspaceId] = useState<string | null>(readActiveWorkspaceId);
  const [workspacesLoaded, setWorkspacesLoaded] = useState(false);
  const [workspaceRenameId, setWorkspaceRenameId] = useState<string | null>(null);
  const [deleteWorkspaceId, setDeleteWorkspaceId] = useState<string | null>(null);
  const [lastSelectedCwd, setLastSelectedCwd] = useState(readLastCwd);
  const [overviewOpen, setOverviewOpen] = useState(readAgentOverviewOpen);
  const [overviewTab, setOverviewTab] = useState(readAgentOverviewTab);
  const [outputRoots, setOutputRoots] = useState<Readonly<Record<string, string>>>({});
  const [outputOpenRequest, setOutputOpenRequest] = useState<OutputPreviewOpenRequest | null>(null);
  const outputOpenNonce = useRef(0);
  const previewApi = useMemo(() => api.previewOutput
    ? { previewOutput: api.previewOutput, visualFrame: api.visualFrame } : null, [api]);
  const outputRoot = activeWorkspaceId ? outputRoots[activeWorkspaceId] ?? null : null;
  // The preview slides in and out when toggled. A workspace switch shows or
  // hides it in place with the panes, since each workspace keeps its own.
  const [previewWorkspaceId, setPreviewWorkspaceId] = useState(activeWorkspaceId);
  const switchedWorkspace = previewWorkspaceId !== activeWorkspaceId;
  if (switchedWorkspace) setPreviewWorkspaceId(activeWorkspaceId);
  const previewPresence = usePresence(outputRoot !== null, switchedWorkspace);
  // Leaving, it keeps showing the project it showed.
  const lastOutputRoot = useRef(outputRoot);
  useLayoutEffect(() => { if (outputRoot !== null) lastOutputRoot.current = outputRoot; });
  const shownOutputRoot = outputRoot ?? (previewPresence.phase === "leaving" ? lastOutputRoot.current : null);
  const closeOutputPreview = useCallback(() => {
    const id = activeWorkspaceIdRef.current;
    if (id) setOutputRoots(values => { const next = { ...values }; delete next[id]; return next; });
  }, []);
  const openOutputPreview = useCallback((cwd: string) => {
    const id = activeWorkspaceIdRef.current;
    if (!id) return;
    setOutputRoots(values => ({ ...values, [id]: cwd }));
    setOverviewOpen(false);
    writeAgentOverviewOpen(false);
  }, []);
  const openOutputFolder = useCallback((cwd: string) => {
    onOpenWorkspaceTool?.("files", cwd);
  }, [onOpenWorkspaceTool]);
  const { inspectTerminalServers, openWorkspaceTool } = api;
  // Browser tabs are panes' tabs like terminals. Their pages live here, out
  // of the panes, so a pane that moves never reloads its page; their events
  // reach the layout through this ref, which later code fills in.
  const browserEvents = useRef<(tabId: string) => BrowserPageEvents>(() => ({
    navigated: () => undefined, titled: () => undefined, focused: () => undefined,
  }));
  const [browserPages] = useState(() => new BrowserPages(tabId => browserEvents.current(tabId)));
  useEffect(() => () => { browserPages.dispose(); }, [browserPages]);
  /** What each open page last said it was: its tab's label. */
  const [browserLabels, setBrowserLabels] = useState<ReadonlyMap<string, {
    readonly title: string | null; readonly url: string | null;
  }>>(() => new Map());
  /** Dev servers each Browser tab offers, as last found. */
  const [browserServers, setBrowserServers] = useState<ReadonlyMap<string, readonly string[]>>(
    () => new Map());
  /** Tabs whose terminals are being inspected now. */
  const browserServerLookups = useRef(new Set<string>());
  /** The folder a Browser was opened from, whose servers it looks at first. */
  const browserOrigins = useRef(new Map<string, string>());
  const [locate, setLocate] = useState<TerminalPaneLocate | null>(null);
  const locateNonce = useRef(0);
  const registryRef = useRef(registry);
  const workspacesRef = useRef(workspaces);
  const workspacesLoadedRef = useRef(false);
  const activeWorkspaceIdRef = useRef(activeWorkspaceId);
  // Lifecycle upserts can beat createTerminal's promise. Keep the intended
  // workspace available until the initial response has filed the terminal.
  const pendingWorkspaceByTerminal = useRef(new Map<string, string>());
  // Every terminal id this renderer minted or seated. A lifecycle upsert for
  // any other id with `passedOffFrom` is a pane an agent's pass_off tool
  // created, which nothing else will place in the layout.
  const mintedTerminalIds = useRef(new Set<string>());
  const reopened = useRef(new Set<string>());
  const layoutRef = useRef(layout);
  const launchProfilesRef = useRef(launchProfiles);
  const terminalSurfaceVisibleRef = useRef(visible);
  const errorRef = useRef(onError);
  // Every stream-keyed output fact: mounted views and their unacknowledged
  // writes, the replay tail a remount restores, byte cursors, sleeping screens
  // and retired terminals. PTYs do not replay old bytes, so the tail outlives
  // any React remount, including a saved workspace switch.
  const [streams] = useState(() => new TerminalStreamRouter<TerminalViewHandle>());
  const [documentVisible, setDocumentVisible] = useState(() => document.visibilityState !== "hidden");
  const retirePending = useRef(new Set<string>());
  const launchProfileRequest = useRef(0);
  const launchPending = useRef(false);
  const textEncoder = useMemo(() => new TextEncoder(), []);
  const resizeQueue = useMemo(() => new TerminalResizeQueue(75, error => {
    errorRef.current(messageFor(error));
  }), []);
  const redrawPulse = useMemo(() => new TerminalRedrawPulseQueue(
    TERMINAL_REDRAW_MIN_DELAY_MS,
    TERMINAL_REDRAW_OUTPUT_TIMEOUT_MS,
    error => { errorRef.current(messageFor(error)); },
  ), []);
  const workspaceActivity = useMemo(() => new TerminalWorkspaceActivityQueue(
    async request => api.setTerminalWorkspaceActivity?.(request),
    error => { errorRef.current(messageFor(error)); },
  ), []);
  useEffect(() => {
    const changed = (): void => { setDocumentVisible(document.visibilityState !== "hidden"); };
    document.addEventListener("visibilitychange", changed);
    return () => {
      document.removeEventListener("visibilitychange", changed);
      workspaceActivity.dispose();
    };
  }, [workspaceActivity]);
  const workspacePersistence = useMemo(() => new TerminalWorkspacePersistenceQueue(
    WORKSPACE_SAVE_DELAY_MS,
    async (submitted, moves) => {
      const saved = await api.setTerminalWorkspaces(submitted,
        moves.length === 0 ? undefined : moves);
      const current = workspacesRef.current;
      const next = reconcileSavedWorkspaceNames(current, submitted, saved);
      if (next !== current) {
        workspacesRef.current = next;
        setWorkspaces(next);
      }
    },
    error => { errorRef.current(messageFor(error)); },
  ), []);
  // registryRef is written by every accept path before its setRegistry, and
  // is ahead of `registry` whenever only the revision fence moved.
  activeWorkspaceIdRef.current = activeWorkspaceId;
  layoutRef.current = layout;
  launchProfilesRef.current = launchProfiles;
  terminalSurfaceVisibleRef.current = visible && !interactionBlocked
    && workspaceView === "split" && !launcherOpen
    && workspaceRenameId === null && deleteWorkspaceId === null;
  errorRef.current = onError;

  const report = useCallback((error: unknown): void => {
    errorRef.current(messageFor(error));
  }, []);

  const forgetInheritedProfile = useCallback((terminalId: string): void => {
    setInheritedProfileByTerminal(current => {
      if (!current.has(terminalId)) return current;
      const next = new Map(current);
      next.delete(terminalId);
      return next;
    });
  }, []);

  const publishActivity = useCallback((streamKey: string, activity: TerminalLiveActivity): void => {
    // New work or a request for input means the finished turn is over.
    if (activity !== "idle") setDoneStreams(current => clearTerminalDone(current, streamKey));
    setActivityByStream(current => {
      if (current.get(streamKey) === activity) return current;
      const next = new Map(current);
      next.set(streamKey, activity);
      return next;
    });
  }, []);

  // Every output chunk asks for its stream's profile: index the registry once
  // per registry snapshot instead of scanning it and building a key per pane.
  const streamProfiles = useRef<{
    source: readonly TerminalSummary[] | null;
    byStream: Map<string, TerminalLaunchProfileId | undefined>;
  }>({ source: null, byStream: new Map() });
  const [activityTracker] = useState(() => new TerminalActivityTracker({
    profile: key => {
      const terminals = registryRef.current.terminals;
      const index = streamProfiles.current;
      if (index.source !== terminals) {
        index.source = terminals;
        index.byStream = new Map(terminals.map(terminal => [
          terminalOutputStreamKey(terminal.id, terminal.generation), terminal.launch?.profileId,
        ]));
      }
      return index.byStream.get(key);
    },
    activity: publishActivity,
    completed: (key, count) => {
      setCompletedTurnsByStream(current => new Map(current).set(key, count));
      setDoneStreams(current => markTerminalDone(current, key, focusedStreamRef.current));
    },
  }));

  const pruneActivity = useCallback((terminalId: string, minimumGeneration?: number): void => {
    const prefix = `${terminalId}:`;
    const doomed = (key: string): boolean => {
      if (!key.startsWith(prefix)) return false;
      if (minimumGeneration === undefined) return true;
      const generation = Number(key.slice(prefix.length));
      return Number.isSafeInteger(generation) && generation < minimumGeneration;
    };
    activityTracker.prune(doomed);
    setDoneStreams(current => {
      if (![...current].some(doomed)) return current;
      return new Set([...current].filter(key => !doomed(key)));
    });
    setCompletedTurnsByStream(current => {
      if (![...current.keys()].some(doomed)) return current;
      const next = new Map(current);
      pruneTerminalOutputBuffers(next, terminalId, minimumGeneration);
      return next;
    });
    setActivityByStream(current => {
      let hit = false;
      for (const key of current.keys()) {
        if (doomed(key)) {
          hit = true;
          break;
        }
      }
      if (!hit) return current;
      const next = new Map(current);
      pruneTerminalOutputBuffers(next, terminalId, minimumGeneration);
      return next;
    });
  }, [activityTracker]);

  /** The host removed a terminal, or a dismiss did: its output, activity and
   *  inherited mark end with it, and later output for it is dropped. */
  const retireTerminal = useCallback((terminalId: string): void => {
    streams.retire(terminalId);
    pruneActivity(terminalId);
    forgetInheritedProfile(terminalId);
  }, [forgetInheritedProfile, pruneActivity, streams]);

  const observeOutput = useCallback((key: string, data: Uint8Array): void => {
    activityTracker.output(key, data);
  }, [activityTracker]);
  const observeInput = useCallback((key: string): void => {
    activityTracker.input(key);
  }, [activityTracker]);
  // Rust decides which panes need the user (host protocol v44): each host
  // summary carries its answer, and the tracker publishes it as "waiting".
  useEffect(() => {
    for (const terminal of registry.terminals) {
      activityTracker.hostAttention(terminalOutputStreamKey(terminal.id, terminal.generation),
        terminal.state === "running" && terminal.attention !== undefined);
    }
  }, [activityTracker, registry.terminals]);

  // Through the ref, never the render's snapshot: every caller is a callback or
  // an event handler that must see the registry as it is now.
  const terminalById = useCallback((terminalId: string): TerminalSummary | undefined =>
    registryRef.current.terminals.find(candidate => candidate.id === terminalId), []);

  const activeWorkspace = useMemo(
    () => workspaceById(workspaces, activeWorkspaceId), [activeWorkspaceId, workspaces]);
  // Which terminals show is a fact of the panes and the saved roster alone. A
  // splitter frame or a focus click keeps both, so nothing below that reads
  // only these recomputes for it.
  const layoutPanes = layout.panes;
  const layoutTerminalIds = useMemo(
    () => terminalIdsInWorkspaceState({ panes: layoutPanes }), [layoutPanes]);
  const activeWorkspaceTerminals = activeWorkspace?.terminals;

  /**
   * Terminals this workspace owns. Rust lets background work finish and then
   * suspends idle processes; selecting the workspace resumes the same PTYs.
   * Untitled split leftovers that were then closed as tabs stay out of
   * the overview so three shells in the same folder do not look like clones.
   */
  const visibleTerminals = useMemo(() => {
    if (activeWorkspaceTerminals === undefined) return registry.terminals;
    return visibleWorkspaceTerminals(
      registry.terminals,
      new Set(activeWorkspaceTerminals.map(entry => entry.id)),
      layoutTerminalIds,
    );
  }, [activeWorkspaceTerminals, layoutTerminalIds, registry.terminals]);

  useEffect(() => {
    if (!registryLoaded || !workspacesLoaded) return;
    const active = new Set(visible && documentVisible ? visibleTerminals.map(terminal => terminal.id) : []);
    workspaceActivity.reconcile(registry.terminals, active, key => streams.screenEvidence(key));
  }, [documentVisible, registry.terminals, registryLoaded, streams, visible, visibleTerminals,
    workspacesLoaded, workspaceActivity]);

  /**
   * Update the roster and write it back, coalescing a burst of edits.
   *
   * A rejected write is reported but not rolled back: the in-memory roster
   * still matches what is on screen, and the next successful save carries it.
   */
  const persistWorkspaces = useCallback((mode: "now" | "soon"): void => {
    if (!workspacesLoadedRef.current) return;
    workspacePersistence.schedule(workspacesRef.current, mode);
  }, [workspacePersistence]);

  const updateWorkspaces = useCallback(
    (change: (current: readonly TerminalWorkspaceRecord[]) => readonly TerminalWorkspaceRecord[],
      persist: "now" | "soon" = "soon"): void => {
      // The ref is the serialized source of truth for native lifecycle
      // callbacks. Update it before yielding to React so a provider binding
      // arriving between `file()` and the batched render can still resolve its
      // workspace after the pending route is cleared.
      const current = workspacesRef.current;
      const next = change(current);
      if (next === current) return;
      workspacesRef.current = next;
      setWorkspaces(next);
      persistWorkspaces(persist);
    }, [persistWorkspaces]);

  /** Record a live terminal under whichever workspace is open. */
  const file = useCallback((terminal: TerminalSummary,
      binding: TerminalLaunchSessionV1 | undefined,
      workspaceId = activeWorkspaceIdRef.current,
      bindingAuthority: "monotonic" | "authoritative" = "monotonic"): void => {
    if (workspaceId === null) return;
    updateWorkspaces(current => rememberTerminal(
      current, workspaceId, terminal, binding, stamp, bindingAuthority,
    ),
      "now");
  }, [updateWorkspaces]);

  // ---- Browser tabs ------------------------------------------------------
  const activeCwdRef = useRef(activeCwd);
  activeCwdRef.current = activeCwd;
  /** The folder a workspace works in: its first saved terminal's, else the app's. */
  const workspaceRoot = useCallback((workspaceId: string | null): string | null => paneStartDirectory(
    null,
    workspaceById(workspacesRef.current, workspaceId)?.terminals.map(entry => entry.cwd) ?? [],
    activeCwdRef.current,
  ), []);

  /**
   * Ask which loopback servers the workspace's own running terminals listen
   * on, the ones in the folder the Browser came from first. A terminal that
   * cannot be inspected has none. One lookup per tab at a time.
   */
  const findBrowserServers = useCallback((tabId: string): void => {
    if (browserServerLookups.current.has(tabId)) return;
    browserServerLookups.current.add(tabId);
    const workspaceId = activeWorkspaceIdRef.current;
    const origin = browserOrigins.current.get(tabId) ?? workspaceRoot(workspaceId);
    const owned = new Set(workspaceById(workspacesRef.current, workspaceId)?.terminals
      .map(entry => entry.id) ?? []);
    const running = registryRef.current.terminals
      .filter(terminal => owned.has(terminal.id) && terminal.state === "running")
      .sort((a, b) => Number(b.cwd === origin) - Number(a.cwd === origin))
      .slice(0, MAX_BROWSER_SERVER_TERMINALS);
    void Promise.all(running.map(terminal => inspectTerminalServers?.({
      id: terminal.id, generation: terminal.generation,
    }).catch(() => []) ?? [])).then(lists => {
      browserServerLookups.current.delete(tabId);
      // A tab closed while its terminals were inspected keeps nothing.
      if (tabId !== BROWSER_COLUMN_ID && browserPages.get(tabId) === undefined) return;
      const servers = browserServerUrls(lists);
      setBrowserServers(values => values.get(tabId)?.join("\n") === servers.join("\n") ? values
        : new Map(values).set(tabId, servers));
    });
  }, [browserPages, inspectTerminalServers, workspaceRoot]);

  /** "Open in default browser": the native Browser tool, rooted in the workspace. */
  const openBrowserOutside = useCallback(async (url: string): Promise<void> => {
    const root = workspaceRoot(activeWorkspaceIdRef.current);
    if (root === null || openWorkspaceTool === undefined) {
      throw new Error("Open a project folder in this workspace first.");
    }
    await openWorkspaceTool({ version: 1, root, tool: "browser", url });
  }, [openWorkspaceTool, workspaceRoot]);

  /**
   * A Browser pane beside `beside` (a pane, or the pane holding a terminal),
   * taking half of that pane the way the pane `+` places a terminal: a link
   * to a running server opens here, so its terminal stays in view. Returns
   * its tab id.
   */
  const openBrowserPane = useCallback((beside: {
    readonly paneId?: TerminalPaneId;
    readonly terminalId?: string;
  }, cwd: string | null, url?: string): string | null => {
    if (layoutRef.current.panes.length >= MAX_TERMINAL_PANES_HARD) {
      report(`The terminal workspace supports up to ${MAX_TERMINAL_PANES_HARD.toString()} panes.`);
      return null;
    }
    const tabId = newBrowserTabId(() => crypto.randomUUID());
    if (cwd !== null) browserOrigins.current.set(tabId, cwd);
    const paneIds = onceTerminalIds(() => crypto.randomUUID());
    const splitIds = onceTerminalIds(() => crypto.randomUUID());
    const tidyIds = onceTerminalIds(() => crypto.randomUUID());
    const layoutNow = layoutRef.current;
    const drawnSource = beside.paneId ?? (beside.terminalId === undefined ? layoutNow.focusedPaneId
      : terminalPaneForTerminal(layoutNow, beside.terminalId)?.id ?? layoutNow.focusedPaneId);
    const orientation = drawnPaneSplitOrientation(drawnSource);
    setLayout(current => {
      const source = beside.paneId !== undefined && terminalPaneById(current, beside.paneId) !== undefined
        ? beside.paneId
        : beside.terminalId === undefined ? current.focusedPaneId
        : terminalPaneForTerminal(current, beside.terminalId)?.id ?? current.focusedPaneId;
      const placed = current.root === null
        ? openTerminalTab(current, paneIds(0), tabId)
        : source === null || terminalPaneById(current, source) === undefined
          ? attachTerminalsBalanced(current, [tabId], paneIds, splitIds)
          : splitTerminalPaneInPlace(current, {
              paneId: source, newPaneId: paneIds(0), splitId: splitIds(0), terminalId: tabId,
              orientation,
            }, tidyIds);
      return url === undefined ? placed : setBrowserTabUrl(placed, tabId, url);
    });
    setLauncherOpen(false);
    return tabId;
  }, [report]);

  /**
   * A new Browser tab in `paneId` (else the focused pane), shown with its
   * address bar ready; nothing else in the layout moves (owner, 2026-11-15).
   * The pane `+` menu's Browser, the palette's, and New Tab over a page.
   * `cwd` is the folder it came from; without it, the showing page's. False
   * at the pane's tab limit, which it says.
   */
  const openBrowserTab = useCallback((paneId: TerminalPaneId | null, cwd?: string | null): boolean => {
    const target = browserTabPaneId(layoutRef.current, paneId);
    const pane = target === null ? undefined : terminalPaneById(layoutRef.current, target);
    if (pane !== undefined && pane.tabs.length >= MAX_TERMINAL_TABS_PER_PANE) {
      report(`A pane holds up to ${MAX_TERMINAL_TABS_PER_PANE.toString()} tabs.`);
      return false;
    }
    const tabId = newBrowserTabId(() => crypto.randomUUID());
    const origin = cwd ?? (pane === undefined ? undefined : browserOrigins.current.get(pane.activeTerminalId));
    if (origin !== undefined && origin !== null) browserOrigins.current.set(tabId, origin);
    const paneIds = onceTerminalIds(() => crypto.randomUUID());
    setLayout(current => openBrowserTabInPane(current, paneId, tabId, paneIds(0)));
    setLauncherOpen(false);
    return true;
  }, [report]);

  /**
   * Keep the page a Browser tab shows with its tab, so the workspace reopens
   * on it. A page in a workspace that is not showing is saved into that
   * workspace's record.
   */
  const rememberBrowserUrl = useCallback((tabId: string, url: string): void => {
    if (terminalPaneForTerminal(layoutRef.current, tabId) !== undefined) {
      setLayout(current => setBrowserTabUrl(current, tabId, url));
      return;
    }
    updateWorkspaces(current => {
      let changed = false;
      const next = current.map(record => {
        if (record.id === activeWorkspaceIdRef.current || record.layout === undefined) return record;
        const saved = parseTerminalWorkspaceState(record.layout);
        const updated = setBrowserTabUrl(saved, tabId, url);
        if (updated === saved) return record;
        changed = true;
        return { ...record, layout: serializeTerminalWorkspaceState(updated), updatedAt: stamp() };
      });
      return changed ? next : current;
    });
  }, [updateWorkspaces]);

  const labelBrowser = useCallback((tabId: string,
      change: { readonly title?: string | null; readonly url?: string | null }): void => {
    setBrowserLabels(values => {
      const previous = values.get(tabId) ?? { title: null, url: null };
      const next = { ...previous, ...change };
      if (values.has(tabId) && previous.title === next.title && previous.url === next.url) return values;
      return new Map(values).set(tabId, next);
    });
  }, []);
  browserEvents.current = tabId => ({
    navigated: url => {
      labelBrowser(tabId, { url });
      rememberBrowserUrl(tabId, url);
    },
    titled: title => { labelBrowser(tabId, { title: title === "" ? null : title }); },
    // Clicking into a page focuses its pane, as clicking into a terminal does.
    focused: () => { setLayout(current => focusTerminalTab(current, tabId)); },
    // A link that wants a new tab: another page right after this one, in its pane.
    openTab: (url, background) => {
      const pane = terminalPaneForTerminal(layoutRef.current, tabId);
      if (pane === undefined) return;
      if (pane.tabs.length >= MAX_TERMINAL_TABS_PER_PANE) {
        report(`A pane holds up to ${MAX_TERMINAL_TABS_PER_PANE.toString()} tabs.`);
        return;
      }
      const opened = newBrowserTabId(() => crypto.randomUUID());
      const origin = browserOrigins.current.get(tabId);
      if (origin !== undefined) browserOrigins.current.set(opened, origin);
      setLayout(current => openBrowserTabBeside(current, tabId, opened, url, background));
    },
  });

  // A page lives while its tab is in any workspace's layout: the showing
  // workspace's live panes, or another's saved arrangement.
  useEffect(() => {
    const kept = browserTabIdsInWorkspaceState({ panes: layout.panes });
    for (const record of workspaces) {
      if (record.id === activeWorkspaceId || record.layout === undefined) continue;
      for (const id of savedLayoutFacts(record.layout).browserTabIds) kept.add(id);
    }
    // The overview's Browser is no layout tab; its servers go only on a switch.
    kept.add(BROWSER_COLUMN_ID);
    browserPages.retain(kept);
    const prune = <T,>(values: ReadonlyMap<string, T>): ReadonlyMap<string, T> =>
      [...values.keys()].every(id => kept.has(id)) ? values
        : new Map([...values].filter(([id]) => kept.has(id)));
    setBrowserLabels(prune);
    setBrowserServers(prune);
    for (const id of browserOrigins.current.keys()) if (!kept.has(id)) browserOrigins.current.delete(id);
  }, [activeWorkspaceId, browserPages, layout.panes, workspaces]);

  // The overview's Browser offers the showing workspace's servers: looked for
  // again after a switch, since the last ones belonged to another workspace.
  useEffect(() => {
    setBrowserServers(values => {
      if (!values.has(BROWSER_COLUMN_ID)) return values;
      const next = new Map(values);
      next.delete(BROWSER_COLUMN_ID);
      return next;
    });
  }, [activeWorkspaceId]);
  /** Where the overview's Browser last was, so reopening the column returns there. */
  const overviewBrowserUrl = useRef<string | undefined>(undefined);
  const rememberOverviewBrowserUrl = useCallback((url: string): void => { overviewBrowserUrl.current = url; }, []);

  // A dragged pane head or tab passes over pages, so it can land on the pane
  // beneath one. Capture: the tab's own handler stops the event.
  useEffect(() => {
    const start = (event: DragEvent): void => {
      if (event.target instanceof Element && event.target.closest(".terminal-pane-head") !== null) {
        browserPages.setDragging(true);
      }
    };
    const end = (): void => { browserPages.setDragging(false); };
    window.addEventListener("dragstart", start, true);
    window.addEventListener("dragend", end, true);
    window.addEventListener("drop", end, true);
    return () => {
      window.removeEventListener("dragstart", start, true);
      window.removeEventListener("dragend", end, true);
      window.removeEventListener("drop", end, true);
    };
  }, [browserPages]);

  /**
   * Ask the host which launchers exist. The launcher says it is checking while
   * it asks. A pane menu opened over a list already drawn asks `quiet`ly, and
   * an unchanged answer keeps the list's identity, so the terminal tree does
   * not redraw and the open menu does not place itself again.
   */
  // `refresh`: the host looks the provider CLIs up again first (Try again).
  const requestLaunchProfiles = useCallback((quiet: boolean, refresh = false): void => {
    const request = launchProfileRequest.current + 1;
    launchProfileRequest.current = request;
    if (!quiet || launchProfilesRef.current.length === 0) setLaunchProfilesLoading(true);
    if (!launchPending.current) setLauncherNotice(null);
    void api.listTerminalLaunchProfiles(refresh ? { refresh: true } : undefined).then(profiles => {
      if (launchProfileRequest.current !== request) return;
      setLaunchProfiles(current => equalValue(current, profiles) ? current : profiles);
    }).catch(error => {
      if (launchProfileRequest.current !== request) return;
      setLaunchProfiles([]);
      setLauncherNotice({
        kind: "error",
        message: `${messageFor(error)} Interface did not attempt to launch a terminal.`,
      });
    }).finally(() => {
      if (launchProfileRequest.current === request) setLaunchProfilesLoading(false);
    });
  }, []);
  const loadLaunchProfiles = useCallback((): void => {
    requestLaunchProfiles(false);
  }, [requestLaunchProfiles]);
  const refreshLaunchProfiles = useCallback((): void => {
    requestLaunchProfiles(true);
  }, [requestLaunchProfiles]);
  // Try again: a CLI installed since Interface started appears (protocol 41).
  const retryLaunchProfiles = useCallback((): void => {
    requestLaunchProfiles(false, true);
  }, [requestLaunchProfiles]);

  /**
   * Seat a pane the host created and file it into its workspace,
   * exactly once: the id joins the minted set so the terminal's later upserts
   * are ordinary. A pass-off sits beside the pane it came from (in that
   * source pane's workspace, shown or parked); a voice or phone start_pane
   * joins the active layout balanced, rather than running where no pane shows it.
   */
  const seatHostTerminal = useCallback((terminal: TerminalSummary): void => {
    const source = terminal.passedOffFrom;
    mintedTerminalIds.current.add(terminal.id);
    const paneIds = onceTerminalIds(() => crypto.randomUUID());
    const splitIds = onceTerminalIds(() => crypto.randomUUID());
    const tidyIds = onceTerminalIds(() => crypto.randomUUID());
    const sourceWorkspace = source === undefined
      ? undefined
      : workspaceForTerminal(workspacesRef.current, source);
    const targetWorkspaceId = sourceWorkspace?.id ?? activeWorkspaceIdRef.current;
    if (source !== undefined && sourceWorkspace !== undefined
        && sourceWorkspace.id !== activeWorkspaceIdRef.current) {
      updateWorkspaces(current => {
        const placed = current.map(record => {
          if (record.id !== sourceWorkspace.id) return record;
          const saved = parseTerminalWorkspaceState(record.layout ?? null);
          const updated = placePassedOffTerminal(
            saved,
            { terminalId: terminal.id, sourceTerminalId: source },
            paneIds,
            splitIds,
            tidyIds,
          );
          return updated === saved ? record : {
            ...record,
            layout: serializeTerminalWorkspaceState(updated),
            updatedAt: stamp(),
          };
        });
        return rememberTerminal(placed, sourceWorkspace.id, terminal, terminal.launchSession, stamp);
      }, "now");
      return;
    }
    setLayout(current => source === undefined
      ? attachTerminalsBalanced(current, [terminal.id], paneIds, splitIds)
      : placePassedOffTerminal(
        current,
        { terminalId: terminal.id, sourceTerminalId: source },
        paneIds,
        splitIds,
        tidyIds,
      ));
    file(terminal, terminal.launchSession, targetWorkspaceId);
  }, [file, updateWorkspaces]);

  const acceptSummary = useCallback((terminal: TerminalSummary): void => {
    const currentRegistry = registryRef.current;
    const terminals = mergeTerminalSummary(currentRegistry.terminals, terminal);
    if (terminals === currentRegistry.terminals) return;
    const nextRegistry = { ...currentRegistry, terminals };
    registryRef.current = nextRegistry;
    setRegistry(nextRegistry);
  }, []);

  const acceptRegistrySnapshot = useCallback((snapshot: TerminalRegistryState): void => {
    const currentRegistry = registryRef.current;
    const nextRegistry = applyTerminalRegistrySnapshot(currentRegistry, snapshot);
    if (nextRegistry === currentRegistry) return;
    registryRef.current = nextRegistry;
    setRegistry(nextRegistry);
    if (workspacesLoadedRef.current) {
      // A snapshot can observe a host mutation before its same-revision push
      // reaches this independently scheduled renderer channel. Persist the
      // provider identity from either accepted path; the later lifecycle event
      // is correctly revision-fenced and becomes a no-op.
      updateWorkspaces(current => reconcileLiveTerminalWorkspaceBindings(
        current, nextRegistry.terminals, stamp,
      ), "now");
    }
  }, [updateWorkspaces]);

  const acceptLifecycle = useCallback((event: TerminalLifecycleEvent): void => {
    const currentRegistry = registryRef.current;
    const application = applyTerminalLifecycleEvent(currentRegistry, event);
    const nextRegistry = application.registry;
    // Filing is an authoritative persistence side effect. A lifecycle event
    // rejected by the revision fence must be entirely inert, not merely absent
    // from the rendered registry.
    if (nextRegistry === currentRegistry) return;
    registryRef.current = nextRegistry;
    // An echo equal to the held row (a resize reply and its push) only moves
    // the revision fence, which lives in the ref: nothing drawn has changed.
    if (nextRegistry.terminals !== currentRegistry.terminals) setRegistry(nextRegistry);
    // A newer global revision can still carry an older generation or summary
    // than an independently completed control response. Advance the revision,
    // but keep every persistence/handoff side effect fenced to the
    // payload that actually won the per-terminal merge.
    if (!application.payloadApplied) return;
    if (event.kind === "removed") {
      pendingWorkspaceByTerminal.current.delete(event.id);
      mintedTerminalIds.current.delete(event.id);
      retireTerminal(event.id);
      return;
    }
    // A launched terminal first seen here, after the roster loaded, that this
    // renderer did not mint is a voice or phone start_pane.
    const hostStarted = workspacesLoadedRef.current && event.terminal.launch !== undefined
      && !currentRegistry.terminals.some(terminal => terminal.id === event.terminal.id);
    // A shell Rust handed an exited agent's pane (protocol 50) takes that
    // agent's tab wherever it is laid out; see the hand-off effect below.
    if (event.terminal.replaces !== undefined) {
      mintedTerminalIds.current.add(event.terminal.id);
    } else if (workspacesLoadedRef.current
        && (event.terminal.passedOffFrom !== undefined || hostStarted)
        && !mintedTerminalIds.current.has(event.terminal.id)
        && terminalPaneForTerminal(layoutRef.current, event.terminal.id) === undefined
        && workspaceForTerminal(workspacesRef.current, event.terminal.id) === undefined) {
      seatHostTerminal(event.terminal);
    }
    const binding = event.terminal.launchSession;
    const workspace = workspaceForTerminal(workspacesRef.current, event.terminal.id);
    const saved = workspace?.terminals.find(terminal => terminal.id === event.terminal.id);
    if (shouldPersistAuthoritativeTerminalIdentity(saved, event.terminal)) {
      const workspaceId = workspace?.id
        ?? pendingWorkspaceByTerminal.current.get(event.terminal.id);
      if (workspaceId !== undefined) {
        // Lifecycle state is the host's current provider truth. In particular,
        // an id-less Claude startup must clear a previously saved id until the
        // validated provider lookup reconfirms it, while a Shell demotion must
        // clear a completed manual Claude chat. Late create responses remain on
        // the default monotonic path and cannot erase later host authority.
        file(event.terminal, binding, workspaceId, "authoritative");
      }
    }
  }, [file, retireTerminal, seatHostTerminal]);

  const acceptData = useCallback((event: TerminalDataEvent, complete: () => void): void => {
    const output = streams.deliver(event, terminalById(event.terminalId), complete);
    if (output === null) return;
    redrawPulse.noteOutput(output.key, event.data, output.boundary);
    observeOutput(output.key, event.data);
  }, [observeOutput, redrawPulse, streams, terminalById]);

  // The saved roster is read beside the registry, not after it: the two are
  // independent host reads, and bringing saved panes back waits for both.
  const savedRosterRead = useRef<Promise<unknown> | null>(null);
  useEffect(() => {
    const removeLifecycle = api.onTerminalLifecycle(acceptLifecycle);
    const removeData = api.onTerminalData(acceptData);
    const removeDataFailure = api.onTerminalDataFailure(message => { report(message); });
    const listed = api.listTerminals();
    const saved = api.getTerminalWorkspaces();
    // Handled where it is used; a read nobody uses must not reject unheard.
    saved.catch(() => undefined);
    savedRosterRead.current = saved;
    void listed.then(snapshot => {
      acceptRegistrySnapshot(snapshot);
      setRegistryLoaded(true);
    }).catch(error => {
      // A registry loaded later (by a launch) reads a fresh roster instead.
      if (savedRosterRead.current === saved) savedRosterRead.current = null;
      // Do not treat an unread registry as an empty one. Workspace migration
      // is destructive if it files an invented empty snapshot over live PTYs;
      // leaving loading incomplete preserves the host roster for a safe retry
      // on the next app start.
      report(error);
    });
    return () => {
      removeLifecycle();
      removeData();
      removeDataFailure();
    };
  }, [acceptData, acceptLifecycle, acceptRegistrySnapshot, report]);

  useEffect(() => { loadLaunchProfiles(); }, [loadLaunchProfiles]);

  useEffect(() => {
    writeInheritedTerminalProfiles(inheritedProfileByTerminal);
  }, [inheritedProfileByTerminal]);

  useEffect(() => {
    if (!registryLoaded || !workspacesLoaded) return;
    const liveTerminalIds = new Set(registry.terminals.map(terminal => terminal.id));
    for (const workspace of workspaces) {
      for (const terminal of workspace.terminals) liveTerminalIds.add(terminal.id);
    }
    setInheritedProfileByTerminal(current => pruneTerminalProfileMemory(
      current, liveTerminalIds,
    ));
  }, [registry.terminals, registryLoaded, workspaces, workspacesLoaded]);

  // First load: adopt the saved roster, or mint one from whatever this install
  // already had so an upgrade does not look like a wipe.
  useEffect(() => {
    if (!registryLoaded) return;
    let cancelled = false;
    const read = savedRosterRead.current ?? api.getTerminalWorkspaces();
    savedRosterRead.current = null;
    void read
      .then(saved => normalizeTerminalWorkspaces(saved))
      .then(saved => {
        if (cancelled) return;
        const migrating = saved.length === 0;
        const seeded = initialTerminalWorkspaceRecords(
          saved,
          registryRef.current.terminals,
          serializeTerminalWorkspaceState(readLayout()),
          () => crypto.randomUUID(),
          stamp,
        );
        // A lifecycle upsert can be accepted after the listener is installed
        // but before this saved-roster read resolves. Reconcile the fenced live
        // registry now so that provider identity is durable before reopen starts.
        const reconciled = reconcileLiveTerminalWorkspaceBindings(
          seeded, registryRef.current.terminals, stamp,
        );
        const reconciledProviderBinding = reconciled !== seeded;
        // Tabs an older build closed kept their saved panes, which respawned
        // unseen on every launch. Nothing reopens them now, so let them go.
        const records = forgetClosedTabs(reconciled, registryRef.current.terminals, stamp);
        const stored = readActiveWorkspaceId();
        const active = workspaceById(records, stored) ?? records[0];
        setWorkspaces(records);
        workspacesRef.current = records;
        activeWorkspaceIdRef.current = active.id;
        setActiveWorkspaceId(active.id);
        writeActiveWorkspaceId(active.id);
        setLayout(parseTerminalWorkspaceState(active.layout ?? null));
        workspacesLoadedRef.current = true;
        setWorkspacesLoaded(true);
        // An empty saved roster after an update is usually an unreadable file,
        // not a first run. Overwriting it would destroy the previous session.
        // Only persist a migration when live terminals were actually adopted.
        if ((migrating && registryRef.current.terminals.length > 0)
            || reconciledProviderBinding || records !== reconciled) {
          workspacePersistence.schedule(records, "now");
        }
      }).catch(report);
    return () => { cancelled = true; };
  }, [registryLoaded, report, workspacePersistence]);

  /**
   * Put one saved terminal back under `workspaceId`.
   *
   * A saved pane can miss its host reopen proof for a moment right after a
   * host restart (the managed-worktree ledger is still verifying, the proof
   * sidecar is mid-settle). One CLI refusing to resume must not take the rest
   * of the workspace down with it, so each terminal reports for itself; a
   * refused plain shell still comes up as a standard terminal, only a refused
   * chat resume stays a failed tab. A failed tab whose folder has moved and
   * left a link behind learns where the link leads, so it can offer that.
   */
  const startReopen = useCallback((request: CreateTerminalRequest, workspaceId: string): void => {
    // A sibling reopening, layout save, or workspace switch replaces the active
    // record. None of those ends this saved pane's lifetime. Cancel only when
    // the center unmounts or the owner removes the saved pane/workspace.
    const cancelled = (): boolean => unmounted.current
      || !workspaceById(workspacesRef.current, workspaceId)?.terminals.some(entry => entry.id === request.id);
    // Rebuild a request from the current host roster so a request that raced
    // the host's own startup write is replaced by the identity its saved proof
    // expects before the retry. The host authority gate is unchanged and
    // re-runs on every attempt; a genuinely gone session still ends failed.
    const rebuild = async (id: string) => {
      const saved = normalizeTerminalWorkspaces(await api.getTerminalWorkspaces());
      const record = workspaceById(saved, workspaceId)
        ?? saved.find(candidate => candidate.terminals.some(entry => entry.id === id));
      return record === undefined ? undefined
        : reopenRequests(record, registryRef.current.terminals, () => crypto.randomUUID())
            .find(candidate => candidate.id === id);
    };
    pendingWorkspaceByTerminal.current.set(request.id, workspaceId);
    mintedTerminalIds.current.add(request.id);
    void reopenWithRetry(request, {
      createTerminal: candidate => api.createTerminal(candidate),
      rebuild,
      shellFallbacks: candidate => shellFallbackRequests(candidate, activeCwd),
      sleep: ms => new Promise(resolve => { window.setTimeout(resolve, ms); }),
      cancelled,
    }).then(result => {
      pendingWorkspaceByTerminal.current.delete(request.id);
      if (result.ok) {
        acceptSummary(result.terminal);
        // A legacy pane may have received its first managed handle while this
        // request was built. Persist the exact launched binding immediately so
        // the next app start resumes this run instead of minting another one.
        file(result.terminal, result.launchSession, workspaceId);
        return;
      }
      if (result.cancelled) return;
      report(result.error);
      setReopenFailures(current => new Map(current).set(request.id, messageFor(result.error)));
      api.resolveMovedTerminalFolder?.(request.cwd).then(to => {
        if (to === null || to === request.cwd) return;
        setMovedFolders(current => new Map(current).set(request.id, { from: request.cwd, to }));
      }, () => undefined);
    });
  }, [acceptSummary, activeCwd, api, file, report]);

  /**
   * Put a workspace's terminals back.
   *
   * Runs once per workspace per app run. Workspace sleep retains the same
   * processes and generations, so returning does not replay a launch request.
   */
  useEffect(() => {
    if (!registryLoaded || !workspacesLoaded || activeWorkspace === undefined) return;
    if (reopened.current.has(activeWorkspace.id)) return;
    reopened.current.add(activeWorkspace.id);
    const requests = reopenRequests(
      activeWorkspace,
      registryRef.current.terminals,
      () => crypto.randomUUID(),
    );
    if (requests.length === 0) return;
    for (const request of requests) startReopen(request, activeWorkspace.id);
  }, [activeWorkspace, registryLoaded, startReopen, workspacesLoaded]);

  /**
   * Follow a folder the owner confirmed has moved: every saved pane inside it
   * now lives in the folder it leads to. The roster and the host's restart
   * proofs move in one save, then each pane that failed there tries again.
   * The host re-proves the move, so a link that changed since it was shown
   * refuses the save and nothing moves.
   */
  const reopenInMovedFolder = useCallback((moved: MovedTerminalFolder): void => {
    const current = workspacesRef.current;
    const failed = new Set(current.flatMap(record => record.terminals)
      .filter(entry => reopenFailures.has(entry.id)
        && movedFolderPath(entry.cwd, moved.from, moved.to) !== null)
      .map(entry => entry.id));
    const next = relocateTerminalWorkspaces(current, moved.from, moved.to);
    if (next !== current) {
      workspacesRef.current = next;
      setWorkspaces(next);
    }
    workspacePersistence.saveMove(next, [moved]).then(() => {
      if (unmounted.current) return;
      const forget = <T,>(map: ReadonlyMap<string, T>): ReadonlyMap<string, T> => {
        const kept = new Map(map);
        for (const id of failed) kept.delete(id);
        return kept;
      };
      setReopenFailures(forget);
      setMovedFolders(forget);
      for (const record of workspacesRef.current) {
        for (const request of reopenRequests(record, registryRef.current.terminals,
          () => crypto.randomUUID())) {
          if (failed.has(request.id)) startReopen(request, record.id);
        }
      }
    }, (error: unknown) => { report(error); });
  }, [reopenFailures, report, startReopen, workspacePersistence]);

  const stampedTerminals = useRef<{
    source: readonly TerminalSummary[];
    value: readonly TerminalActivitySummary[];
  }>({ source: [], value: [] });
  useEffect(() => {
    if (!registryLoaded) return;
    // Re-stamp the previous result whenever the host rows have not moved, so
    // one terminal's activity transition leaves every other row's identity — and
    // the array's own, when nothing changed — alone.
    const next = stampedTerminals.current.source === registry.terminals
      ? withTerminalActivity(stampedTerminals.current.value, activityByStream, completedTurnsByStream,
        doneStreams)
      : withTerminalActivity(registry.terminals, activityByStream, completedTurnsByStream, doneStreams);
    stampedTerminals.current = { source: registry.terminals, value: next };
    onTerminalsChange?.(next);
  }, [activityByStream, completedTurnsByStream, doneStreams, onTerminalsChange, registry.terminals,
    registryLoaded]);

  // A restart gives the same terminal id a new process identity. Keep any
  // future-generation signal buffered, but retire output and activity for
  // generations the control plane has conclusively passed. A listed terminal
  // is alive, so its output flows again even after an earlier removal.
  useEffect(() => {
    streams.sync(registry.terminals);
    for (const terminal of registry.terminals) {
      pruneActivity(terminal.id, terminal.generation);
    }
  }, [pruneActivity, registry.terminals, streams]);

  // A recorded terminal that has not respawned yet keeps its tab: a reopen puts
  // the layout back before the processes exist, and pruning here would throw
  // the arrangement away between the two.
  useEffect(() => {
    if (!registryLoaded || !workspacesLoaded) return;
    const reconciliationWorkspaceId = activeWorkspaceId;
    if (reconciliationWorkspaceId === null) return;
    const reconciliationLayout = layout;
    const known = new Set(visibleTerminals.map(terminal => terminal.id));
    for (const entry of activeWorkspace?.terminals ?? []) known.add(entry.id);
    setLayout(current => reconcileActiveTerminalWorkspaceState(
      current, known, reconciliationWorkspaceId, activeWorkspaceIdRef.current,
      reconciliationLayout,
    ));
  }, [activeWorkspace, activeWorkspaceId, layout, registryLoaded, visibleTerminals,
    workspacesLoaded]);

  // The arrangement belongs to the workspace it arranges, so it is written back
  // into the record rather than into one global key.
  useEffect(() => {
    if (!registryLoaded || !workspacesLoaded || activeWorkspaceId === null) return;
    updateWorkspaces(current => rememberLayout(
      current, activeWorkspaceId, serializeTerminalWorkspaceState(layout), stamp, layoutTerminalIds,
    ));
  }, [activeWorkspaceId, layout, layoutTerminalIds, registryLoaded, updateWorkspaces, workspacesLoaded]);

  useEffect(() => {
    const onHide = (): void => {
      if (document.visibilityState === "hidden") persistWorkspaces("now");
    };
    const onPageHide = (): void => { persistWorkspaces("now"); };
    document.addEventListener("visibilitychange", onHide);
    window.addEventListener("pagehide", onPageHide);
    return () => {
      document.removeEventListener("visibilitychange", onHide);
      window.removeEventListener("pagehide", onPageHide);
      persistWorkspaces("now");
    };
  }, [persistWorkspaces]);

  useEffect(() => () => {
    resizeQueue.dispose();
    redrawPulse.dispose();
    activityTracker.dispose();
  }, [activityTracker, redrawPulse, resizeQueue]);

  const focusedTerminalId = activeTerminalId(layout);
  const focusedTerminal = focusedTerminalId === null
    ? undefined
    : registry.terminals.find(candidate => candidate.id === focusedTerminalId);
  const adoptCwd = useCallback((path: string | null | undefined, persist: boolean): void => {
    const usable = terminalLauncherDefaultCwd(path, null, null);
    if (usable === null) return;
    setLastSelectedCwd(usable);
    if (persist) writeLastCwd(usable);
  }, []);
  const contextualCwd = paneStartDirectory(
    focusedTerminal?.cwd,
    activeWorkspace?.terminals.map(terminal => terminal.cwd) ?? [],
    terminalLauncherDefaultCwd(null, lastSelectedCwd, activeCwd),
  );
  const contextualCwdRef = useRef(contextualCwd);
  contextualCwdRef.current = contextualCwd;
  const launcherCwd = terminalLauncherChosenCwd(browsedCwd, contextualCwd);
  const contextualSessionKey = focusedTerminal?.sessionKey ?? null;
  useEffect(() => { setBrowsedCwd(null); }, [contextualCwd]);
  useEffect(() => {
    adoptCwd(focusedTerminal?.cwd, false);
  }, [adoptCwd, focusedTerminal?.cwd]);
  useEffect(() => {
    onFocusedTerminalChange?.(focusedTerminalId);
  }, [focusedTerminalId, onFocusedTerminalChange]);
  const focusedGeneration = focusedTerminal?.generation;
  const focusedStreamKey = focusedTerminalId === null || focusedGeneration === undefined
    ? null : terminalOutputStreamKey(focusedTerminalId, focusedGeneration);
  // Looking at a pane is what clears its done dot, and a pane already focused
  // never turns blue.
  useEffect(() => {
    focusedStreamRef.current = focusedStreamKey;
    setDoneStreams(current => clearTerminalDone(current, focusedStreamKey));
  }, [focusedStreamKey]);
  // Fit, then focus, after focus/visibility transitions, in one frame.
  // Fit: ordinary box changes are observed by TerminalView; a background
  // terminal's title is not a geometry change.
  // Focus follows the focused terminal's routing identity, the surface coming
  // back, and a dialog closing — those openers are transient menu items, so
  // useModalFocus has no still-connected element to restore focus to. What is
  // NOT a dep here is registry.terminals: keeping it meant an unrelated
  // terminal exiting pulled the caret out of the find input.
  useEffect(() => {
    if (!terminalSurfaceVisibleRef.current || focusedTerminalId === null
        || focusedGeneration === undefined) return;
    const key = terminalOutputStreamKey(focusedTerminalId, focusedGeneration);
    const frame = window.requestAnimationFrame(() => {
      const handle = streams.view(key);
      handle?.fit();
      handle?.focus();
    });
    return () => { window.cancelAnimationFrame(frame); };
  }, [focusedTerminalId, focusedGeneration, visible, interactionBlocked, workspaceView, launcherOpen,
    workspaceRenameId, deleteWorkspaceId]);

  // A focused Browser tab takes the keyboard into its page, unless the click
  // that focused it landed in the page or its address bar already.
  const focusedTab = focusedTabId(layout);
  const focusedBrowserTabId = focusedTab !== null && isBrowserTabId(focusedTab) ? focusedTab : null;
  useEffect(() => {
    if (!terminalSurfaceVisibleRef.current || focusedBrowserTabId === null) return;
    const frame = window.requestAnimationFrame(() => {
      const current = document.activeElement;
      if (current instanceof Element && current.closest(".browser-pane, .browser-page") !== null) return;
      browserPages.get(focusedBrowserTabId)?.focus();
    });
    return () => { window.cancelAnimationFrame(frame); };
  }, [browserPages, focusedBrowserTabId, visible, interactionBlocked, workspaceView, launcherOpen,
    workspaceRenameId, deleteWorkspaceId]);

  // ResizeObserver is a useful fallback for ordinary window resizes, but it
  // can miss a pane that was display:none while a maximize/overview/workspace
  // transition settled. Refit every measurable terminal after the layout and
  // once more on the following frame; TerminalView safely ignores hidden
  // hosts. This keeps all six PTYs aligned with their equal-grid cells, not
  // only whichever pane happens to own focus.
  useLayoutEffect(() => {
    if (!visible || interactionBlocked || workspaceView !== "split" || launcherOpen) return;
    let secondFrame: number | null = null;
    const fitAll = (): void => {
      for (const handle of streams.attachedViews()) handle.fit();
    };
    const firstFrame = window.requestAnimationFrame(() => {
      fitAll();
      secondFrame = window.requestAnimationFrame(fitAll);
    });
    return () => {
      window.cancelAnimationFrame(firstFrame);
      if (secondFrame !== null) window.cancelAnimationFrame(secondFrame);
    };
    // Not `layout`: a splitter drag mints a new layout per frame while only the
    // ratio moves, and the ResizeObserver in TerminalView already covers ratios.
  }, [activeWorkspaceId, interactionBlocked, launcherOpen, visible, workspaceView,
    layout.arrangement, layout.maximizedPaneId, layout.panes.length]);

  const attachTerminal = useCallback((terminalId: string): void => {
    const paneId = crypto.randomUUID();
    setLayout(current => {
      if (terminalPaneForTerminal(current, terminalId) !== undefined) {
        return focusTerminalTab(current, terminalId);
      }
      return openTerminalTab(current, current.focusedPaneId ?? paneId, terminalId);
    });
  }, []);

  const createTerminal = useCallback(async (input: {
    cwd: string;
    title?: string;
    sessionKey?: SessionKey;
    launch?: TerminalLaunchRequestV1;
    handoff?: TerminalHandoffV1;
    split?: { paneId: TerminalPaneId; orientation: TerminalSplitOrientation };
    /** Presentation identity inherited by a profile-less shell split. */
    inheritedProfileId?: TerminalLaunchProfileId;
    /** Capture the clicked pane before the host reply, even if focus moves. */
    tabPaneId?: TerminalPaneId;
    /** Resume this provider session instead of minting a new one (Open in terminal). */
    launchSession?: TerminalLaunchSessionV1;
    /** Run the profile CLI's sign-in instead of a chat: no chat id is minted. */
    signIn?: boolean;
    /** Protocol 60: the provider account a sign-in pane signs in; only with `signIn`. */
    account?: string;
  }): Promise<TerminalSummary | null> => {
    if (input.cwd.trim() === "") {
      report("Select a project before starting an integrated terminal.");
      return null;
    }
    if (input.split !== undefined && layoutRef.current.panes.length >= MAX_TERMINAL_PANES_HARD) {
      report(`The terminal workspace supports up to ${MAX_TERMINAL_PANES_HARD.toString()} panes.`);
      return null;
    }
    const id = crypto.randomUUID();
    mintedTerminalIds.current.add(id);
    const launchWorkspaceId = activeWorkspaceIdRef.current;
    // A pass-off pane's `cwd` may be a subdirectory of its approved project
    // `root`, whereas ordinary creates require the approved root itself.
    const authorizedCwd = input.handoff === undefined
      ? (registryRef.current.terminals.find(
          candidate => candidate.cwd === input.cwd && candidate.root !== "",
        )?.root ?? input.cwd)
      : input.cwd;
    // Mint the chat id up front for the CLIs that accept one, so this terminal
    // has something exact to be reopened onto later.
    const launchSession = input.signIn === true ? undefined
      : input.launchSession ?? newChatBinding(input.launch?.profileId, () => crypto.randomUUID());
    if (launchWorkspaceId !== null) {
      pendingWorkspaceByTerminal.current.set(id, launchWorkspaceId);
    }
    try {
      const terminal = await api.createTerminal({
        id,
        cwd: authorizedCwd,
        ...(input.title === undefined ? {} : { title: input.title }),
        ...(input.sessionKey === undefined ? {} : { sessionKey: input.sessionKey }),
        ...(input.launch === undefined ? {} : { launch: input.launch }),
        ...(launchSession === undefined ? {} : { launchSession }),
        ...(input.handoff === undefined ? {} : { handoff: input.handoff }),
        ...(input.signIn === true ? { signIn: true } : {}),
        ...(input.signIn === true && input.account !== undefined ? { account: input.account } : {}),
      });
      const inheritedProfileId = input.inheritedProfileId;
      if (inheritedProfileId !== undefined) {
        setInheritedProfileByTerminal(current => {
          if (current.get(terminal.id) === inheritedProfileId) return current;
          const next = new Map(current);
          next.set(terminal.id, inheritedProfileId);
          return next;
        });
      }
      acceptSummary(terminal);
      file(terminal, launchSession, launchWorkspaceId);
      pendingWorkspaceByTerminal.current.delete(id);
      // Minted out here: React may run this updater twice (see onceTerminalIds).
      const newPaneId = crypto.randomUUID();
      const splitId = crypto.randomUUID();
      const tidyIds = onceTerminalIds(() => crypto.randomUUID());
      const place = (current: TerminalWorkspaceState): TerminalWorkspaceState => {
        if (input.split !== undefined
            && terminalPaneById(current, input.split.paneId) !== undefined) {
          return splitTerminalPaneInPlace(current, {
            paneId: input.split.paneId,
            newPaneId,
            splitId,
            terminalId: terminal.id,
            orientation: input.split.orientation,
          }, tidyIds);
        }
        const target = input.tabPaneId !== undefined && terminalPaneById(current, input.tabPaneId)
          ? input.tabPaneId : current.focusedPaneId ?? newPaneId;
        return openTerminalTab(current, target, terminal.id);
      };
      if (launchWorkspaceId !== null && launchWorkspaceId !== activeWorkspaceIdRef.current) {
        // The reply belongs to the workspace that asked for it: the tab slot,
        // split or tab row it names is there. Placing it in the live layout
        // would move it into a workspace selected meanwhile, so it goes into
        // the asking workspace's saved layout instead.
        updateWorkspaces(current => {
          let changed = false;
          const next = current.map(record => {
            if (record.id !== launchWorkspaceId) return record;
            const saved = parseTerminalWorkspaceState(record.layout ?? null);
            const placed = place(saved);
            if (placed === saved) return record;
            changed = true;
            return { ...record, layout: serializeTerminalWorkspaceState(placed), updatedAt: stamp() };
          });
          return changed ? next : current;
        }, "now");
        return terminal;
      }
      setLayout(place);
      return terminal;
    } catch (error) {
      pendingWorkspaceByTerminal.current.delete(id);
      report(error);
      return null;
    }
  }, [acceptSummary, file, report, updateWorkspaces]);

  const openLauncher = useCallback((): void => {
    setLauncherOpen(true);
    setLauncherNotice(null);
    setBrowsedCwd(null);
    loadLaunchProfiles();
  }, [loadLaunchProfiles]);

  const launchTerminalBatch = useCallback((profileId: TerminalLaunchProfileId,
      count: number, cwd: string): void => {
    if (launchPending.current) return;
    const profile = launchProfilesRef.current.find(candidate => candidate.id === profileId);
    if (profile === undefined || !profile.available) {
      setLauncherNotice({
        kind: "error",
        message: "That terminal launcher is no longer available. Refresh the choices and try again.",
      });
      return;
    }
    if (!Number.isInteger(count) || !TERMINAL_LAUNCH_COUNTS.includes(count)) {
      setLauncherNotice({
        kind: "error",
        message: `Choose a terminal count from ${TERMINAL_LAUNCH_COUNTS[0]} through ${TERMINAL_LAUNCH_COUNTS.at(-1)}.`,
      });
      return;
    }
    if (cwd.trim() === "") {
      setLauncherNotice({ kind: "error", message: "Choose a working folder before launching terminals." });
      return;
    }
    adoptCwd(cwd, true);
    setLastProfileId(profileId);
    setLastCount(count);
    const root = cwd;
    const launchWorkspaceId = activeWorkspaceIdRef.current;
    if (launchWorkspaceId === null) {
      setLauncherNotice({
        kind: "error",
        message: "Choose a terminal workspace before launching terminals.",
      });
      return;
    }

    launchPending.current = true;
    setLauncherBusy(true);
    setLauncherNotice(null);
    const ids: string[] = Array.from({ length: count }, () => crypto.randomUUID());
    for (const id of ids) mintedTerminalIds.current.add(id);
    const launch: TerminalLaunchRequestV1 = { version: 1, profileId };
    const sessionKey = root === contextualCwd ? contextualSessionKey : null;
    // Each terminal gets its OWN chat id: a shared one would have every tab in
    // the batch reopen onto the same conversation.
    const bindings = ids.map(() => newChatBinding(profileId, () => crypto.randomUUID()));
    for (const id of ids) pendingWorkspaceByTerminal.current.set(id, launchWorkspaceId);
    const requests = ids.map((id, index) => api.createTerminal({
      id,
      cwd: root,
      title: count === 1 ? profile.label : `${profile.label} ${index + 1}`,
      launch,
      ...(sessionKey === null ? {} : { sessionKey }),
      ...(bindings[index] === undefined ? {} : { launchSession: bindings[index] }),
    }));

    void Promise.allSettled(requests).then(async settled => {
      let snapshot: Awaited<ReturnType<typeof api.listTerminals>> | null = null;
      let listingError: unknown = null;
      try {
        snapshot = await api.listTerminals();
      } catch (error) {
        listingError = error;
      }

      const requested = new Set(ids);
      const fulfilled = settled.flatMap(result => result.status === "fulfilled"
        && requested.has(result.value.id)
        && result.value.launch?.version === 1
        && result.value.launch.profileId === profileId ? [result.value] : []);
      const fulfilledById = new Map(fulfilled.map(terminal => [terminal.id, terminal]));
      const currentSnapshot = snapshot;
      // A successful create response is already positive confirmation. The
      // registry read is an additional recovery path for an unknown/rejected
      // response, not a reason to discard a success merely because the
      // lifecycle snapshot is one revision behind the just-finished batch.
      const confirmed = ids.flatMap(id => {
        const listed = currentSnapshot?.terminals.find(candidate => candidate.id === id);
        const terminal = listed?.launch?.version === 1
            && listed.launch.profileId === profileId
          ? listed : fulfilledById.get(id);
        return terminal === undefined ? [] : [terminal];
      });
      for (const terminal of confirmed) {
        file(terminal, bindings[ids.indexOf(terminal.id)], launchWorkspaceId);
      }
      for (const id of ids) pendingWorkspaceByTerminal.current.delete(id);

      if (currentSnapshot !== null) {
        acceptRegistrySnapshot(currentSnapshot);
        setRegistryLoaded(true);
      } else {
        for (const terminal of fulfilled) acceptSummary(terminal);
      }
      const confirmedIds = confirmed.map(terminal => terminal.id);
      if (confirmedIds.length > 0) {
        const paneIds = onceTerminalIds(() => crypto.randomUUID());
        const splitIds = onceTerminalIds(() => crypto.randomUUID());
        setLayout(current => attachTerminalsBalanced(current, confirmedIds, paneIds, splitIds));
      }

      const failed = count - confirmedIds.length;
      const firstRejected = settled.find(result => result.status === "rejected");
      const detail = listingError !== null
        ? ` Interface could not confirm the complete registry: ${messageFor(listingError)}`
        : firstRejected?.status === "rejected" ? ` ${messageFor(firstRejected.reason)}` : "";
      if (failed === 0 && listingError === null) {
        setLauncherOpen(false);
        setBrowsedCwd(null);
        setLauncherNotice(null);
      } else if (confirmedIds.length > 0) {
        const outcome = failed === 0
          ? `${confirmedIds.length.toString()} ${profile.label} terminal${
              confirmedIds.length === 1 ? " was" : "s were"} accepted, but Interface could not confirm the complete registry.`
          : `${confirmedIds.length.toString()} of ${count.toString()} ${profile.label} terminal${
              confirmedIds.length === 1 ? "" : "s"} started. ${failed.toString()} failed.`;
        setLauncherNotice({
          kind: "partial",
          message: `${outcome} Interface did not retry.${detail}`,
        });
      } else {
        setLauncherNotice({
          kind: "error",
          message: `No ${profile.label} terminals started. Interface did not retry.${detail}`,
        });
      }
    }).catch(error => {
      setLauncherNotice({
        kind: "error",
        message: `${messageFor(error)} Interface did not retry the launch.`,
      });
    }).finally(() => {
      launchPending.current = false;
      setLauncherBusy(false);
    });
  }, [acceptRegistrySnapshot, acceptSummary, adoptCwd, contextualCwd, contextualSessionKey, file]);

  const launchAdversarialReview = useCallback((
      request: TerminalAdversarialReviewLaunchRequest): void => {
    if (launchPending.current || reserveAgentSessionPanes === undefined
        || releaseAgentSessionPanes === undefined || onOpenAgentSessions === undefined) return;
    const builder = launchProfilesRef.current.find(profile =>
      profile.id === request.builderProfileId && profile.available);
    const reviewer = launchProfilesRef.current.find(profile =>
      profile.id === request.reviewerProfileId && profile.available);
    const supportedPair = builder !== undefined && reviewer !== undefined
      && ((builder.id === "claude" && reviewer.id === "codex")
        || (builder.id === "codex" && reviewer.id === "claude"));
    if (!supportedPair) {
      setLauncherNotice({
        kind: "error",
        message: "Adversarial review requires both Claude Code and Codex.",
      });
      return;
    }
    if (request.cwd.trim() === "" || request.objective.trim() === "") {
      setLauncherNotice({
        kind: "error",
        message: "Choose a working folder and enter an objective before starting review.",
      });
      return;
    }
    adoptCwd(request.cwd, true);
    if (!reserveAgentSessionPanes(2)) {
      setLauncherNotice({
        kind: "error",
        message: "Close an agent pane before starting a two-pane adversarial review.",
      });
      return;
    }
    let reservationHeld = true;
    launchPending.current = true;
    setLauncherBusy(true);
    setLauncherNotice(null);
    const builderEngineId: "claude" | "codex" = builder.id === "claude" ? "claude" : "codex";
    const reviewerEngineId: "claude" | "codex" = reviewer.id === "claude" ? "claude" : "codex";
    void api.startAdversarialReview({
      builderEngineId,
      reviewerEngineId,
      canonicalCwd: request.cwd,
      objective: request.objective.trim(),
      clientRequestId: crypto.randomUUID(),
    }).then(result => {
      const builderKey = result.builder.sessionKey;
      const reviewerKey = result.reviewer.sessionKey;
      if (result.version !== 1 || typeof builderKey !== "string" || builderKey === ""
          || typeof reviewerKey !== "string" || reviewerKey === "") {
        setLauncherNotice({
          kind: "partial",
          message: "The review workflow was recorded, but both agent sessions were not ready. "
            + `${productLabel()} did not retry an unknown or failed provider effect.`,
        });
        return;
      }
      if (!onOpenAgentSessions(
          builderKey as SessionKey, reviewerKey as SessionKey, request.objective.trim())) {
        setLauncherNotice({
          kind: "partial",
          message: "The builder and reviewer started, but their two panes could not be opened. "
            + "The sessions remain available in the project list; Interface did not retry.",
        });
        return;
      }
      reservationHeld = false;
      setReviewOpen(false);
      setLauncherOpen(false);
      setBrowsedCwd(null);
      setLauncherNotice(null);
    }).catch(error => {
      setLauncherNotice({
        kind: "error",
        message: `${messageFor(error)} Interface did not retry the review launch.`,
      });
    }).finally(() => {
      if (reservationHeld) releaseAgentSessionPanes(2);
      launchPending.current = false;
      setLauncherBusy(false);
    });
  }, [adoptCwd, onOpenAgentSessions, releaseAgentSessionPanes, reserveAgentSessionPanes]);

  // Review needs all three agent-pane callbacks; without them there is no
  // workflow to link to and the launcher shows no link at all, rather than a
  // dead entry that explains itself only after you press it.
  const reviewConnected = reserveAgentSessionPanes !== undefined
    && releaseAgentSessionPanes !== undefined && onOpenAgentSessions !== undefined;
  const openReview = useCallback((): void => {
    if (launchPending.current || !reviewConnected) return;
    setLauncherNotice(null);
    setReviewOpen(true);
  }, [reviewConnected]);
  /** "Send selection to thread…" (T3 section 9): the active terminal's
      selected text, read from its emulator, waits for a target thread. */
  const [selectionSend, setSelectionSend] = useState<{ title: string; selection: string } | null>(null);
  const sendSelection = useCallback((terminalId: string): void => {
    const terminal = terminalById(terminalId);
    if (terminal === undefined) return;
    const selection = streams.view(terminalOutputStreamKey(terminal.id, terminal.generation))
      ?.getSelection() ?? "";
    if (selection.trim() === "") {
      onError("Select text in the terminal first, then send it to a thread.");
      return;
    }
    setSelectionSend({ title: terminal.title.trim() || "Terminal", selection });
  }, [onError, streams, terminalById]);

  /** Empty a terminal's screen. The view's own handle also drops the replay
      tail, so what the pane shows and what a remount would restore agree. */
  const clearScrollback = useCallback((terminalId: string): void => {
    const terminal = terminalById(terminalId);
    if (terminal === undefined) return;
    streams.view(terminalOutputStreamKey(terminal.id, terminal.generation))?.clear();
  }, [streams, terminalById]);

  // "Start again" on an ended agent pane. The host restarts the same terminal
  // (a new generation, resuming its captured session) under its restart proof.
  const restarting = useRef(new Set<string>());
  const restartTerminal = useCallback((terminalId: string): void => {
    if (api.restartTerminal === undefined) return;
    const terminal = terminalById(terminalId);
    if (terminal === undefined || (terminal.state !== "exited" && terminal.state !== "failed")) return;
    const key = terminalOutputStreamKey(terminal.id, terminal.generation);
    if (restarting.current.has(key)) return;
    restarting.current.add(key);
    void api.restartTerminal({ id: terminal.id, generation: terminal.generation })
      .then(acceptSummary, report)
      .finally(() => { restarting.current.delete(key); });
  }, [acceptSummary, api, report, terminalById]);

  const dismiss = useCallback((terminalId: string): void => {
    retirePending.current.delete(terminalId);
    void api.dismissTerminal(terminalId).then(result => {
      if (!result.removed) return;
      retireTerminal(terminalId);
      const currentRegistry = registryRef.current;
      const nextRegistry = {
        ...currentRegistry,
        terminals: currentRegistry.terminals.filter(terminal => terminal.id !== terminalId),
      };
      registryRef.current = nextRegistry;
      setRegistry(nextRegistry);
      setLayout(current => closeTerminalTab(current, terminalId));
      updateWorkspaces(current => forgetTerminal(current, terminalId, stamp), "now");
    }).catch(report);
  }, [report, retireTerminal, updateWorkspaces]);

  useEffect(() => {
    for (const terminal of registry.terminals) {
      if (!retirePending.current.has(terminal.id)) continue;
      if (terminal.state !== "exited" && terminal.state !== "failed") continue;
      retirePending.current.delete(terminal.id);
      dismiss(terminal.id);
    }
  }, [dismiss, registry.terminals]);

  /**
   * End terminals the way deleting their workspace would: a running one is
   * asked to close and dismissed once it exits, anything else is dismissed
   * now. A saved pane with no live terminal (a refused reopen) is forgotten.
   */
  const endTerminals = useCallback((terminalIds: readonly string[]): void => {
    const wanted = new Set(terminalIds);
    for (const terminal of registryRef.current.terminals) {
      if (!wanted.delete(terminal.id)) continue;
      if (terminal.state === "running" || terminal.state === "starting") {
        retirePending.current.add(terminal.id);
        void api.closeTerminal({ id: terminal.id, generation: terminal.generation }).catch(error => {
          retirePending.current.delete(terminal.id);
          report(error);
        });
      } else {
        dismiss(terminal.id);
      }
    }
    if (wanted.size === 0) return;
    updateWorkspaces(current => [...wanted].reduce(
      (records, terminalId) => forgetTerminal(records, terminalId, stamp), current), "now");
  }, [api, dismiss, report, updateWorkspaces]);

  // A terminal running where no pane shows it — every workspace's layout
  // misses it — slipped past closing its tab (see terminal-hidden-sweep.ts).
  // Once it has sat hidden and quiet for the grace, it ends. Creates, reopens
  // and closes already in flight are not hidden; nor is anything while a
  // launch batch is placing its panes.
  const hiddenSince = useRef<ReadonlyMap<string, number>>(new Map());
  const [hiddenSweepTick, setHiddenSweepTick] = useState(0);
  const attachedAnywhere = useMemo(() => attachedTerminalIds(workspaces, layoutTerminalIds),
    [layoutTerminalIds, workspaces]);
  useEffect(() => {
    if (!registryLoaded || !workspacesLoaded || launchPending.current) return undefined;
    const hidden = registry.terminals.filter(terminal => !attachedAnywhere.has(terminal.id)
      && !retirePending.current.has(terminal.id)
      && !pendingWorkspaceByTerminal.current.has(terminal.id));
    const keys = new Map(hidden.map(terminal => [
      terminalOutputStreamKey(terminal.id, terminal.generation), terminal.id]));
    const sweep = sweepHiddenTerminals(hidden.map(terminal => {
      const key = terminalOutputStreamKey(terminal.id, terminal.generation);
      const activity = activityByStream.get(key);
      return {
        key,
        busy: terminal.state === "starting" || activity === "working" || activity === "waiting",
      };
    }), hiddenSince.current, Date.now());
    hiddenSince.current = sweep.since;
    if (sweep.due.length > 0) endTerminals(sweep.due.flatMap(key => keys.get(key) ?? []));
    if (sweep.nextCheckMs === null) return undefined;
    const timer = window.setTimeout(() => { setHiddenSweepTick(tick => tick + 1); },
      sweep.nextCheckMs);
    return () => { window.clearTimeout(timer); };
  }, [activityByStream, attachedAnywhere, endTerminals, hiddenSweepTick, registry.terminals,
    registryLoaded, workspacesLoaded]);

  /**
   * An agent CLI that ended on its own is handed a plain shell by Rust, in the
   * folder it worked in, whatever this window shows (protocol 50). This only
   * reflects it: the shell takes the agent's tab slot wherever the agent is
   * laid out — the shown workspace or a parked one's saved layout — and the
   * agent's saved entry becomes the shell's. It runs on every registry and
   * layout change and on load, so a window that missed the moment (a reload)
   * still makes the move; once nothing names the agent it does nothing.
   */
  useEffect(() => {
    if (!registryLoaded || !workspacesLoaded) return;
    for (const shell of registry.terminals) {
      if (shell.replaces === undefined) continue;
      const placed = placeHandedOffShell({
        shell, layout: layoutRef.current, records: workspacesRef.current, now: stamp,
      });
      if (placed === null) continue;
      mintedTerminalIds.current.add(shell.id);
      const agentId = shell.replaces;
      if (placed.layout !== layoutRef.current) {
        setLayout(current => replaceTerminalTab(current, agentId, shell.id));
      }
      updateWorkspaces(current => placeHandedOffShell({
        shell, layout: layoutRef.current, records: current, now: stamp,
      })?.records ?? current, "now");
    }
    for (const terminal of registry.terminals) {
      if (terminal.replaces !== undefined || terminal.passedOffFrom === undefined) continue;
      if (mintedTerminalIds.current.has(terminal.id)) continue;
      if (terminalPaneForTerminal(layoutRef.current, terminal.id) !== undefined) continue;
      if (workspaceForTerminal(workspacesRef.current, terminal.id) !== undefined) continue;
      seatHostTerminal(terminal);
    }
  }, [layout, registry.terminals, registryLoaded, seatHostTerminal, updateWorkspaces, workspacesLoaded]);

  // `blank`: the ⌘D chords open a fresh shell that carries none of the
  // parent's session or provider mark; the menus' splits keep them.
  const split = useCallback((paneId: TerminalPaneId,
      orientation: TerminalSplitRequest, profileId?: TerminalLaunchProfileId,
      blank = false): void => {
    const pane = terminalPaneById(layoutRef.current, paneId);
    const terminal = pane === undefined ? undefined
      : terminalById(pane.activeTerminalId);
    const cwd = paneStartDirectory(
      terminal?.cwd,
      workspaceById(workspacesRef.current, activeWorkspaceIdRef.current)
        ?.terminals.map(entry => entry.cwd) ?? [],
      activeCwd,
    );
    if (cwd === null) {
      report("Select a project before splitting the terminal workspace.");
      return;
    }
    const profile = profileId === undefined ? undefined
      : launchProfilesRef.current.find(candidate => candidate.id === profileId);
    if (profileId !== undefined && profile?.available !== true) {
      report("That terminal launcher is no longer available. Refresh the choices and try again.");
      return;
    }
    const launched: { title?: string; launch?: TerminalLaunchRequestV1 } = profile === undefined
      ? {} : { title: profile.label, launch: { version: 1, profileId: profile.id } };
    const inheritedProfileId = profileId === undefined && terminal !== undefined && !blank
      ? terminal.launch?.profileId ?? inheritedProfileByTerminal.get(terminal.id)
      : undefined;
    void createTerminal({
      cwd,
      ...(terminal?.sessionKey === undefined || blank ? {} : { sessionKey: terminal.sessionKey }),
      ...(inheritedProfileId === undefined ? {} : { inheritedProfileId }),
      ...launched,
      split: { paneId, orientation: orientation === "auto" ? drawnPaneSplitOrientation(paneId) : orientation },
    });
  }, [activeCwd, createTerminal, inheritedProfileByTerminal, report]);

  // Pass off: a new agent starts beside this pane holding what the old one was
  // doing. The host resolves the source's live cwd itself, so `cwd` below is
  // only the fallback for a source whose shell never announced one.
  const passOff = useCallback((paneId: TerminalPaneId,
      orientation: TerminalSplitOrientation,
      profileId: TerminalLaunchProfileId): void => {
    const pane = terminalPaneById(layoutRef.current, paneId);
    const source = pane === undefined ? undefined : terminalById(pane.activeTerminalId);
    const profile = launchProfilesRef.current.find(candidate => candidate.id === profileId);
    const refusal = passOffRefusal(
      source === undefined ? undefined : { status: source.state }, profile);
    if (refusal !== null || source === undefined || profile === undefined) {
      // `report` prints an Error's own words and a fallback sentence for
      // anything else, so the refusal travels as an Error to be read at all.
      report(new Error(refusal ?? "Pass off needs a running source terminal."));
      return;
    }
    const handle = streams.view(terminalOutputStreamKey(source.id, source.generation));
    if (handle === undefined) {
      // No attached view for this generation — a restart mid-click, or a pane
      // whose xterm has not mounted yet. Handing over an empty transcript would
      // open a blank agent that has been told to read nothing, so refuse and
      // let the human retry once the view is back.
      report(new Error("That terminal's output is not attached yet. Try the pass off again."));
      return;
    }
    const transcript = passOffTranscript(handle.readLines());
    const fromTitle = passOffTitle(source.title);
    void createTerminal({
      cwd: source.cwd,
      title: profile.label,
      launch: { version: 1, profileId: profile.id },
      handoff: {
        version: 1,
        sourceTerminalId: source.id,
        transcript,
        ...(fromTitle === undefined ? {} : { fromTitle }),
      },
      split: { paneId, orientation },
    });
  }, [createTerminal, report, streams]);

  // Keyboard-only terminal switching and splits. Capture beats xterm's own
  // listener on the way down, so the chord moves focus or splits instead of
  // reaching the shell.
  useEffect(() => {
    if (!terminalSurfaceVisibleRef.current) return;
    const onKeyDown = (event: KeyboardEvent): void => {
      const action = terminalCenterKeyAction(event);
      if (action === null) return;
      if (!(event.target instanceof Element)
          || event.target.closest(".terminal-center") === null) return;
      if (action === "new-pane") {
        // ⌘↩ with no pane to open one beside is the launcher's own chord.
        if (layoutRef.current.focusedPaneId === null
            || terminalLauncherShowing(launcherOpen, layoutRef.current)) return;
        const field = event.target.closest("input, textarea, select, [contenteditable='true']");
        if (!newPaneChordAllowedAt({
          insideLauncher: event.target.closest(".terminal-launcher") !== null,
          field: field === null ? "none"
            : field.classList.contains("xterm-helper-textarea") ? "terminal-input" : "other",
        })) return;
      }
      event.preventDefault();
      event.stopPropagation();
      if (action === "previous-terminal" || action === "next-terminal") {
        const next = cycleTerminalFocus(layoutRef.current,
          action === "previous-terminal" ? "previous" : "next");
        layoutRef.current = next;
        setLayout(next);
        return;
      }
      const paneId = layoutRef.current.focusedPaneId;
      if (paneId === null) return;
      if (action === "new-pane") { split(paneId, "auto", undefined, true); return; }
      split(paneId, action === "split-horizontal" ? "horizontal" : "vertical", undefined, true);
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => { window.removeEventListener("keydown", onKeyDown, true); };
  }, [visible, interactionBlocked, workspaceView, launcherOpen, workspaceRenameId,
    deleteWorkspaceId, split]);

  /**
   * A pasted or dragged screenshot has no path until Interface writes it, so
   * the terminal gets one the same way the composer does: main sniffs and
   * decodes the bytes, writes an app-owned file, and the agent running in the
   * terminal opens that path with its own tools.
   */
  const storeImages = useCallback(async (files: readonly PastedFile[]): Promise<string[]> => {
    if (files.length === 0) return [];
    try {
      const bytes = await Promise.all(files.map(async file =>
        new Uint8Array(await file.arrayBuffer())));
      const result = await api.savePastedImages(bytes);
      for (const rejection of result.rejected) errorRef.current(rejection.message);
      return result.saved.map(saved => saved.path);
    } catch (error) {
      report(error);
      return [];
    }
  }, [report]);

  const sendBytes = useCallback((terminal: TerminalSummary, bytes: Uint8Array): boolean => {
    const current = terminalById(terminal.id);
    if (current === undefined || current.generation !== terminal.generation
        || current.state !== "running") return false;
    try {
      api.sendTerminalInput(current.id, current.generation, bytes);
      if (!isTerminalProtocolReply(bytes)) {
        observeInput(terminalOutputStreamKey(current.id, current.generation));
      }
      return true;
    } catch (error) {
      report(error);
      return false;
    }
  }, [observeInput, report]);

  /** Size one running generation's PTY. */
  const resizeHost = useCallback(async (terminal: TerminalSummary,
      size: { columns: number; rows: number }): Promise<void> => {
    const current = terminalById(terminal.id);
    if (current === undefined || current.generation !== terminal.generation
        || current.state !== "running") return;
    const summary = await api.resizeTerminal({
      id: current.id,
      generation: current.generation,
      size,
    });
    acceptSummary(summary);
  }, [acceptSummary, terminalById]);

  const scheduleResize = useCallback((terminal: TerminalSummary,
      size: { columns: number; rows: number }): void => {
    const key = terminalOutputStreamKey(terminal.id, terminal.generation);
    if (redrawPulse.updateTarget(key, size)) return;
    resizeQueue.schedule(key, size, requestedSize => resizeHost(terminal, requestedSize));
  }, [redrawPulse, resizeHost, resizeQueue]);

  const kickRedraw = useCallback((terminal: TerminalSummary,
      size: { columns: number; rows: number }): void => {
    const kick = terminalRedrawKickSize(size);
    if (kick === null) {
      scheduleResize(terminal, size);
      return;
    }
    const key = terminalOutputStreamKey(terminal.id, terminal.generation);
    const expectedHandle = streams.view(key);
    // A prior view can still have a resize inside the host when this xterm is
    // mounted. Finish that one request before beginning the distinct pulse.
    const resizeIdle = resizeQueue.cancelAndWait(key);
    const send = async (requestedSize: { columns: number; rows: number }): Promise<void> => {
      await resizeIdle;
      if (expectedHandle === undefined || streams.view(key) !== expectedHandle) return;
      await resizeHost(terminal, requestedSize);
    };
    // completeSetup() fits the new emulator and queues the same final size.
    // Fence that routine resize so the two distinct SIGWINCH geometries cannot
    // collapse back into the old back-to-back race.
    redrawPulse.start(key, kick, size, send,
      terminal.launch?.profileId === "opencode" || streams.queriedPixelSize(key)
        ? "opentui-resize" : "output");
  }, [redrawPulse, resizeHost, resizeQueue, scheduleResize, streams]);

  /** Located paths first, then whatever the pathless images were written to.
   *  A line that already has text gets the paths at its end, not at the caret.
   *  Each path is pasted, not typed: xterm brackets the paste when the program
   *  enabled mode 2004, and Claude Code and Codex turn a pasted image path into
   *  their own `[Image #1]` placeholder, exactly as after a Finder drag into
   *  Terminal.app. A shell pane shows the path. One paste per path, because
   *  both CLIs only recognise a paste that is a single path.
   *  A pasted image in an agent pane skips all of that: the CLI pastes it from
   *  the clipboard itself on Ctrl+V. Dropped files are not on the clipboard. */
  const insertPaths = useCallback(async (terminal: TerminalSummary,
      located: readonly string[], files: readonly PastedFile[],
      source: "clipboard" | "drop"): Promise<void> => {
    const pasteInput = terminalImagePasteInput(terminal.launch?.profileId, source);
    if (located.length === 0 && files.length > 0 && pasteInput !== null) {
      sendBytes(terminal, textEncoder.encode(pasteInput));
      return;
    }
    const paths = [...located, ...await storeImages(files)];
    if (paths.length === 0) return;
    const key = terminalOutputStreamKey(terminal.id, terminal.generation);
    const handle = streams.view(key);
    const line = handle?.currentLine() ?? "";
    const prefix = terminalPathInsertPrefix(line);
    if (prefix !== "") sendBytes(terminal, textEncoder.encode(prefix));
    for (const path of paths) handle?.paste(`${shellQuote(path)} `);
  }, [sendBytes, storeImages, streams, textEncoder]);

  /** A view's emulator mounted (a handle) or was disposed (null). */
  const attachView = useCallback((terminal: TerminalSummary,
      handle: TerminalViewHandle | null): void => {
    const key = terminalOutputStreamKey(terminal.id, terminal.generation);
    if (handle === null) {
      streams.detach(key);
      resizeQueue.cancel(key);
      redrawPulse.cancel(key);
      return;
    }
    const focusIfCurrent = (): void => {
      focusTerminalViewIfCurrent(handle, streams.view(key),
        terminalSurfaceVisibleRef.current, activeTerminalId(layoutRef.current) === terminal.id);
    };
    streams.attach(key, handle, terminal.launch?.profileId, redraw => {
      focusIfCurrent();
      if (!redraw) return;
      const size = handle.size();
      if (size !== null) kickRedraw(terminal, size);
    });
    focusIfCurrent();
  }, [kickRedraw, redrawPulse, resizeQueue, streams]);

  const openTerminalLink = useCallback((terminal: TerminalSummary, rawLink: string): void => {
    const resolved = resolveTerminalLinkTarget(rawLink, terminal.cwd);
    if (resolved === null) return;
    if (resolved.kind === "url") {
      const page = browserPageUrl(resolved.url);
      if (page !== null && openBrowserPane({ terminalId: terminal.id }, terminal.cwd, page) !== null) {
        return;
      }
      void openBrowserOutside(resolved.url).catch(report);
      return;
    }
    const workspaceFolder = terminal.root || terminal.cwd || contextualCwdRef.current;
    const target = attachmentPreviewTarget(resolved.path, workspaceFolder);
    if (target === null) return;
    if (previewApi !== null && workspaceFolder !== null && workspaceFolder !== "") {
      openOutputPreview(workspaceFolder);
      outputOpenNonce.current += 1;
      setOutputOpenRequest({
        root: target.root,
        path: target.path,
        nonce: outputOpenNonce.current,
      });
      return;
    }
    onOpenWorkspaceTool?.("files", resolved.path);
  }, [onOpenWorkspaceTool, openBrowserOutside, openBrowserPane, openOutputPreview, previewApi, report]);

  const viewActions = useCommittedActions<TerminalViewActions>({
    sendBytes, scheduleResize, insertPaths,
    pathForFile: file => api.pathForFile(file),
    clearReplay: key => { streams.clear(key); },
    attachView,
    openTerminalLink,
  });

  // Stable across activity, layout and registry changes: the pool asks it for
  // each view, and every pane holds it (through renderTerminal) as a prop.
  const renderTerminalView = useCallback((context: TerminalViewRenderContext): ReactNode => {
    const terminal = terminalById(context.terminalId);
    if (terminal === undefined) return null;
    const key = terminalOutputStreamKey(terminal.id, terminal.generation);
    return <PooledTerminalView key={key} terminal={terminal}
      active={context.active && terminalSurfaceVisibleRef.current}
      working={context.working === true} actions={viewActions} />;
  }, [terminalById, viewActions]);

  const terminalStreamKeys = useMemo(() => new Set(registry.terminals.map(terminal =>
    terminalOutputStreamKey(terminal.id, terminal.generation))), [registry.terminals]);
  const workingStreams = useMemo(() => new Set([...activityByStream]
    .flatMap(([key, activity]) => activity === "working" ? [key] : [])), [activityByStream]);
  const renderTerminal = useCallback((context: TerminalPaneRenderContext): ReactNode => {
    if (isBrowserTabId(context.terminalId)) {
      const tabId = context.terminalId;
      return <BrowserPane key={tabId} tabId={tabId}
        page={browserPages.page(tabId, browserTabUrl(layoutRef.current, tabId))}
        servers={browserServers.get(tabId)} active={context.active}
        onFindServers={findBrowserServers}
        onOpenExternal={openWorkspaceTool === undefined ? undefined : openBrowserOutside}
        pointTargets={outputFeedbackTargets} onAddToDraft={onAddBrowserNote} />;
    }
    const terminal = terminalById(context.terminalId);
    if (terminal === undefined) {
      const failure = reopenFailures.get(context.terminalId);
      if (failure === undefined) return null;
      const moved = movedFolders.get(context.terminalId);
      return <TerminalReopenFailure message={failure} moved={moved}
        onReopenMoved={moved === undefined ? undefined : () => { reopenInMovedFolder(moved); }} />;
    }
    const key = terminalOutputStreamKey(terminal.id, terminal.generation);
    return <TerminalViewSlot key={key} streamKey={key} context={context} />;
  }, [browserPages, browserServers, findBrowserServers, movedFolders, onAddBrowserNote, openBrowserOutside,
    openWorkspaceTool, outputFeedbackTargets, reopenFailures, reopenInMovedFolder, renderTerminalView, terminalById]);

  // A card says what its pane was last doing. The lines are read off the
  // covered xterm buffers when the cards appear rather than per write — the
  // panes stay mounted, so the buffers are still there to read.
  useEffect(() => {
    if (workspaceView === "split") {
      setPreviews(current => (current.size === 0 ? current : new Map()));
      return;
    }
    const snapshot = new Map<string, readonly string[]>();
    for (const terminal of registryRef.current.terminals) {
      const lines = streams.view(terminalOutputStreamKey(terminal.id, terminal.generation))
        ?.tailLines(40) ?? [];
      const preview = terminalCardPreview(lines, terminal.cwd, terminal.launch?.profileId
        ?? inheritedProfileByTerminal.get(terminal.id));
      if (preview.length > 0) snapshot.set(terminal.id, preview);
    }
    setPreviews(snapshot);
  }, [workspaceView, inheritedProfileByTerminal]);

  // When each agent entered its status, watched whether or not the overview
  // is open, so a card's clock is right the moment it appears. A status first
  // seen has no clock: it may be far older than this window.
  const overviewSinceRef = useRef<ReadonlyMap<string, AgentOverviewSince>>(new Map());
  const overviewSince = useMemo(() => {
    const entries = registry.terminals.flatMap(terminal => {
      if (!isAgentProfile(terminal.launch?.profileId ?? inheritedProfileByTerminal.get(terminal.id))) return [];
      const key = terminalOutputStreamKey(terminal.id, terminal.generation);
      return [{ id: terminal.id, status: agentOverviewStatus(terminal.state, activityByStream.get(key),
        doneStreams.has(key)) }];
    });
    const next = agentOverviewSince(overviewSinceRef.current, entries, Date.now());
    overviewSinceRef.current = next;
    return next;
  }, [activityByStream, doneStreams, inheritedProfileByTerminal, registry.terminals]);

  // Each pane redraws for its own terminals only: an entry equal to the one
  // drawn last keeps that object, and the list is kept when every entry is.
  const drawnTerminals = useRef<readonly TerminalPaneTerminal[]>([]);
  const terminals = useMemo<readonly TerminalPaneTerminal[]>(() => {
    const shown = { panes: layoutPanes };
    const next = visibleTerminals.map<TerminalPaneTerminal>(terminal => {
      const pane = terminalPaneForTerminal(shown, terminal.id);
      return {
        id: terminal.id,
        title: terminal.title,
        cwd: terminal.cwd,
        sessionKey: terminal.sessionKey,
        status: terminal.state,
        active: pane?.activeTerminalId === terminal.id,
        attached: pane !== undefined,
        exitCode: terminal.exit?.code ?? null,
        activity: activityByStream.get(terminalOutputStreamKey(terminal.id, terminal.generation))
          ?? "idle",
        done: doneStreams.has(terminalOutputStreamKey(terminal.id, terminal.generation)),
        profileId: terminal.launch?.profileId ?? inheritedProfileByTerminal.get(terminal.id),
        preview: previews.get(terminal.id),
        generation: terminal.generation,
        fastMode: terminal.fastMode,
        startupDownload: terminal.startupDownload,
        restartOffer: terminalRestartOffer(terminal,
          terminal.launch?.profileId ?? inheritedProfileByTerminal.get(terminal.id)),
      };
    }).concat(failedReopenTerminals(
      activeWorkspaceTerminals === undefined ? undefined : { terminals: activeWorkspaceTerminals },
      reopenFailures, visibleTerminals, shown)
      .map(entry => ({
        id: entry.id,
        title: entry.title,
        cwd: entry.cwd,
        sessionKey: undefined,
        status: "failed" as const,
        active: terminalPaneForTerminal(shown, entry.id)?.activeTerminalId === entry.id,
        attached: true,
        exitCode: null,
        activity: "idle" as const,
        profileId: entry.profileId,
        preview: [entry.message],
        generation: undefined,
        fastMode: undefined,
      })));
    // A Browser tab is drawn like a terminal's, named by its page. Its folder
    // is the workspace's, which its pane's + menu offers.
    const browserCwd = paneStartDirectory(null, activeWorkspaceTerminals?.map(entry => entry.cwd) ?? [],
      activeCwd) ?? "";
    for (const pane of layoutPanes) {
      for (const tab of pane.tabs) {
        if (!isBrowserTabId(tab.terminalId)) continue;
        const label = browserLabels.get(tab.terminalId);
        const url = label?.url ?? tab.url;
        next.push({
          id: tab.terminalId,
          title: browserTabTitle(label?.title, url),
          cwd: browserCwd,
          ...(url === undefined || url === null ? {} : { url }),
          status: "running",
          active: pane.activeTerminalId === tab.terminalId,
          attached: true,
          exitCode: null,
          activity: "idle",
          profileId: BROWSER_PROFILE_ID,
        });
      }
    }
    drawnTerminals.current = retainEqualItems(drawnTerminals.current, next, terminal => terminal.id);
    return drawnTerminals.current;
  }, [activeCwd, activeWorkspaceTerminals, activityByStream, browserLabels,
    doneStreams, inheritedProfileByTerminal, layoutPanes, previews, reopenFailures, visibleTerminals]);

  const createWorkspace = useCallback((projectPath?: string): void => {
    if (launchPending.current) return;
    if (workspacesRef.current.length >= MAX_TERMINAL_WORKSPACES) {
      report(`${productLabel()} keeps up to ${MAX_TERMINAL_WORKSPACES.toString()} terminal workspaces.`);
      return;
    }
    const record = newTerminalWorkspace(
      `Workspace ${(workspacesRef.current.length + 1).toString()}`,
      () => crypto.randomUUID(), stamp,
    );
    // Empty by construction, so there is nothing for the reopen effect to do.
    reopened.current.add(record.id);
    // New workspaces start at the front; selecting an existing one keeps its slot.
    updateWorkspaces(current => [record, ...current], "now");
    activeWorkspaceIdRef.current = record.id;
    setActiveWorkspaceId(record.id);
    writeActiveWorkspaceId(record.id);
    setLayout(EMPTY_TERMINAL_WORKSPACE_STATE);
    setWorkspaceView("split");
    // Project-menu launches override the previous folder for this new draft.
    // Persist it only when the user launches or explicitly picks a folder.
    adoptCwd(projectPath, false);
    setBrowsedCwd(null);
    setLauncherNotice(null);
  }, [adoptCwd, report, updateWorkspaces]);

  const openPaneTool = useCallback((tool: WorkspaceTool, cwd: string, paneId?: TerminalPaneId) => {
    const root = registryRef.current.terminals.find(
      candidate => candidate.cwd === cwd && candidate.root !== "",
    )?.root ?? cwd;
    if (tool === "browser") openBrowserTab(paneId ?? null, root);
    else if (tool === "files" && previewApi) openOutputPreview(root);
    else onOpenWorkspaceTool?.(tool, root);
  }, [onOpenWorkspaceTool, openBrowserTab, openOutputPreview, previewApi]);

  const callbacks = useMemo<TerminalWorkspaceCallbacks>(() => ({
    onFocusTerminal: attachTerminal,
    onDetachTerminal: terminalId => {
      // Closing a tab ends its terminal (owner, 2026-10-01). A closed tab used
      // to keep running where nothing showed it, and every launch respawned
      // it unseen. A closed Browser tab's page closes with it.
      setLayout(current => closeTerminalTab(current, terminalId));
      if (!isBrowserTabId(terminalId)) endTerminals([terminalId]);
    },
    onFocusPane: paneId => {
      setLayout(current => focusTerminalPane(current, paneId));
    },
    onOpenWorkspaceTool: openPaneTool, onInspectTask,
    onRequestNewTab: paneId => {
      const pane = terminalPaneById(layoutRef.current, paneId);
      // New tab over a page is another page; over a terminal, a shell.
      if (pane !== undefined && isBrowserTabId(pane.activeTerminalId)) {
        openBrowserTab(paneId);
        return;
      }
      const source = pane && terminalById(pane.activeTerminalId);
      if (source) void createTerminal({ cwd: source.cwd, tabPaneId: paneId });
    },
    onRequestNewSession: paneId => {
      const pane = terminalPaneById(layoutRef.current, paneId);
      const source = pane && terminalById(pane.activeTerminalId);
      const cwd = source?.cwd ?? (pane !== undefined && isBrowserTabId(pane.activeTerminalId)
        ? workspaceRoot(activeWorkspaceIdRef.current) : null);
      if (cwd) createWorkspace(cwd);
    },
    onRequestSplit: split,
    onRequestPassOff: passOff,
    onClearTerminal: clearScrollback,
    ...(onAddTerminalSelection === undefined ? {} : { onSendSelection: sendSelection }),
    ...(api.restartTerminal === undefined ? {} : { onRestartTerminal: restartTerminal }),
    onToggleFastMode: terminalId => {
      const terminal = terminalById(terminalId);
      if (terminal?.state !== "running" || terminal.fastMode?.version !== 1
          || !terminal.fastMode.ready) {
        throw new Error("The terminal is not ready for Fast mode.");
      }
      api.sendTerminalInput(terminal.id, terminal.generation,
        textEncoder.encode(TERMINAL_FAST_MODE_KEY));
    },
    onSplitRatioChange: (splitId, ratio) => {
      setLayout(current => setTerminalSplitRatio(current, splitId, ratio));
    },
    onToggleMaximize: paneId => {
      setLayout(current => toggleTerminalPaneMaximized(current, paneId));
    },
    // The equal grid renders cells directly and has no splitter bars, so the
    // same control has to lead back out of it or the layout cannot be resized.
    onTidy: () => {
      const splitIds = onceTerminalIds(() => crypto.randomUUID());
      setLayout(current => current.arrangement === "equal-grid"
        ? setTerminalWorkspaceArrangement(current, "custom-split")
        : tidyTerminalWorkspace(current, splitIds));
    },
    onMoveTerminal: (terminalId, targetPaneId, targetIndex) => {
      setLayout(current => dropTerminalPane(current, terminalId, targetPaneId, targetIndex));
    },
    ...(onContinueAsThread === undefined ? {} : {
      onContinueAsThread: (terminalId: string) => {
        const sessionKey = continueAsThreadKey(terminalById(terminalId));
        if (sessionKey === null) {
          report("This pane has not reported its Claude or Codex session yet, so it cannot continue as a thread.");
          return;
        }
        // One writer: the pane ends first; Rust waits for it to exit before
        // Threads takes the session.
        setLayout(current => closeTerminalTab(current, terminalId));
        endTerminals([terminalId]);
        onContinueAsThread(sessionKey);
      },
    }),
  }), [attachTerminal, clearScrollback, createTerminal, createWorkspace, endTerminals, openBrowserTab,
    openPaneTool, onAddTerminalSelection, onContinueAsThread, onInspectTask, passOff, report,
    sendSelection, split, workspaceRoot]);

  const switchWorkspace = useCallback((workspaceId: string): void => {
    if (launchPending.current) return;
    if (workspaceId === activeWorkspaceIdRef.current) return;
    const target = workspaceById(workspacesRef.current, workspaceId);
    if (target === undefined) return;
    activeWorkspaceIdRef.current = workspaceId;
    setActiveWorkspaceId(workspaceId);
    writeActiveWorkspaceId(workspaceId);
    // Selection preserves the saved sidebar order.
    // The arrangement travels with the workspace, so it is restored here rather
    // than reconciled into place afterwards.
    setLayout(parseTerminalWorkspaceState(target.layout ?? null));
    setWorkspaceView("split");
    setLauncherOpen(false);
    setBrowsedCwd(null);
  }, []);

  const changeOverviewOpen = useCallback((open: boolean): void => {
    setOverviewOpen(open);
    writeAgentOverviewOpen(open);
    if (open) closeOutputPreview();
  }, [closeOutputPreview]);
  const changeOverviewTab = useCallback((tab: AgentOverviewTab): void => {
    setOverviewTab(tab);
    writeAgentOverviewTab(tab);
  }, []);

  // Other workspaces' layouts only change when the roster is saved, and each
  // saved record is read once (savedAgentOverviewWorkspace). The active one is
  // read from the live panes.
  const parkedOverviewWorkspaces = useMemo(() => overviewOpen
    ? new Map(workspaces.filter(record => record.id !== activeWorkspaceId).map(record => [record.id,
      savedAgentOverviewWorkspace(record)]))
    : null, [activeWorkspaceId, overviewOpen, workspaces]);

  const overviewRows = useMemo<readonly AgentOverviewRow[]>(() => {
    if (parkedOverviewWorkspaces === null) return [];
    const terminals = new Map<string, AgentOverviewTerminal>(registry.terminals.map(terminal => [
      terminal.id, {
        id: terminal.id,
        title: terminal.title,
        state: terminal.state,
        activity: activityByStream.get(terminalOutputStreamKey(terminal.id, terminal.generation)),
        done: doneStreams.has(terminalOutputStreamKey(terminal.id, terminal.generation)),
        profileId: terminal.launch?.profileId ?? inheritedProfileByTerminal.get(terminal.id),
        ...(terminal.sessionKey === undefined ? {} : { sessionKey: terminal.sessionKey }),
      },
    ]));
    return agentOverviewRows(workspaces.map(record => parkedOverviewWorkspaces.get(record.id)
      ?? agentOverviewWorkspace(record, { panes: layoutPanes })), terminals);
  }, [activityByStream, doneStreams, inheritedProfileByTerminal, layoutPanes, parkedOverviewWorkspaces,
    registry.terminals, workspaces]);


  // The live cards' last screen lines, read off the mounted xterm buffers
  // while the overview is open: at once, on any activity change, and every
  // two seconds for agents that print without changing state.
  const [overviewTails, setOverviewTails] = useState<ReadonlyMap<string, readonly string[]>>(
    () => new Map());
  const overviewRowsRef = useRef(overviewRows);
  overviewRowsRef.current = overviewRows;
  useEffect(() => {
    if (!overviewOpen) {
      setOverviewTails(current => (current.size === 0 ? current : new Map()));
      return undefined;
    }
    if (!visible || !documentVisible || overviewTab !== "agents") return undefined;
    const read = (): void => {
      const next = new Map<string, readonly string[]>();
      for (const row of overviewRowsRef.current) {
        if (row.status === "idle" || row.status === "stopped") continue;
        const terminal = terminalById(row.terminalId);
        if (terminal === undefined) continue;
        const lines = streams.view(terminalOutputStreamKey(terminal.id, terminal.generation))
          ?.tailLines(40) ?? [];
        const tail = terminalCardPreview(lines, terminal.cwd, row.profileId, 3);
        if (tail.length > 0) next.set(row.terminalId, tail);
      }
      setOverviewTails(current => sameTails(current, next) ? current : next);
    };
    read();
    const timer = window.setInterval(read, 2_000);
    return () => { window.clearInterval(timer); };
  }, [activityByStream, documentVisible, overviewOpen, overviewTab, streams, terminalById, visible]);

  // Close idle (owner, 2026-10-01): end each idle agent the way deleting its
  // workspace would, after the overview's own confirmation.
  const closeIdleAgents = useCallback((terminalIds: readonly string[]): void => {
    if (launchPending.current) return;
    endTerminals(terminalIds);
  }, [endTerminals]);

  const overviewProfileLabel = useCallback((profileId: string): string =>
    launchProfiles.find(profile => profile.id === profileId)?.label
      ?? providerLabel(profileId === "gemini" ? "acp:gemini" : profileId), [launchProfiles]);

  // Go to an agent from the overview: its workspace, its tab (reopened if it
  // was closed while still running), then one pulse on the pane holding it.
  const locateTerminal = useCallback((row: AgentOverviewRow): void => {
    if (launchPending.current) return;
    if (workspaceById(workspacesRef.current, row.workspaceId) === undefined) return;
    if (row.workspaceId !== activeWorkspaceIdRef.current) switchWorkspace(row.workspaceId);
    setLauncherOpen(false);
    setBrowsedCwd(null);
    attachTerminal(row.terminalId);
    locateNonce.current += 1;
    setLocate({ terminalId: row.terminalId, nonce: locateNonce.current });
    if (window.matchMedia?.(AGENT_OVERVIEW_FLOATS_QUERY).matches === true) changeOverviewOpen(false);
  }, [attachTerminal, changeOverviewOpen, switchWorkspace]);
  // A request is spent once its pulse has run. Kept any longer, a pane that
  // mounts later holding that terminal — a workspace switch back, a tab dragged
  // to another pane — would pulse for a choice made long before.
  useEffect(() => {
    if (locate === null) return undefined;
    const timer = window.setTimeout(() => {
      setLocate(current => (current === locate ? null : current));
    }, TERMINAL_PANE_LOCATE_MS + 100);
    return () => { window.clearTimeout(timer); };
  }, [locate]);

  useImperativeHandle(ref, () => ({
    controlProject: () => terminalById(activeTerminalId(layoutRef.current) ?? "")?.root ?? null,
    browserEvent: event => { BrowserPage.forWebContents(event.page)?.handle(event); },
    closeFocusedPane: () => {
      // The launcher covers the panes and has its own close: never a pane behind it.
      if (launchPending.current || terminalLauncherShowing(launcherOpen, layoutRef.current)) return false;
      const paneId = layoutRef.current.focusedPaneId;
      const pane = paneId === null ? undefined : terminalPaneById(layoutRef.current, paneId);
      if (pane === undefined) return false;
      callbacks.onDetachTerminal(pane.activeTerminalId);
      return true;
    },
    openReviewDialog: openReview,
    openResumedSession: input => {
      // Its own workspace, unless the roster is full: then beside the panes.
      if (workspacesRef.current.length < MAX_TERMINAL_WORKSPACES) createWorkspace(input.cwd);
      void createTerminal({
        cwd: input.cwd,
        ...(input.title === undefined ? {} : { title: input.title }),
        launch: { version: 1, profileId: input.profileId },
        launchSession: { version: 1, id: input.providerSessionId, resume: true },
      });
    },
    focusTerminal: (terminalId: string) => {
      const home = workspaceForTerminal(workspacesRef.current, terminalId);
      if (home !== undefined && home.id !== activeWorkspaceIdRef.current) switchWorkspace(home.id);
      // Both are functional layout updates, so the focus lands on the layout
      // the switch just restored.
      attachTerminal(terminalId);
    },
    openBrowser: (url: string, terminalId: string) => {
      const page = browserPageUrl(url);
      if (page === null) return false;
      const home = workspaceForTerminal(workspacesRef.current, terminalId);
      if (home !== undefined && home.id !== activeWorkspaceIdRef.current) switchWorkspace(home.id);
      return openBrowserPane({ terminalId }, terminalById(terminalId)?.cwd ?? null, page) !== null;
    },
    openTool: (tool: "files" | "browser") => {
      const cwd = contextualCwdRef.current;
      // A tab in the focused pane, as the pane + menu opens one there. At
      // the tab limit it has said so; that is no missing project.
      if (tool === "browser") { openBrowserTab(null, cwd); return true; }
      if (cwd === null || previewApi === null || activeWorkspaceIdRef.current === null) return false;
      openOutputPreview(cwd);
      return true;
    },
    openSignIn: (profileId: TerminalLaunchProfileId, label: string, cwd: string,
        account?: string, title?: string) => {
      createWorkspace();
      void createTerminal({
        cwd, title: title ?? `Sign in to ${label}`, launch: { version: 1, profileId }, signIn: true,
        ...(account === undefined ? {} : { account }),
      });
    },
  }), [attachTerminal, callbacks, createTerminal, createWorkspace, launcherOpen, openBrowserPane,
    openBrowserTab, openOutputPreview, openReview, previewApi, switchWorkspace, terminalById]);

  const renameWorkspace = useCallback((name: string): void => {
    const workspaceId = workspaceRenameId;
    setWorkspaceRenameId(null);
    const normalized = normalizeWorkspaceName(name);
    if (workspaceId === null || normalized === "") return;
    updateWorkspaces(current => current.map(record => record.id === workspaceId
      ? { ...record, name: normalized, updatedAt: stamp() } : record), "now");
  }, [updateWorkspaces, workspaceRenameId]);

  const requestDeleteWorkspace = useCallback((workspaceId: string): void => {
    if (launchPending.current) return;
    if (workspaceById(workspacesRef.current, workspaceId) === undefined) return;
    setDeleteWorkspaceId(workspaceId);
  }, []);

  const confirmDeleteWorkspace = useCallback((): void => {
    if (launchPending.current) return;
    const workspaceId = deleteWorkspaceId;
    setDeleteWorkspaceId(null);
    if (workspaceId === null) return;
    const current = workspacesRef.current;
    const target = workspaceById(current, workspaceId);
    if (target === undefined) return;
    const claimed = new Set(target.terminals.map(entry => entry.id));
    setInheritedProfileByTerminal(profiles => new Map(
      [...profiles].filter(([terminalId]) => !claimed.has(terminalId)),
    ));
    for (const terminal of registryRef.current.terminals) {
      if (!claimed.has(terminal.id)) continue;
      if (terminal.state === "running" || terminal.state === "starting") {
        retirePending.current.add(terminal.id);
        void api.closeTerminal({
          id: terminal.id,
          generation: terminal.generation,
        }).catch(error => {
          retirePending.current.delete(terminal.id);
          report(error);
        });
      } else {
        dismiss(terminal.id);
      }
    }
    const wasActive = activeWorkspaceIdRef.current === workspaceId;
    const nextId = neighboringWorkspaceId(current, workspaceId);
    const remaining = forgetWorkspace(current, workspaceId);
    updateWorkspaces(() => remaining, "now");
    if (remaining.length === 0) {
      createWorkspace();
      return;
    }
    if (wasActive && nextId !== null) switchWorkspace(nextId);
  }, [createWorkspace, deleteWorkspaceId, dismiss, report, switchWorkspace, updateWorkspaces]);

  // The switcher lands in App state (and the Sidebar). A splitter drag makes a
  // new layout, and so a new saved record, every frame; the rows it shows stay
  // equal, and keeping their identity keeps the switcher and App unchanged.
  const activeOpenIds = useMemo(() => [...layoutTerminalIds], [layoutTerminalIds]);
  const switcherRecordsRef = useRef<TerminalWorkspaceSwitcher["records"] | undefined>(undefined);
  const switcherRecords = useMemo(() => {
    const next = terminalSwitcherRecords(switcherRecordsRef.current, workspaces,
      activeWorkspaceId, activeOpenIds);
    switcherRecordsRef.current = next;
    return next;
  }, [activeOpenIds, activeWorkspaceId, workspaces]);
  const switcher = useMemo<TerminalWorkspaceSwitcher | null>(() => workspacesLoaded ? {
    records: switcherRecords,
    activeId: activeWorkspaceId,
    onSelect: switchWorkspace,
    onCreate: createWorkspace,
    onRename: workspaceId => {
      if (launchPending.current
          || workspaceById(workspacesRef.current, workspaceId) === undefined) return;
      setWorkspaceRenameId(workspaceId);
    },
    onDelete: requestDeleteWorkspace,
    busy: launcherBusy,
    canCreate: !launcherBusy && workspaces.length < MAX_TERMINAL_WORKSPACES,
  } : null, [activeWorkspaceId, createWorkspace, requestDeleteWorkspace, switchWorkspace,
    launcherBusy, switcherRecords, workspaces.length, workspacesLoaded]);
  useEffect(() => { onSwitcherChange?.(switcher); }, [onSwitcherChange, switcher]);

  const deletingWorkspace = deleteWorkspaceId === null
    ? undefined : workspaceById(workspaces, deleteWorkspaceId);
  const deletingCopy = deletingWorkspace === undefined
    ? null : workspaceDeleteConfirm(deletingWorkspace.name, deletingWorkspace.terminals.length);

  const renamedWorkspace = workspaceRenameId === null
    ? undefined : workspaceById(workspaces, workspaceRenameId);

  return <>
    <TerminalViewPool streamKeys={terminalStreamKeys} workingKeys={workingStreams}
      renderTerminal={renderTerminalView}>
    <TerminalWorkspace state={layout} terminals={terminals} activeCwd={contextualCwd}
      callbacks={callbacks} renderTerminal={renderTerminal}
      view={workspaceView} onViewChange={setWorkspaceView}
      toolbarSlot={toolbarSlot}
      onMenuChange={onMenuChange}
      overview={{
        open: overviewOpen,
        rows: overviewRows,
        profileLabel: overviewProfileLabel,
        tails: overviewTails,
        since: overviewSince,
        onCloseIdle: closeIdleAgents,
        ...(onOpenTasks === undefined ? {} : { onOpenTasks }),
        browser: <BrowserColumn url={overviewBrowserUrl.current} active={overviewTab === "browser"}
          servers={browserServers.get(BROWSER_COLUMN_ID)} onFindServers={findBrowserServers}
          onNavigated={rememberOverviewBrowserUrl}
          onOpenExternal={openWorkspaceTool === undefined ? undefined : openBrowserOutside}
          pointTargets={outputFeedbackTargets} onAddToDraft={onAddBrowserNote} />,
        tab: overviewTab,
        onTabChange: changeOverviewTab,
        onOpenChange: changeOverviewOpen,
        onLocate: locateTerminal,
      }}
      outputPreview={previewApi ? {
        open: outputRoot !== null,
        present: previewPresence.present,
        canOpen: contextualCwd !== null,
        onToggle: () => { if (outputRoot) closeOutputPreview(); else if (contextualCwd) openOutputPreview(contextualCwd); },
        panel: <OutputPreviewPanel root={shownOutputRoot} visible={visible && !interactionBlocked}
          phase={previewPresence.phase} onAnimationEnd={previewPresence.onAnimationEnd}
          api={previewApi} openRequest={outputOpenRequest}
          feedbackTargets={outputFeedbackTargets} onSendFeedback={onSendOutputFeedback}
          onClose={closeOutputPreview} onOpenFolder={openOutputFolder} />,
      } : undefined}
      browserLayer={browserPages.setLayer}
      locate={locate}
      launcher={{
        open: launcherOpen,
        cwd: launcherCwd,
        visible: visible && !interactionBlocked,
        profiles: launchProfiles,
        profilesLoading: launchProfilesLoading,
        launching: launcherBusy,
        notice: launcherNotice,
        onOpen: openLauncher,
        onClose: () => {
          if (launchPending.current) return;
          setLauncherOpen(false);
          setBrowsedCwd(null);
        },
        onLaunch: launchTerminalBatch,
        ...(headroom === undefined ? {} : { headroom }),
        defaultProfileId: lastProfileId,
        defaultCount: lastCount,
        ...(reviewConnected ? { onOpenReview: openReview } : {}),
        onBrowse: preferredPath => api.pickDirectory(
          preferredPath || launcherCwd || undefined,
        ).then(picked => {
          adoptCwd(picked, true);
          return picked;
        }),
        onCwdChange: setBrowsedCwd,
        onRetryProfiles: retryLaunchProfiles,
        onRefreshProfiles: refreshLaunchProfiles,
      }} />
    </TerminalViewPool>
    <AdversarialReviewDialog open={reviewConnected && reviewOpen}
      profiles={launchProfiles} cwd={launcherCwd} launching={launcherBusy}
      notice={launcherNotice}
      onStart={launchAdversarialReview}
      onCancel={() => {
        if (launchPending.current) return;
        setReviewOpen(false);
        setLauncherNotice(null);
        // Only when the dialog WAS the session. Cancelling it over a launcher
        // that is still open would pull the folder out from under a form the
        // user is still looking at.
        if (!terminalLauncherShowing(launcherOpen, layout)) setBrowsedCwd(null);
      }} />
    <TerminalSelectionDialog open={selectionSend !== null && onAddTerminalSelection !== undefined}
      paneTitle={selectionSend?.title ?? ""} selection={selectionSend?.selection ?? ""}
      targets={outputFeedbackTargets ?? []}
      onChoose={key => {
        const chosen = selectionSend;
        setSelectionSend(null);
        if (chosen === null || onAddTerminalSelection === undefined) return;
        if (!onAddTerminalSelection(key, chosen.title, chosen.selection)) {
          onError("That selection was empty; nothing was added to the thread's draft.");
        }
      }}
      onCancel={() => { setSelectionSend(null); }} />
    <TerminalRenameDialog open={renamedWorkspace !== undefined}
      title={renamedWorkspace?.name ?? "Workspace"} heading="Rename workspace"
      onSubmit={renameWorkspace}
      onCancel={() => { setWorkspaceRenameId(null); }} />
    <ConfirmDialog open={deletingCopy !== null}
      title={deletingCopy?.title ?? "Delete workspace?"}
      description={deletingCopy?.description ?? ""}
      confirmLabel="Delete workspace" tone="danger" onConfirm={confirmDeleteWorkspace}
      onCancel={() => { setDeleteWorkspaceId(null); }} />
  </>;
}
