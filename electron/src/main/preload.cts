import type { CustomizationCommand, ProviderCustomization } from "../shared/ipc.js";
import type { PromptRecoveryInput, RecoveryRequest, RecoveryResponse } from "../shared/recovery.js";
import type { OutputPreviewRequest, OutputPreviewResponse } from "../shared/output-preview.js";
import { contextBridge, ipcRenderer, webUtils } from "electron";
import type { IpcRendererEvent } from "electron";
import type {
  EngineEvent, EngineUsage, LaunchOptions, SessionKey, SessionSummary,
  ProviderCommandCatalogue, ProviderCommandScope,
} from "../shared/engine.js";
import type {
  AcpProfile, AcpProfilesUpdate, CHANNELS as ChannelMap, CustomizationInventory,
  CollaborationApprovalAction, CollaborationDraftStateUpdate, CollaborationEventDto,
  CollaborationPeerAction, CollaborationStatusDto, CollaborationSubmitRequest,
  EngineOpSupport, EngineProbe,
  FileSnapshot, HistoryPage, HistoryReadOptions, IfaceApi,
  ManagedWorktreeCleanup, ManagedWorktreeInfo, OpenTerminalResult, OpenWorkspaceToolV1,
  OrchestrationAgentReplyRequest, OrchestrationAgentSteerRequest,
  OrchestrationAgentStopRequest, OrchestrationAgentDto,
  OrchestrationEventDto,
  OrchestrationInboxRequest,
  OrchestrationRoleClaimRequest, OrchestrationRoleRelinquishRequest,
  OrchestrationSnapshotDto, OrchestrationTranscriptDto,
  OrchestrationTimelineDto, OrchestrationTimelineRequest, OrchestrationTranscriptRequest,
  OrchestrationWorktreeEscrowRequest,
  PromptContextGrantResult, PromptImagePasteResult,
  ProjectFileListResponse, ProjectSearchResponse, PromptContextDescriptor,
  PromptContextValidation, PromptContextValidationTarget, RepositoryReview, ReviewCheckDiscovery,
  RequestedReviewCheckpointKind,
  ReviewCheckName, ReviewCheckRunResult, ReviewCheckpointDetail,
  ReviewCheckpointRestoreResult, ReviewCheckpointSummary,
  SessionActivationResult, SessionListingSnapshot, SessionSearchResponse,
  CreateTerminalRequest, DismissTerminalResult, RenameTerminalRequest,
  ResizeTerminalRequest, TerminalWorkspaceActivityRequest, TerminalGenerationRequest, TerminalId,
  TerminalDataEvent, TerminalLaunchProfile, TerminalLifecycleEvent,
  TerminalSnapshot, TerminalSummary, TerminalWorkspaceRecord, MovedTerminalFolder,
  ReadTerminalDelegation, TerminalDelegationView,
  TerminalControlsRequestV1, TerminalControlsStateV1,
  VoiceEvent, VoiceSetEnabledResponse, VoiceUiCall,
} from "../shared/ipc.js";
import type { MailboxMessageV1, OrchestratorPolicyV1 } from "../shared/orchestration.js";
import type {
  AdversarialReviewStartRequestV1, AdversarialReviewStartV1,
  OrchestrationAgentCreateRequestV1, OrchestrationAgentSendRequestV1,
  OrchestrationInboxPageV1,
  OrchestrationAgentBindParentRequestV1, OrchestrationAgentRegisterSessionRequestV1,
  OrchestratorPolicyClearRequestV1, OrchestratorPolicySetRequestV1,
  CoordinationStatusRequestV1, CoordinationStatusV1,
  OrchestratorModelsV1, OrchestratorSubmitResultV1,
  WorkerAbandonRequestV1, WorkerAckRequestV1, WorkerIntegrateRequestV1, WorkerSubmitRequestV1,
} from "../shared/orchestration-control.js";
import type { AppearanceChange, AppearanceState } from "../shared/appearance.js";
import type { UpdateState } from "../shared/updater.js";
// Types only, so nothing from the main process is pulled into the renderer at
// runtime — see the note below on why a runtime import is impossible here.
// bridge.ts is also what augments IfaceApi with the import half of the API, so
// importing from it is what makes `api` below required to expose all of it.
import type {
  EXTRA_CHANNELS as ExtraChannelMap, ImportProgress, ImportReport, WorktreeNotice,
} from "./bridge.js";
import type * as MainTerminal from "./terminal-bridge-controller.js";
import type * as QuitGuard from "../shared/quit-guard.js";
import type * as DockAttention from "../shared/dock-attention.js";
import type * as VisualFrame from "../shared/visual-frame.js";
import type * as AppMenu from "../shared/app-menu.js";
import type * as BrowserEvents from "../shared/browser-events.js";

// This file is loaded into a sandboxed renderer, where `require` is polyfilled
// down to electron and a couple of node builtins — a relative file is not
// resolvable, so CHANNELS (shared/ipc.ts) and EXTRA_CHANNELS (bridge.ts, the
// channels the bridge owns, including one CLI-internal live-only send that is
// intentionally not exposed below) cannot be imported at runtime. Both tables
// assert at compile time that every channel is its key under `iface:`, so the
// string is built from the key here, and the key is checked against those
// tables: a channel renamed, added or dropped there fails this file at compile
// time rather than silently going unhandled.
type ChannelTable = typeof ChannelMap & typeof ExtraChannelMap;
const ch = <K extends keyof ChannelTable>(key: K): ChannelTable[K] =>
  `iface:${key}` as ChannelTable[K];

// Raw PTY bytes deliberately bypass invoke/send and the JSON host protocol.
// The main process transfers one private MessagePort after navigation is
// committed; only this sandboxed preload sees the port itself.
// Typed against main's handshake, so the two ends cannot drift apart.
const TERMINAL_DATA_PORT_CHANNEL: typeof MainTerminal.TERMINAL_DATA_PORT_CHANNEL =
  "iface:terminalDataPort";
const TERMINAL_DATA_FAILURE_CHANNEL: typeof MainTerminal.TERMINAL_DATA_FAILURE_CHANNEL =
  "iface:terminalDataFailure";
const TERMINAL_DATA_PORT_REQUEST_CHANNEL: typeof MainTerminal.TERMINAL_DATA_PORT_REQUEST_CHANNEL =
  "iface:terminalDataPortRequest";
const TERMINAL_DATA_PORT_ACK_CHANNEL: typeof MainTerminal.TERMINAL_DATA_PORT_ACK_CHANNEL =
  "iface:terminalDataPortAck";
const WORK_IN_FLIGHT_CHANNEL: typeof QuitGuard.WORK_IN_FLIGHT_CHANNEL = "iface:workInFlight";
const DOCK_ATTENTION_CHANNEL: typeof DockAttention.DOCK_ATTENTION_CHANNEL = "iface:dockAttention";
const VISUAL_FRAME_CHANNEL: typeof VisualFrame.VISUAL_FRAME_CHANNEL = "iface:visualFrame";
const MENU_COMMAND_CHANNEL: typeof AppMenu.MENU_COMMAND_CHANNEL = "iface:menuCommand";
const BROWSER_EVENT_CHANNEL: typeof BrowserEvents.BROWSER_EVENT_CHANNEL = "iface:browserEvent";
const TERMINAL_DATA_MAX_PAYLOAD_BYTES = 64 * 1024;
const TERMINAL_DATA_MAX_BACKLOG_EVENTS = 256;
const TERMINAL_DATA_MAX_FAILURE_CHARS = 1024;
// Mirrors Rust's 32 retained-terminal ceiling and the main transport bound.
const TERMINAL_DATA_MAX_RETIRED_TERMINALS = 32;
const TERMINAL_DATA_UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/u;
const TERMINAL_DATA_BRIDGE_TOKEN = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u;
const terminalDataListeners = new Set<(event: TerminalDataEvent, complete: () => void) => void>();
const terminalDataFailureListeners = new Set<(message: string) => void>();
const terminalInputSequences = new Map<string, bigint>();
const retiredTerminalInputGenerations = new Map<string, number>();
const terminalDataBacklog: TerminalDataEvent[] = [];
let terminalDataBacklogBytes = 0;
let terminalDataPort: MessagePort | null = null;
let retainedTerminalDataFailure: string | null = null;

function terminalDataRecord(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown> : null;
}

function recordTerminalDataFailure(message: string): void {
  const normalized = message.trim().slice(0, TERMINAL_DATA_MAX_FAILURE_CHARS);
  if (normalized === "") return;
  terminalInputSequences.clear();
  terminalDataPort?.close();
  terminalDataPort = null;
  if (retainedTerminalDataFailure !== null) return;
  retainedTerminalDataFailure = normalized;
  for (const listener of terminalDataFailureListeners) listener(normalized);
}

function failTerminalDataPort(message: string): void {
  try {
    terminalDataPort?.postMessage({
      channel: "interface-terminal-data",
      version: 2,
      transportFailure: true,
    });
  } catch {
    // Closing the port below is still the fail-closed renderer boundary. The
    // malformed notice is best-effort so main can distinguish it from reload.
  }
  recordTerminalDataFailure(message);
}

function retireTerminalInput(terminalId: string, generation: number): void {
  const previous = retiredTerminalInputGenerations.get(terminalId);
  if (previous !== undefined && generation <= previous) return;
  retiredTerminalInputGenerations.delete(terminalId);
  retiredTerminalInputGenerations.set(terminalId, generation);
  while (retiredTerminalInputGenerations.size > TERMINAL_DATA_MAX_RETIRED_TERMINALS) {
    const oldest = retiredTerminalInputGenerations.keys().next().value as string | undefined;
    if (oldest === undefined) return;
    retiredTerminalInputGenerations.delete(oldest);
  }
}

function acceptTerminalData(value: unknown): void {
  const message = terminalDataRecord(value);
  if (message === null || message.channel !== "interface-terminal-data" || message.version !== 2
      || message.direction !== 1 || (message.kind !== 1 && message.kind !== 2)
      || typeof message.terminalId !== "string" || !TERMINAL_DATA_UUID.test(message.terminalId)
      || typeof message.generation !== "string"
      || !/^[1-9][0-9]*$/u.test(message.generation)
      || typeof message.sequence !== "string" || !/^(0|[1-9][0-9]*)$/u.test(message.sequence)
      || !(message.payload instanceof ArrayBuffer)) {
    failTerminalDataPort("The integrated terminal data channel received an invalid frame.");
    return;
  }
  const generation = Number(message.generation);
  if (!Number.isSafeInteger(generation) || generation < 1
      || message.payload.byteLength > TERMINAL_DATA_MAX_PAYLOAD_BYTES
      || (message.kind === 2 && message.payload.byteLength !== 0)) {
    failTerminalDataPort("The integrated terminal data channel received an invalid frame.");
    return;
  }
  const event: TerminalDataEvent = {
    terminalId: message.terminalId,
    generation,
    sequence: message.sequence,
    kind: message.kind === 2 ? "eof" : "data",
    // MessagePort structured-clone already minted a private ArrayBuffer.
    // slice(0) would copy every PTY byte again. A resizable payload would
    // length-track after the byteLength check; the trusted peer does not send one.
    data: new Uint8Array(message.payload),
  };
  const port = terminalDataPort;
  const complete = (): void => {
    if (port === null || port !== terminalDataPort) return;
    try {
      port.postMessage({ channel: "interface-terminal-data", version: 2,
        terminalId: event.terminalId, generation: message.generation, sequence: event.sequence,
        direction: 2, kind: 3, payload: new ArrayBuffer(0) });
    } catch { recordTerminalDataFailure("The terminal output acknowledgement could not be delivered."); }
  };
  if (event.kind === "eof") {
    const key = `${event.terminalId}:${event.generation.toString()}`;
    terminalInputSequences.delete(key);
    retireTerminalInput(event.terminalId, event.generation);
  }
  if (terminalDataListeners.size === 0) {
    // The bounded startup tail owns these bytes; no renderer write is pending.
    complete();
    terminalDataBacklog.push(event);
    terminalDataBacklogBytes += event.data.byteLength;
    while ((terminalDataBacklogBytes > 1024 * 1024
      || terminalDataBacklog.length > TERMINAL_DATA_MAX_BACKLOG_EVENTS)
      && terminalDataBacklog.length > 1) {
      terminalDataBacklogBytes -= terminalDataBacklog.shift()?.data.byteLength ?? 0;
    }
    return;
  }
  const listeners = [...terminalDataListeners];
  let remaining = listeners.length;
  for (const listener of listeners) {
    let consumed = false;
    const done = (): void => {
      if (consumed) return;
      consumed = true;
      if (--remaining === 0) complete();
    };
    try { listener(event, done); }
    catch { done(); failTerminalDataPort("A terminal output consumer failed."); }
  }
}

ipcRenderer.on(TERMINAL_DATA_PORT_CHANNEL, (event: IpcRendererEvent, value: unknown) => {
  const port = event.ports[0];
  if (port === undefined) return;
  const handshake = terminalDataRecord(value);
  const token = handshake?.token;
  if (handshake?.version !== 1 || typeof token !== "string"
      || !TERMINAL_DATA_BRIDGE_TOKEN.test(token)) {
    port.close();
    recordTerminalDataFailure(
      "The integrated terminal data channel received an invalid secure handshake.",
    );
    return;
  }
  if (retainedTerminalDataFailure !== null) {
    port.close();
    return;
  }
  terminalDataPort?.close();
  terminalDataPort = port;
  terminalInputSequences.clear();
  port.onmessage = message => { acceptTerminalData(message.data); };
  port.onmessageerror = () => {
    failTerminalDataPort("The integrated terminal data channel could not decode a frame.");
  };
  port.start();
  ipcRenderer.send(TERMINAL_DATA_PORT_ACK_CHANNEL, { version: 1, token });
});

ipcRenderer.on(TERMINAL_DATA_FAILURE_CHANNEL, (_event: IpcRendererEvent, message: unknown) => {
  if (typeof message !== "string" || message.trim() === "") return;
  recordTerminalDataFailure(message);
});

// Preload initiates the transfer only after both private listeners exist. Main
// authenticates this exact frame and will not admit terminal creation until it
// receives the token acknowledgement above.
ipcRenderer.send(TERMINAL_DATA_PORT_REQUEST_CHANNEL, { version: 1 });

function sendTerminalInput(id: TerminalId, generation: number, supplied: Uint8Array): void {
  if (retainedTerminalDataFailure !== null) throw new Error(retainedTerminalDataFailure);
  if (!TERMINAL_DATA_UUID.test(id) || !Number.isSafeInteger(generation) || generation < 1
      || !(supplied instanceof Uint8Array)) {
    throw new TypeError("The integrated terminal input envelope is invalid.");
  }
  const key = `${id}:${generation.toString()}`;
  // Output EOF seals this generation. Xterm or a stale paste/drop callback may
  // still attempt input briefly; discard it locally instead of endangering the
  // shared transport used by other terminals.
  if ((retiredTerminalInputGenerations.get(id) ?? 0) >= generation) return;
  const port = terminalDataPort;
  if (port === null) {
    throw new Error("The integrated terminal data channel is still connecting.");
  }
  let sequence = terminalInputSequences.get(key) ?? 0n;
  for (let offset = 0; offset < supplied.byteLength; offset += TERMINAL_DATA_MAX_PAYLOAD_BYTES) {
    const payload = new Uint8Array(supplied.subarray(
      offset, Math.min(supplied.byteLength, offset + TERMINAL_DATA_MAX_PAYLOAD_BYTES),
    )).buffer;
    try {
      port.postMessage({
        channel: "interface-terminal-data",
        version: 2,
        terminalId: id,
        generation: generation.toString(),
        sequence: sequence.toString(),
        direction: 2,
        kind: 1,
        payload,
      });
    } catch (error) {
      failTerminalDataPort("The integrated terminal data channel could not send input.");
      throw error;
    }
    sequence += 1n;
  }
  terminalInputSequences.set(key, sequence);
}

// The one place every channel's rejection passes through, which is why the
// unwrapping lives here: Electron rewrites a main-process rejection as
// `Error invoking remote method '<channel>': Error: <the real message>`, and
// that whole string was reaching the user's file tree verbatim. Strip the
// wrapper once and no channel can leak it.
//
// Only the wrapper. Nothing is humanised here — the Rust host does that before
// the message ever gets wrapped.
//
// A rejection stays a rejection: setModel/setEffort depend on it (the renderer
// catches and reverts the pill), and every other caller's catch keeps firing.
//
// A Rust host error ends in the v1 error shape (shared/ipc-error.ts, which this
// file cannot import): one `[iface-error/<n> …]` marker carrying the host's
// code, retryable and replaySafe. It passes through untouched, after the
// cleaned words, for the renderer to read; only the words are cleaned.
const IPC_ERROR_MARKER = /\s*(\[iface-error\/\d{1,6}(?: [^\]\n]*)?\])$/u;
const BUILD_CHANNEL_ARG_PREFIX = "--interface-build-channel=";
const buildChannel = process.argv.some(argument => argument === `${BUILD_CHANNEL_ARG_PREFIX}dev`)
  ? "dev" as const
  : "release" as const;
const productName = buildChannel === "dev" ? "Interface Dev" : "Interface";

function invoke<T>(channel: string, ...args: unknown[]): Promise<T> {
  return ipcRenderer.invoke(channel, ...args).catch((err: unknown) => {
    const raw = err instanceof Error ? err.message : String(err);
    const marker = IPC_ERROR_MARKER.exec(raw);
    const clean = (marker === null ? raw : raw.slice(0, marker.index))
      .replace(/^Error invoking remote method '[^']*':\s*/, "")
      // The main-process error's own `Name: ` prefix, added when Electron
      // stringified it into the wrapper — or the bare name on its own, which is
      // all `${err}` yields for an error with an empty message. Only these four
      // names, so a message that legitimately starts with a word and a colon
      // survives intact.
      .replace(/^(?:Error|TypeError|RangeError|SyntaxError)(?::\s*|$)/, "")
      .trim();
    const message = clean.length > 0 ? clean : `${productName} could not complete that request.`;
    throw new Error(marker === null ? message : `${message} ${marker[1]}`);
  }) as Promise<T>;
}

// Every push subscription hands back its own unsubscribe: the listener identity
// stays inside this closure, so a component that mounts twice cannot remove
// another one's handler.
function subscribe<A extends unknown[]>(channel: string, cb: (...args: A) => void): () => void {
  const listener = (_e: IpcRendererEvent, ...args: A): void => { cb(...args); };
  ipcRenderer.on(channel, listener);
  return () => { ipcRenderer.removeListener(channel, listener); };
}

// Annotated as IfaceApi so the exposed surface is checked against the contract
// the renderer compiles against; contextBridge deep-clones every argument and
// proxies the callbacks, so nothing here hands the page a live main-process
// object.
const api: IfaceApi = {
  remoteAccess: request => invoke(ch("remoteAccess"), request),
  getAppearance: () => invoke<AppearanceState>(ch("getAppearance")),
  reportWorkInFlight: work => { ipcRenderer.send(WORK_IN_FLIGHT_CHANNEL, work); },
  reportDockAttention: report => { ipcRenderer.send(DOCK_ATTENTION_CHANNEL, report); },
  visualFrame: request => invoke<string | null>(VISUAL_FRAME_CHANNEL, request),
  setAppearance: (preference: AppearanceChange) =>
    invoke<AppearanceState>(ch("setAppearance"), preference),
  getAcpProfiles: () => invoke<AcpProfile[]>(ch("getAcpProfiles")),
  setAcpProfiles: (profiles: AcpProfile[]) =>
    invoke<AcpProfilesUpdate>(ch("setAcpProfiles"), profiles),
  inspectTerminalServers: (request: { id: string; generation: number }) => invoke<import("../shared/harness.js").TerminalServerLink[]>(ch("inspectTerminalServers"), request),
  harnessControl: (command: import("../shared/harness.js").HarnessCommand) => invoke<import("../shared/harness.js").HarnessView>(ch("harnessControl"), command),
  threadControl: (command: import("../shared/threads.js").ThreadCommand) =>
    invoke<import("../shared/threads.js").ThreadControlResponse>(ch("threadControl"), command),
  threadJournal: (command: import("../shared/thread-journal.js").ThreadJournalCommand) =>
    invoke<import("../shared/thread-journal.js").ThreadJournalResponse>(ch("threadJournal"), command),
  providerAccounts: (command: import("../shared/provider-accounts.js").ProviderAccountsCommand) =>
    invoke<import("../shared/provider-accounts.js").ProviderAccountsView>(ch("providerAccounts"), command),
  draftStash: (command: import("../shared/draft-stash.js").DraftStashCommand) =>
    invoke<import("../shared/draft-stash.js").DraftStashView>(ch("draftStash"), command),
  threadSecret: (command: import("../shared/thread-secrets.js").ThreadSecretCommand) =>
    invoke<import("../shared/thread-secrets.js").ThreadSecretsView>(ch("threadSecret"), command),
  agentControl: (command: import("../shared/agents.js").AgentCommand) =>
    invoke<import("../shared/agents.js").AgentControlResponse>(ch("agentControl"), command),
  getTerminalWorkspaces: () =>
    invoke<TerminalWorkspaceRecord[]>(ch("getTerminalWorkspaces")),
  setTerminalWorkspaces: (workspaces: TerminalWorkspaceRecord[],
    movedFolders?: readonly MovedTerminalFolder[]) => movedFolders === undefined
    ? invoke<TerminalWorkspaceRecord[]>(ch("setTerminalWorkspaces"), workspaces)
    : invoke<TerminalWorkspaceRecord[]>(ch("setTerminalWorkspaces"), workspaces, movedFolders),
  resolveMovedTerminalFolder: (path: string) =>
    invoke<string | null>(ch("resolveMovedTerminalFolder"), path),
  updateSessions: (command) => invoke<import("../shared/update-sessions.js").UpdateSessionsView>(ch("updateSessions"), command),
  getUpdateState: () => invoke<UpdateState>(ch("getUpdateState")),
  checkForUpdates: () => invoke<UpdateState>(ch("checkForUpdates")),
  restartToUpdate: () => invoke<boolean>(ch("restartToUpdate")),
  getOrchestrationSnapshot: () =>
    invoke<OrchestrationSnapshotDto>(ch("getOrchestrationSnapshot")),
  readOrchestrationTimeline: (input: OrchestrationTimelineRequest) =>
    invoke<OrchestrationTimelineDto>(ch("readOrchestrationTimeline"), input),
  createOrchestrationAgent: (input: OrchestrationAgentCreateRequestV1) =>
    invoke<OrchestrationAgentDto>(ch("createOrchestrationAgent"), input),
  startAdversarialReview: (input: AdversarialReviewStartRequestV1) =>
    invoke<AdversarialReviewStartV1>(ch("startAdversarialReview"), input),
  sendOrchestrationMessage: (input: OrchestrationAgentSendRequestV1) =>
    invoke<MailboxMessageV1>(ch("sendOrchestrationMessage"), input),
  replyOrchestrationMessage: (input: OrchestrationAgentReplyRequest) =>
    invoke<MailboxMessageV1>(ch("replyOrchestrationMessage"), input),
  readOrchestrationTranscript: (input: OrchestrationTranscriptRequest) =>
    invoke<OrchestrationTranscriptDto>(ch("readOrchestrationTranscript"), input),
  readOrchestrationInbox: (input: OrchestrationInboxRequest) =>
    invoke<OrchestrationInboxPageV1>(ch("readOrchestrationInbox"), input),
  stopOrchestrationAgent: (input: OrchestrationAgentStopRequest) =>
    invoke<OrchestrationAgentDto>(ch("stopOrchestrationAgent"), input),
  steerOrchestrationAgent: (input: OrchestrationAgentSteerRequest) =>
    invoke<OrchestrationAgentDto>(ch("steerOrchestrationAgent"), input),
  claimOrchestrationRole: (input: OrchestrationRoleClaimRequest) =>
    invoke<OrchestrationSnapshotDto>(ch("claimOrchestrationRole"), input),
  relinquishOrchestrationRole: (input: OrchestrationRoleRelinquishRequest) =>
    invoke<OrchestrationSnapshotDto>(ch("relinquishOrchestrationRole"), input),
  escrowOrchestrationWorktree: (input: OrchestrationWorktreeEscrowRequest) =>
    invoke<OrchestrationSnapshotDto>(ch("escrowOrchestrationWorktree"), input),
  setOrchestratorPolicy: (input: OrchestratorPolicySetRequestV1) =>
    invoke<{ policy: OrchestratorPolicyV1 }>(ch("setOrchestratorPolicy"), input),
  clearOrchestratorPolicy: (input: OrchestratorPolicyClearRequestV1) =>
    invoke<{ agentId: string }>(ch("clearOrchestratorPolicy"), input),
  bindOrchestrationParent: (input: OrchestrationAgentBindParentRequestV1) =>
    invoke<{ agent: OrchestrationAgentDto }>(ch("bindOrchestrationParent"), input),
  registerOrchestrationSession: (input: OrchestrationAgentRegisterSessionRequestV1) =>
    invoke<{ agent: OrchestrationAgentDto }>(ch("registerOrchestrationSession"), input),
  submitCoordinatedWorker: (input: WorkerSubmitRequestV1) =>
    invoke<OrchestratorSubmitResultV1>(ch("submitCoordinatedWorker"), input),
  integrateCoordinatedWorker: (input: WorkerIntegrateRequestV1) =>
    invoke<{ agentId: string; taskId: string; dryRun: boolean; state: "verifying" }>(
      ch("integrateCoordinatedWorker"), input),
  abandonCoordinatedTask: (input: WorkerAbandonRequestV1) =>
    invoke<{ agentId: string; abandoned: true }>(ch("abandonCoordinatedTask"), input),
  ackCoordinatedObligation: (input: WorkerAckRequestV1) =>
    invoke<{ agentId: string; kept: string; pending: string[] }>(
      ch("ackCoordinatedObligation"), input),
  getCoordinationStatus: (input: CoordinationStatusRequestV1) =>
    invoke<CoordinationStatusV1>(ch("getCoordinationStatus"), input),
  getOrchestratorModels: () =>
    invoke<OrchestratorModelsV1>(ch("getOrchestratorModels")),
  listCollaborations: () =>
    invoke<CollaborationStatusDto[]>(ch("listCollaborations")),
  submitCollaboration: (input: CollaborationSubmitRequest) =>
    invoke<CollaborationStatusDto>(ch("submitCollaboration"), input),
  getCollaborationStatus: (collaborationId: string) =>
    invoke<CollaborationStatusDto>(ch("getCollaborationStatus"), collaborationId),
  approveCollaboration: (input: CollaborationApprovalAction) =>
    invoke<CollaborationStatusDto>(ch("approveCollaboration"), input),
  denyCollaboration: (input: CollaborationApprovalAction) =>
    invoke<CollaborationStatusDto>(ch("denyCollaboration"), input),
  acceptCollaborationPeer: (input: CollaborationPeerAction) =>
    invoke<CollaborationStatusDto>(ch("acceptCollaborationPeer"), input),
  discardCollaborationPeer: (input: CollaborationPeerAction) =>
    invoke<CollaborationStatusDto>(ch("discardCollaborationPeer"), input),
  cancelCollaboration: (collaborationId: string) =>
    invoke<CollaborationStatusDto>(ch("cancelCollaboration"), collaborationId),
  setCollaborationDraftState: (input: CollaborationDraftStateUpdate) =>
    invoke<void>(ch("setCollaborationDraftState"), input),
  listSessions: () => invoke<SessionSummary[]>(ch("listSessions")),
  listSessionSnapshot: () => invoke<SessionListingSnapshot>(ch("listSessionSnapshot")),
  listArchivedSessions: () => invoke<SessionSummary[]>(ch("listArchivedSessions")),
  listProviderCommands: (scope: ProviderCommandScope) =>
    invoke<ProviderCommandCatalogue>(ch("listProviderCommands"), scope),
  createSession: (opts: LaunchOptions & { recovery?: PromptRecoveryInput }) =>
    invoke<{ key: SessionKey }>(ch("createSession"), opts),
  resumeSession: (key: SessionKey) =>
    invoke<{ key: SessionKey }>(ch("resumeSession"), key),
  send: (key: SessionKey, text: string, recovery?: PromptRecoveryInput) => invoke<void>(ch("send"), key, text, recovery),
  recoveryControl: (request: RecoveryRequest) => invoke<RecoveryResponse>(ch("recoveryControl"), request),
  interrupt: (key: SessionKey) => invoke<void>(ch("interrupt"), key),
  closeSession: (key: SessionKey) => invoke<void>(ch("closeSession"), key),
  respond: (key: SessionKey, approvalId: string, verdict: "allow" | "deny",
    note?: string, answers?: Record<string, string>) =>
    invoke<void>(ch("respond"), key, approvalId, verdict, note, answers),
  // Model selection may lazily attach a known closed session; effort keeps the
  // live-only contract. Nothing is normalized on the way through — `undefined`
  // (engine default) and `null` (clear the effort flag) are meaningful values.
  setModel: (key: SessionKey, model?: string) =>
    invoke<SessionActivationResult>(ch("setModel"), key, model),
  setEffort: (key: SessionKey, effort: string | null) =>
    invoke<void>(ch("setEffort"), key, effort),
  setFastMode: (key: SessionKey, enabled: boolean) =>
    invoke<void>(ch("setFastMode"), key, enabled),
  archive: (key: SessionKey, archived: boolean) =>
    invoke<void>(ch("archive"), key, archived),
  // A one-shot read of what the session already said, in the same EngineEvent
  // shape onEvent delivers live — so a caller can concatenate the two without
  // translating between them. [] means "nothing to show", never a failure.
  // `opts` is optional and passed straight through: the clamp lives in the main
  // process, where a renderer cannot skip it.
  readHistory: (key: SessionKey, opts?: HistoryReadOptions) =>
    invoke<HistoryPage>(ch("readHistory"), key, opts),
  getDefaultDirectory: () => invoke<string>(ch("getDefaultDirectory")),
  createProject: (name: string) => invoke<string>(ch("createProject"), name),
  projectsFolder: (request: import("../shared/projects.js").ProjectsFolderRequest) =>
    invoke<import("../shared/projects.js").ProjectsFolderView>(ch("createProject"), request),
  pickDirectory: (preferredPath?: string) => preferredPath === undefined
    || preferredPath === ""
    ? invoke<string | null>(ch("pickDirectory"))
    : invoke<string | null>(ch("pickDirectory"), preferredPath),
  grantPromptContextPaths: (paths: readonly string[]) =>
    invoke<PromptContextGrantResult>(ch("grantPromptContextPaths"), paths),
  pathForFile: (file: { name: string }): string => {
    try {
      return webUtils.getPathForFile(file as Parameters<typeof webUtils.getPathForFile>[0]);
    } catch {
      return "";
    }
  },
  // Structured clone carries the Uint8Arrays across as bytes; nothing here
  // stringifies them, so a screenshot never becomes a base64 blob in transit.
  savePastedImages: (images: readonly Uint8Array[]) =>
    invoke<PromptImagePasteResult>(ch("savePastedImages"), images),
  readPromptContextThumbnail: (path: string) =>
    invoke<string | null>(ch("readPromptContextThumbnail"), path),
  readFile: (path: string) => invoke<string>(ch("readFile"), path),
  readFileSnapshot: (path: string) => invoke<FileSnapshot>(ch("readFileSnapshot"), path),
  previewOutput: (input: OutputPreviewRequest) => invoke<OutputPreviewResponse>(ch("previewOutput"), input),
  diagnostics: command =>
    invoke<import("../shared/diagnostics.js").DiagnosticsResponse>(ch("diagnostics"), command),
  onMenuCommand: cb => subscribe(MENU_COMMAND_CHANNEL, cb),
  onBrowserEvent: cb => subscribe(BROWSER_EVENT_CHANNEL, cb),
  saveFile: (path: string, text: string, expectedRevision: string) =>
    invoke<FileSnapshot>(ch("saveFile"), path, text, expectedRevision),
  setWorkspaceOpen: (root: string, open: boolean) =>
    invoke<void>(ch("setWorkspaceOpen"), root, open),
  listDir: (path: string) =>
    invoke<Array<{ name: string; dir: boolean }>>(ch("listDir"), path),
  listProjectFiles: (root: string) =>
    invoke<ProjectFileListResponse>(ch("listProjectFiles"), root),
  searchProject: (root: string, query: string) =>
    invoke<ProjectSearchResponse>(ch("searchProject"), root, query),
  searchSessions: (query: string) =>
    invoke<SessionSearchResponse>(ch("searchSessions"), query),
  openTerminal: (root: string) => invoke<OpenTerminalResult>(ch("openTerminal"), root),
  listTerminalLaunchProfiles: options =>
    invoke<TerminalLaunchProfile[]>(ch("listTerminalLaunchProfiles"), options),
  listTerminals: () => invoke<TerminalSnapshot>(ch("listTerminals")),
  openWorkspaceTool: (input: OpenWorkspaceToolV1) => invoke<void>(ch("openWorkspaceTool"), input),
  readTerminalDelegation: (input: ReadTerminalDelegation) => invoke<TerminalDelegationView>(ch("readTerminalDelegation"), input),
  advanceTerminalControls: (input: TerminalControlsRequestV1) => invoke<TerminalControlsStateV1>(ch("advanceTerminalControls"), input),
  createTerminal: (input: CreateTerminalRequest) =>
    invoke<TerminalSummary>(ch("createTerminal"), input),
  renameTerminal: (input: RenameTerminalRequest) =>
    invoke<TerminalSummary>(ch("renameTerminal"), input),
  setTerminalWorkspaceActivity: (input: TerminalWorkspaceActivityRequest) =>
    invoke<TerminalSummary>(ch("setTerminalWorkspaceActivity"), input),
  resizeTerminal: (input: ResizeTerminalRequest) =>
    invoke<TerminalSummary>(ch("resizeTerminal"), input),
  restartTerminal: (input: TerminalGenerationRequest) =>
    invoke<TerminalSummary>(ch("restartTerminal"), input),
  closeTerminal: (input: TerminalGenerationRequest) =>
    invoke<TerminalSummary>(ch("closeTerminal"), input),
  dismissTerminal: (id: TerminalId) =>
    invoke<DismissTerminalResult>(ch("dismissTerminal"), id),
  sendTerminalInput,
  listManagedWorktrees: () =>
    invoke<ManagedWorktreeInfo[]>(ch("listManagedWorktrees")),
  openManagedWorktree: (key: SessionKey) =>
    invoke<ManagedWorktreeInfo>(ch("openManagedWorktree"), key),
  cleanupManagedWorktree: (key: SessionKey) =>
    invoke<ManagedWorktreeCleanup>(ch("cleanupManagedWorktree"), key),
  readRepositoryReview: (root: string) =>
    invoke<RepositoryReview>(ch("readRepositoryReview"), root),
  discoverReviewChecks: (root: string) =>
    invoke<ReviewCheckDiscovery>(ch("discoverReviewChecks"), root),
  backgroundControl: (command: { kind: "read" } | { kind: "set"; enabled: boolean }) =>
    invoke<{ enabled: boolean; available: boolean }>(ch("backgroundControl"), command),
  customizationControl: (command: CustomizationCommand) =>
    invoke<ProviderCustomization>(ch("customizationControl"), command),
  listCustomization: (root: string) =>
    invoke<CustomizationInventory>(ch("listCustomization"), root),
  runReviewChecks: (root: string, selected: readonly ReviewCheckName[]) =>
    invoke<ReviewCheckRunResult[]>(ch("runReviewChecks"), root, selected),
  createReviewCheckpoint: (root: string, kind?: RequestedReviewCheckpointKind) =>
    invoke<ReviewCheckpointDetail>(ch("createReviewCheckpoint"), root, kind),
  listReviewCheckpoints: (root: string) =>
    invoke<ReviewCheckpointSummary[]>(ch("listReviewCheckpoints"), root),
  restoreReviewCheckpoint: (root: string, id: string, paths?: readonly string[]) =>
    invoke<ReviewCheckpointRestoreResult>(ch("restoreReviewCheckpoint"), root, id, paths),
  validatePromptContext: (target: PromptContextValidationTarget,
      items: readonly PromptContextDescriptor[]) =>
    invoke<PromptContextValidation>(ch("validatePromptContext"), target, items),
  probeEngines: options => invoke<EngineProbe[]>(ch("probeEngines"), options),
  voiceSetEnabled: (enabled: boolean, cwd: string) =>
    invoke<VoiceSetEnabledResponse>(ch("voiceSetEnabled"), enabled, cwd),
  voiceInterrupt: () => invoke<void>(ch("voiceInterrupt")),
  projectAway: (request: import("../shared/away.js").AwayRequest) => invoke<import("../shared/away.js").AwaySnapshot>(ch("projectAway"), request),
  voiceControl: (request: import("../shared/voice-control.js").ControlRequest) => invoke<import("../shared/voice-control.js").ControlSnapshot>(ch("voiceControl"), request),
  voiceUiResult: (callId: string, ok: boolean, output: string) =>
    invoke<void>(ch("voiceUiResult"), callId, ok, output),

  // ---- session lifecycle ---------------------------------------------------
  //
  // Every one of these can reject, and every rejection is already a sentence
  // written for a person by the time it gets here (the Rust host builds it);
  // `invoke` above only strips Electron's wrapper. A caller can therefore show
  // `err.message` without knowing which engine refused or why.
  renameSession: (key: SessionKey, title: string) =>
    invoke<void>(ch("renameSession"), key, title),
  deleteSession: (key: SessionKey) => invoke<void>(ch("deleteSession"), key),
  forgetSession: (key: SessionKey) => invoke<void>(ch("forgetSession"), key),
  // Resolves the NEW session's key, already registered and listable.
  forkSession: (key: SessionKey) =>
    invoke<{ key: SessionKey }>(ch("forkSession"), key),
  // Archives in the engine's own store and mirrors it locally; `archive` above
  // remains the local-only view flag. See the handler for how the two relate.
  setEngineArchived: (key: SessionKey, archived: boolean) =>
    invoke<void>(ch("setEngineArchived"), key, archived),
  setPermissionMode: (key: SessionKey, mode: string) =>
    invoke<void>(ch("setPermissionMode"), key, mode),
  adoptSession: (key: SessionKey, adopt: boolean) =>
    invoke<void>(ch("adoptSession"), key, adopt),
  // The long one. It resolves once, at the end, with counts — watch
  // onImportProgress for what is happening while it runs.
  adoptAll: (adopt: boolean) => invoke<ImportReport>(ch("adoptAll"), adopt),
  readUsage: () => invoke<EngineUsage[]>(ch("readUsage")),
  engineOps: () => invoke<EngineOpSupport[]>(ch("engineOps")),

  onEvent: (cb: (key: SessionKey, evt: EngineEvent) => void) => subscribe(ch("event"), cb),
  onRegistry: (cb: (sessions: SessionSummary[]) => void) => subscribe(ch("registry"), cb),
  onUpdateState: (cb: (state: UpdateState) => void) => subscribe(ch("updateState"), cb),
  onCollaborationEvent: (cb: (event: CollaborationEventDto) => void) =>
    subscribe(ch("collaborationEvent"), cb),
  onOrchestrationEvent: (cb: (event: OrchestrationEventDto) => void) =>
    subscribe(ch("orchestrationEvent"), cb),
  onTerminalLifecycle: (cb: (event: TerminalLifecycleEvent) => void) =>
    subscribe(ch("terminalLifecycle"), cb),
  onVoice: (cb: (event: VoiceEvent) => void) => subscribe(ch("voice"), cb),
  onVoiceUiCall: (cb: (call: VoiceUiCall) => void) => subscribe(ch("voiceUiCall"), cb),
  onAppearanceChanged: (cb: () => void) => subscribe(ch("appearanceChanged"), () => cb()),
  onThreads: (cb: (view: import("../shared/threads.js").ThreadsView) => void) =>
    subscribe(ch("threads"), cb),
  onTerminalData: (cb: (event: TerminalDataEvent, complete: () => void) => void) => {
    terminalDataListeners.add(cb);
    if (terminalDataBacklog.length > 0) {
      const queued = terminalDataBacklog.splice(0);
      terminalDataBacklogBytes = 0;
      for (const event of queued) cb(event, () => {});
    }
    return () => { terminalDataListeners.delete(cb); };
  },
  onTerminalDataFailure: (cb: (message: string) => void) => {
    terminalDataFailureListeners.add(cb);
    if (retainedTerminalDataFailure !== null) cb(retainedTerminalDataFailure);
    return () => { terminalDataFailureListeners.delete(cb); };
  },
  // Importing ~170 sessions is not instant, so it reports as it goes: one push
  // before any work (with the real total) and one per session settled.
  onImportProgress: (cb: (progress: ImportProgress) => void) =>
    subscribe(ch("importProgress"), cb),
  // A session's throwaway git worktree appearing, going away, or — the one that
  // matters — being KEPT because it still holds uncommitted work. That last
  // notice is the only place the user is told where that work went, so it is
  // worth surfacing rather than logging.
  onWorktree: (cb: (notice: WorktreeNotice) => void) => subscribe(ch("worktree"), cb),
};

contextBridge.exposeInMainWorld("iface", api);
contextBridge.exposeInMainWorld("ifaceBuild", { channel: buildChannel });
function applyBuildChannelToDocument(): void {
  const root = globalThis.document?.documentElement;
  if (root === undefined || root === null) return;
  root.dataset.build = buildChannel;
  if (globalThis.document !== undefined) globalThis.document.title = productName;
}
if (globalThis.document?.readyState === "loading") {
  globalThis.document.addEventListener("DOMContentLoaded", applyBuildChannelToDocument, {
    once: true,
  });
} else {
  applyBuildChannelToDocument();
}
