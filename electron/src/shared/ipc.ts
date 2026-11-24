import type { PromptRecoveryInput, RecoveryRequest, RecoveryResponse } from "./recovery.js";
import type { OutputPreviewRequest, OutputPreviewResponse } from "./output-preview.js";
import type { RemoteAdministrationRequest, RemoteAdministrationStatus } from "./remote.js";
export interface OpenWorkspaceToolV1 {
  version: 1;
  root: string;
  tool: "files" | "browser" | "ios-simulator";
  url?: string;
}

import type {
  EngineEvent, EngineId, EngineInfo, EngineUsage, LaunchOptions, SessionKey,
  ProviderCommandCatalogue, ProviderCommandScope, SessionSummary,
} from "./engine.js";
import type { AppearanceChange, AppearanceState } from "./appearance.js";
import type {
  MailboxMessageV1, OrchestratorPolicyV1,
} from "./orchestration.js";
import type {
  AdversarialReviewStartRequestV1,
  AdversarialReviewStartV1,
  OrchestrationAgentBindParentRequestV1,
  OrchestrationAgentCreateRequestV1,
  OrchestrationAgentRegisterSessionRequestV1,
  OrchestrationControlAgentV1,
  OrchestrationControlSnapshotV1,
  OrchestrationEventPageV1,
  OrchestrationAgentSendRequestV1,
  OrchestrationInboxPageV1,
  OrchestrationTranscriptPageV1,
  OrchestratorPolicyClearRequestV1,
  OrchestratorPolicySetRequestV1,
  OrchestratorModelsV1,
  OrchestratorSubmitResultV1,
  CoordinationStatusRequestV1,
  CoordinationStatusV1,
  WorkerAbandonRequestV1,
  WorkerAckRequestV1,
  WorkerIntegrateRequestV1,
  WorkerSubmitRequestV1,
} from "./orchestration-control.js";
import type { UpdateState } from "./updater.js";
import type { ReadTerminalDelegation, TerminalDelegationView } from "./delegation.js";
export type { ReadTerminalDelegation, TerminalDelegationView } from "./delegation.js";

// How many of a session's stored messages one history read may show. Applied by
// every engine, so one number describes the whole feature.
//
// It is a MESSAGE count, not a byte budget, because that is the axis both
// engines can bound: the Claude SDK takes `limit`/`offset` directly, and the
// Codex app-server hands back a whole thread at once, so its adapter stops
// after the same number of items.
//
// THE CAP KEEPS THE LAST 500 MESSAGES, NOT THE FIRST — and that is the whole
// point of it. A chat view opens at the end, and what a person needs is what
// the agent just said. Keeping the first 500 is what this constant used to
// mean, and it was wrong in a way nothing on screen could reveal: measured
// here against the real store, the largest session by message count (1353
// messages, 26.7 MB) answered a limit-500 read with messages 1..500 and
// dropped the 853 most recent, so the biggest chat in the app opened at its
// beginning and stopped a third of the way through with no hint there was
// more. 14 of the 87 sessions on this machine are over the cap.
//
// THREE CLAIMS THAT USED TO BE HERE WERE FALSE, and are corrected rather than
// quietly dropped, because the number they justified was this one. The largest
// transcript is 39.5 MB, not 11 MB. The SDK's chain walk yields 503 messages
// from it, not six. And the reason a SCOPED read of it returns nothing at all
// is NOT that its directory name contains a space — that third claim was
// written down as measured fact in three files and is simply wrong.
//
// What was actually measured: that session's recorded cwd is
// …/Desktop/Sample App, which no longer exists, because the project
// moved to …/Desktop/Projects/Sample App. Scoped to the recorded path
// the read answers 0 messages; scoped to the real one, or unscoped, it answers
// all 503. Both paths contain a space. A STALE RECORDED cwd is the fault, and
// it is what the unscoped fallbacks in the Claude adapter exist for — in
// readHistory, and in every lifecycle mutation, where the same miss throws
// instead of answering empty. Exactly one of the 87 sessions here needs them
// today; any session whose project is later moved or renamed joins it.
//
// 500 is still the cap, unchanged: the whole 18,464-message store averages 212
// messages a session, so this bounds the pathological case and nothing else.
export const HISTORY_LIMIT = 500;

/**
 * The ceiling a renderer may page UP to, one "Load older" at a time.
 *
 * Paging is expressed as a bigger `limit` rather than an offset because that is
 * what the engines actually offer: both read the whole transcript and slice the
 * newest `limit` messages off the end, so a
 * larger limit returns a strict superset of the previous page — the older half
 * plus exactly what the caller already has. An offset would have to be honoured
 * by every adapter and would leave the renderer holding two disjoint pages it
 * has no way to prove are adjacent.
 *
 * 5000 is ten pages, and comfortably above the longest transcript on this
 * machine (1353 messages). It exists so a malformed or hostile renderer cannot
 * ask the main process to serialize an unbounded array across the IPC boundary.
 */
export const HISTORY_MAX_LIMIT = 5_000;

/** What a renderer may ask of a history read beyond the session key. */
export interface HistoryReadOptions {
  /**
   * How many of the newest stored messages to return. Omitted, out of range or
   * not a number all mean HISTORY_LIMIT — the first page, which is what every
   * caller wants until the user asks for more.
   */
  limit?: number;
}

/**
 * One page of a session's stored transcript: the events to draw, and how much
 * of the transcript is NOT among them.
 *
 * The counts are the entire reason this is not a bare `EngineEvent[]`. A capped
 * read that hands back only events silently asserts that the session BEGAN
 * where the page begins. For the 14 sessions here that exceed HISTORY_LIMIT
 * that assertion is false, and the user has no way to discover it — the
 * transcript simply appears to start in the middle of a thought. A renderer
 * holding `olderMessages` can say "500 of 1353 shown, 853 older" instead.
 */
export interface HistoryPage {
  /**
   * Explicit provider capability state. Omitted is the legacy spelling of
   * "available"; "unavailable" is never equivalent to an empty transcript.
   */
  availability?: "available" | "unavailable";
  /** Stable, user-facing explanation when availability is unavailable. */
  reason?: string;
  /** The page's events, oldest first — the same shapes a live session streams. */
  events: EngineEvent[];
  /**
   * How many STORED MESSAGES this page covers. Deliberately not
   * `events.length`: one message becomes several events (prose, a tool call,
   * its diff), and several messages can become none. ABSENT means the engine
   * does not count messages, never that there are none.
   */
  messages?: number;
  /**
   * Stored messages that exist BEFORE `events[0]` and are not in this page.
   * 0 means the page starts at the beginning of the transcript; anything above
   * 0 is the truncation notice the UI owes the user.
   */
  olderMessages: number;
}

/**
 * The optional engine lifecycle operations a UI offers as actions — the
 * TypeScript mirror of `interface_contracts::ipc::ENGINE_OPS`; each Rust
 * adapter reports the subset it supports through `engineOps`.
 *
 * Every one is optional and the engines genuinely differ, so a menu built
 * from a hardcoded per-engine table would offer buttons that cannot work —
 * and would go stale the first time an engine gains an operation.
 *
 * `setArchived` is the ENGINE's archive, not the local view flag behind
 * CHANNELS.archive, which every engine has because Interface owns it.
 * `setPermissionMode` is deliberately absent: it lives on the live session
 * handle rather than on the adapter, so it can only be detected per session
 * at call time.
 */
export const ENGINE_OPS = [
  "renameSession", "deleteSession", "forkSession", "setArchived",
  "adoptSession", "readUsage",
] as const;
export type EngineOp = typeof ENGINE_OPS[number];

/** What one engine can be asked to do, for a UI that hides what it cannot. */
export interface EngineOpSupport { engine: EngineId; ops: EngineOp[] }

/** Installation health plus the adapter's live capabilities; never credentials. */
export interface EngineProbe {
  engine: EngineId;
  ok: boolean;
  version?: string;
  info?: EngineInfo;
  error?: string;
}

/** A serializable, environment-free ACP stdio provider configuration. */
export interface AcpProfile {
  /** Lowercase stable key. The corresponding engine id is `acp:${slug}`. */
  slug: string;
  displayName: string;
  /** Absolute executable path; command strings and shell interpreters are refused. */
  executablePath: string;
  /** Exact argv entries. Interface performs no splitting or interpolation. */
  args: string[];
}

export interface AcpProfilesUpdate {
  /** The canonical profiles that were durably saved. */
  profiles: AcpProfile[];
  /** True when the running adapter roster differs and an app restart is needed. */
  restartRequired: boolean;
}

/** UTF-8 file content paired with the exact bytes it was read from. */
export interface FileSnapshot { text: string; revision: string }

/** One literal project-search hit. Line and column are one-based. */
export interface ProjectSearchMatch {
  path: string;
  line: number;
  column: number;
  preview: string;
}

/** A bounded search result; truncated means at least one safety ceiling won. */
export interface ProjectSearchResponse {
  matches: ProjectSearchMatch[];
  /** Regular-file candidates considered, including skipped binary/large files. */
  filesScanned: number;
  truncated: boolean;
}

/** A safe regular file beneath a canonical, currently authorized project root. */
export interface ProjectFileEntry {
  /** Absolute path, suitable for readFile/readFileSnapshot after their own re-check. */
  path: string;
  /** Root-relative display/search path, always using forward slashes. */
  relativePath: string;
}

/** Bounded Quick Open index with an explicit statement about omitted entries. */
export interface ProjectFileListResponse {
  files: ProjectFileEntry[];
  /** Regular-file candidates considered, including excluded binary/large files. */
  filesScanned: number;
  /** Directory entries considered before filtering. */
  entriesScanned: number;
  truncated: boolean;
}

export interface ProviderListingStatus {
  engine: EngineId;
  status: "fresh" | "stale" | "unavailable";
  /** Only a complete provider snapshot makes an absent cached row removable. */
  complete: boolean;
  refreshedAt?: string;
  error?: string;
}

/**
 * Whether a listing snapshot should surface as a refresh failure.
 * `unavailable` is a first-class answer (Grok ACP has no store listing) and
 * must not be painted as "the refresh broke".
 */
export function isProviderListingFailure(provider: ProviderListingStatus): boolean {
  if (provider.status === "unavailable") return false;
  return provider.status !== "fresh" || !provider.complete;
}

export interface SessionListingSnapshot {
  sessions: SessionSummary[];
  providers: ProviderListingStatus[];
}

export interface SessionSearchMatch {
  kind: "user" | "assistant" | "tool";
  preview: string;
}

export interface SessionSearchHit {
  key: SessionKey;
  title: string;
  cwd: string;
  matches: SessionSearchMatch[];
}

export interface SessionSearchProviderNotice {
  engine: EngineId;
  complete: boolean;
  error?: string;
}

export interface SessionSearchResponse {
  hits: SessionSearchHit[];
  providers: SessionSearchProviderNotice[];
  sessionsScanned: number;
  truncated: boolean;
}

/** Durable metadata for a checkout Interface itself created. */
export interface ManagedWorktreeInfo {
  key: SessionKey;
  /** Checkout root, used for cleanup and opening the whole isolated workspace. */
  path: string;
  /** Exact nested directory the provider session runs in. */
  sessionCwd: string;
  branch: string;
  repoRoot: string;
  sourceCwd: string;
  dirtySource: boolean;
  state: "active" | "retained";
}

export interface ManagedWorktreeCleanup {
  worktree: ManagedWorktreeInfo;
  removed: boolean;
  kept?: "dirty" | "failed" | "open" | "active";
  changes?: number;
  detail?: string;
}

export interface OpenTerminalResult {
  root: string;
  app: "Ghostty" | "Terminal";
}

/** Canonical UUID string minted by the renderer for idempotent terminal creation. */
export type TerminalId = string;
export type TerminalGeneration = number;

/** Frozen launcher identities understood by the Rust terminal host. */
export const TERMINAL_LAUNCH_PROFILE_IDS = [
  "shell", "claude", "codex", "grok", "qwen", "gemini", "omp", "opencode", "muse", "kimi",
] as const;
export type TerminalLaunchProfileId = typeof TERMINAL_LAUNCH_PROFILE_IDS[number];

/** Versioned launch intent retained with a terminal across restarts. */
export interface TerminalLaunchRequestV1 {
  version: 1;
  profileId: TerminalLaunchProfileId;
}

/**
 * Which agent chat a launched profile should open.
 *
 * A sibling of the launch request rather than a field inside it, so the v1
 * launch wire is unchanged. `id` may be either a provider session identifier
 * or an opaque launcher client handle for providers such as Codex
 * whose provider-specific resume grammar is owned by Project Agent.
 */
export interface TerminalLaunchSessionV1 {
  version: 1;
  id?: string;
  resume?: boolean;
}

/** One Rust-discovered launcher choice. Availability is host-authoritative. */
export interface TerminalLaunchProfile {
  id: TerminalLaunchProfileId;
  engine?: EngineId;
  label: string;
  available: boolean;
  unavailableReason?: string;
  /** Protocol 43: how to install a missing CLI, ending "then press Refresh". */
  installHint?: string;
  /** Protocol 43: the found CLI's `--version`, first line (a `details` listing only). */
  version?: string;
  /** Protocol 48: whether Grok or Antigravity may use Interface's tools
   * through the `iface` entry in its own config (a `details` listing only). */
  paneTools?: TerminalPaneToolsState;
  /** Protocol 51: what changed in this CLI since Interface last checked it
   * against what Interface depends on (a `details` listing only). */
  changedSinceLastCheck?: TerminalToolChangeV1;
}

/** Protocol 51: one CLI's change since the provider canary last checked it.
 * `from` is the version seen before, when the version is what changed;
 * `findings` say what no longer matches, and are empty when it all does. */
export interface TerminalToolChangeV1 {
  version: 1;
  from?: string;
  to: string;
  checkedAt: string;
  findings: string[];
}

/** Protocol 48: what a CLI's own config holds for Interface's tools. `taken`
 * is an `iface` entry that is not Interface's; `unknown`, an unreadable list. */
export type TerminalPaneToolsState = "on" | "off" | "taken" | "unknown";

/** Protocol 48: turn one CLI's use of Interface's tools on or off. */
export interface TerminalPaneToolsChange {
  profileId: TerminalLaunchProfileId;
  enabled: boolean;
}

export interface TerminalSize {
  columns: number;
  rows: number;
}

export interface TerminalExit {
  code?: number;
  signal?: number;
}

/** Host protocol v8. Present only when the CLI reports model-specific support. */
export interface TerminalFastModeV1 {
  version: 1;
  model: string;
  enabled: boolean;
  ready: boolean;
}

/** Host protocol v13: bounded current xterm screen, never terminal history authority. */
export interface TerminalControlScreenV1 {
  lines: string[];
  cursorRow: number;
  cursorColumn: number;
  atBottom: boolean;
  bracketedPaste: boolean;
  ghostText: boolean;
}
export type TerminalControlSelectionV1 =
  | { kind: "model"; value: string } | { kind: "effort"; value: string } | { kind: "cancel" };
export interface TerminalControlsRequestV1 {
  version: 1;
  id: TerminalId;
  generation: TerminalGeneration;
  screen: TerminalControlScreenV1;
  selection: TerminalControlSelectionV1 | null;
}
export interface TerminalControlsStateV1 {
  version: 1;
  model: string | null;
  effort: string | null;
  ready: boolean;
  pending: boolean;
  error: string | null;
}

/** Native Codex action configured by Rust; never submits or replaces a draft. */
export const TERMINAL_FAST_MODE_KEY = "\u001b[24;2~";

/**
 * Host protocol v31. Startup files the pane's program is still waiting for a
 * File Provider (iCloud Drive) to download: at most three named, relative to
 * the pane's cwd or `~/`, and `remaining` counting all of them. Live only.
 */
export interface TerminalStartupDownloadV1 {
  version: 1;
  files: string[];
  remaining: number;
}

/**
 * Host protocol v44: the pane's program is waiting for the user, decided by
 * Rust (crates/interface-terminal/src/attention.rs) from the PTY bytes and
 * input it sees. Live only; absent while the pane is not waiting.
 */
export interface TerminalAttentionV1 {
  version: 1;
  /** When the wait began, in the frozen ISO form `updatedAt` uses. */
  since: string;
}

export interface TerminalSummary {
  id: TerminalId;
  generation: TerminalGeneration;
  title: string;
  root: string;
  cwd: string;
  sessionKey?: SessionKey;
  state: "starting" | "running" | "exited" | "failed";
  size: TerminalSize;
  createdAt: string;
  updatedAt: string;
  launch?: TerminalLaunchRequestV1;
  launchSession?: TerminalLaunchSessionV1;
  exit?: TerminalExit;
  /** The pane this one was passed off from, when a handoff created it. */
  passedOffFrom?: TerminalId;
  fastMode?: TerminalFastModeV1;
  startupDownload?: TerminalStartupDownloadV1;
  /** Host protocol v44: present while the pane needs the user. */
  attention?: TerminalAttentionV1;
  /** Host protocol v50: the agent terminal whose pane Rust handed this shell
   * when the agent exited cleanly by itself. The renderer moves the tab. */
  replaces?: TerminalId;
  /**
   * Renderer-derived live activity. Absent on the host wire; the terminal
   * center stamps it before the sidebar and tabs read the summary.
   */
  activity?: "idle" | "working" | "waiting";
}

export interface TerminalSnapshot {
  revision: number;
  terminals: TerminalSummary[];
}

/** Transient pass-off input consumed at spawn; never persisted, only its
 *  source id is echoed back as `TerminalSummary.passedOffFrom`. */
export interface TerminalHandoffV1 {
  version: number;
  sourceTerminalId: TerminalId;
  transcript: string;
  fromTitle?: string;
  /** Absent means transcript: the renderer never sends a brief. */
  kind?: "transcript" | "brief";
}

export interface CreateTerminalRequest {
  id: TerminalId;
  cwd: string;
  size?: TerminalSize;
  title?: string;
  sessionKey?: SessionKey;
  launch?: TerminalLaunchRequestV1;
  launchSession?: TerminalLaunchSessionV1;
  handoff?: TerminalHandoffV1;
  /** Protocol 41: run the profile CLI's own sign-in command instead of a chat. */
  signIn?: boolean;
  /** Protocol 60: the provider account to sign in to; only with `signIn`.
   *  Absent = the provider's default account. */
  account?: string;
}

/** One terminal as a saved workspace remembers it. */
export interface TerminalWorkspaceTerminal {
  id: TerminalId;
  title: string;
  cwd: string;
  sessionKey?: SessionKey;
  launch?: TerminalLaunchRequestV1;
  launchSession?: TerminalLaunchSessionV1;
}

/**
 * A named set of terminals that reopens onto the same chats.
 *
 * `layout` is this renderer's own serialized pane tree, stored opaquely by the
 * host. Terminal ids are kept rather than re-minted so the layout still
 * addresses the right tabs after a reopen.
 */
export interface TerminalWorkspaceRecord {
  id: string;
  name: string;
  createdAt: string;
  updatedAt: string;
  terminals: TerminalWorkspaceTerminal[];
  layout?: string;
}

/** A saved folder the owner confirmed has moved: `from` now leads to `to`. */
export interface MovedTerminalFolder {
  from: string;
  to: string;
}

export interface RenameTerminalRequest {
  id: TerminalId;
  generation: TerminalGeneration;
  title: string;
}

export interface ResizeTerminalRequest {
  id: TerminalId;
  generation: TerminalGeneration;
  size: TerminalSize;
}

export interface TerminalWorkspaceActivityRequest extends TerminalGenerationRequest {
  version: 1;
  active: boolean;
  screen?: TerminalControlScreenV1;
  outputCursor?: string;
}

export interface TerminalGenerationRequest {
  id: TerminalId;
  generation: TerminalGeneration;
}

export interface DismissTerminalResult {
  id: TerminalId;
  removed: boolean;
}

export type TerminalLifecycleEvent =
  | { kind: "upsert"; revision: number; terminal: TerminalSummary }
  | {
    kind: "removed";
    revision: number;
    id: TerminalId;
    generation: TerminalGeneration;
  };

/** Raw PTY output delivered over the dedicated binary data plane. */
export interface TerminalDataEvent {
  terminalId: TerminalId;
  generation: TerminalGeneration;
  /** Decimal u64 retained as text so JavaScript never truncates it. */
  sequence: string;
  kind: "data" | "eof";
  data: Uint8Array;
}

/** The independent git layers in which one path currently has a change. */
export type RepositoryReviewState =
  | "staged" | "unstaged" | "untracked" | "conflict";

/** Git's path-level change kind, separate from the index/worktree layer above. */
export type RepositoryReviewFileStatus =
  | "added" | "modified" | "deleted" | "renamed" | "copied"
  | "untracked" | "conflicted" | "type-changed" | "unknown";

/** Why Interface deliberately left a file's bytes out of a review response. */
export type RepositoryReviewOmission =
  | "credential" | "binary" | "symlink" | "not-regular"
  | "unsafe-path" | "output-limit" | "git-error";

/** One changed path, merged across the index and working tree. */
export interface RepositoryReviewFile {
  /** Path relative to the exact open folder being reviewed, with forward slashes. */
  path: string;
  /** The old path for a rename/copy, when git reports one. */
  previousPath?: string;
  /** A path can be both staged and unstaged; conflicts are kept distinct. */
  states: RepositoryReviewState[];
  status: RepositoryReviewFileStatus;
  /** Null means git could not safely derive text line counts. */
  additions: number | null;
  deletions: number | null;
  binary: boolean;
  /** Bounded unified git patch text. Credential and binary bytes are never included. */
  patch: string;
  patchTruncated: boolean;
  omitted?: RepositoryReviewOmission;
}

export type RepositoryReviewErrorCode =
  | "git-unavailable" | "not-repository" | "git-error" | "output-limit";

/** A non-throwing, bounded snapshot of git state beneath one open folder. */
export interface RepositoryReview {
  /** The canonical, currently authorized folder whose subtree was inspected. */
  root: string;
  repository: boolean;
  branch: string | null;
  head: string | null;
  /** False means null is unknown, not a factual detached/unborn state. */
  metadataComplete?: boolean;
  /** Null only when an output/error boundary prevents a truthful answer. */
  clean: boolean | null;
  files: RepositoryReviewFile[];
  truncated: boolean;
  error?: { code: RepositoryReviewErrorCode; message: string };
}

// ---- Review checks and recoverable checkpoints ---------------------------

/** The only package scripts Review will ever offer or execute. */
export type ReviewCheckName = "check" | "test" | "typecheck" | "lint" | "build";

/** A credential-minimal discovery result: script bodies remain in main. */
export interface ReviewCheckDiscovery {
  root: string;
  packagePresent: boolean;
  checks: ReviewCheckName[];
  error?: "invalid-package" | "package-too-large" | "unsafe-package";
}

export type ReviewCheckRunStatus =
  | "passed" | "failed" | "timed-out" | "output-limit" | "unavailable";

/** One bounded result, in the exact order the selected checks ran. */
export interface ReviewCheckRunResult {
  name: ReviewCheckName;
  status: ReviewCheckRunStatus;
  exitCode: number | null;
  stdout: string;
  stderr: string;
  durationMs: number;
  outputTruncated: boolean;
}

/**
 * "review" is a checkpoint the user asked for by hand and "recovery" is the
 * one a restore takes of what it is about to overwrite; both are kept until
 * the user removes them. "turn" is taken automatically as a prompt goes out,
 * so it is the only kind that is pruned — the newest few per repository — and
 * the only one that is not stored at all when it captured nothing.
 */
export type ReviewCheckpointKind = "review" | "recovery" | "turn";

/** The kinds a caller may ASK for. "recovery" is minted by restore alone. */
export type RequestedReviewCheckpointKind = "review" | "turn";
export type ReviewCheckpointOmissionReason = RepositoryReviewOmission
  | "deleted" | "changed" | "file-limit" | "byte-limit";

export interface ReviewCheckpointSummary {
  id: string;
  kind: ReviewCheckpointKind;
  createdAt: string;
  repositoryRoot: string;
  repositoryHead: string | null;
  sourceCheckpointId?: string;
  fileCount: number;
  omittedCount: number;
  totalBytes: number;
}

/** Created-checkpoint detail. Blob names, hashes and filesystem modes stay in main. */
export interface ReviewCheckpointDetail extends ReviewCheckpointSummary {
  files: Array<{ path: string; bytes: number }>;
  omitted: Array<{ path: string; reason: ReviewCheckpointOmissionReason }>;
}

export type ReviewCheckpointRestoreSkipReason =
  | "not-in-checkpoint" | "unsafe-path" | "credential" | "binary"
  | "symlink" | "not-regular" | "parent-missing" | "unchanged";
export type ReviewCheckpointRestoreConflictReason =
  | "checkpoint-corrupt" | "recovery-unavailable" | "changed-during-restore";

export interface ReviewCheckpointRestoreResult {
  checkpointId: string;
  recoveryCheckpointId?: string;
  restored: string[];
  skipped: Array<{ path: string; reason: ReviewCheckpointRestoreSkipReason }>;
  conflicts: Array<{ path: string; reason: ReviewCheckpointRestoreConflictReason }>;
}

// ---- queued prompt-context authorization ---------------------------------

/**
 * The provider-neutral descriptor shape accepted at the main-process trust
 * boundary. It is intentionally structural with the renderer's
 * PromptContextItem: the renderer may pass its existing objects without a
 * second mapping layer, while the bridge still validates every field at
 * runtime before a restored queue item can reach an engine.
 */
export const PROMPT_CONTEXT_DESCRIPTOR_KINDS = [
  "file", "folder", "diff", "chat", "url", "mcp", "skill",
] as const;
export type PromptContextDescriptorKind =
  typeof PROMPT_CONTEXT_DESCRIPTOR_KINDS[number];

export interface PromptContextDescriptor {
  id: string;
  kind: PromptContextDescriptorKind;
  label: string;
  value: string;
  detail?: string;
  estimatedTokens?: number;
  /**
   * Fresh, Rust-generated and bounded text: a `diff`'s Review snapshot, or
   * (protocol 63) a `chat` thread reference's block. Whatever the renderer
   * sends here is ignored and rebuilt; every other kind refuses it.
   */
  safeContent?: string;
}

/** A live session or a currently authorized exact workspace for pre-launch use. */
export type PromptContextValidationTarget = SessionKey | { root: string };

/**
 * Exact files and folders the user dropped into a composer. The drop is the
 * grant: it authorizes those paths as prompt context without opening their
 * parent folder to ordinary file IPC.
 */
export interface PromptContextGrant {
  path: string;
  kind: "file" | "folder";
}

export type PromptContextGrantReason =
  | "invalid-request"
  | "item-limit"
  | "target-missing"
  | "target-type"
  | "symbolic-link"
  | "credential-path"
  | "field-limit";

export interface PromptContextGrantRejection {
  path: string;
  reason: PromptContextGrantReason;
  message: string;
}

export interface PromptContextGrantResult {
  granted: PromptContextGrant[];
  rejected: PromptContextGrantRejection[];
}

/**
 * A pasted screenshot after main has written it to a real file. `path` is a
 * prompt-context grant exactly like a dropped one, so from here the renderer
 * cannot tell a paste from a drop — which is the point: the agent can open
 * either with its own tools.
 */
export interface PromptImageSaved {
  path: string;
  /** Bounded `data:` thumbnail. Absent when the image decoded but would not downscale. */
  previewUrl?: string;
}

export type PromptImageRejectReason =
  | "invalid-request"
  | "item-limit"
  | "size-limit"
  | "not-an-image"
  | "write-failed";

export interface PromptImageRejection {
  reason: PromptImageRejectReason;
  message: string;
}

export interface PromptImagePasteResult {
  saved: PromptImageSaved[];
  rejected: PromptImageRejection[];
}

/** Hard IPC ceilings. Invalid input is reported, never silently truncated. */
export const PROMPT_CONTEXT_VALIDATION_LIMITS = {
  maxItems: 8,
  maxIdChars: 128,
  maxLabelChars: 256,
  maxValueChars: 512,
  maxDetailChars: 512,
  maxEstimatedTokens: 10_000_000,
  maxSafeContentChars: 8_000,
} as const;

export type PromptContextInvalidReason =
  | "invalid-request"
  | "item-limit"
  | "invalid-shape"
  | "unsupported-kind"
  | "field-limit"
  | "duplicate"
  | "session-unavailable"
  | "outside-authorized-root"
  | "target-missing"
  | "target-type"
  | "symbolic-link"
  | "credential-path"
  | "diff-root-mismatch"
  | "review-unavailable"
  | "invalid-url";

export interface InvalidPromptContextDescriptor {
  /** Null means the request itself was not an array. */
  index: number | null;
  /** Bounded identifying fields, when the malformed item supplied them. */
  id?: string;
  kind?: string;
  reason: PromptContextInvalidReason;
  /** A user-facing sentence safe to show beside a retained queue item. */
  message: string;
}

/**
 * All-or-nothing by design: one invalid descriptor empties `items`, so a
 * caller cannot accidentally send a partially reauthorized persisted prompt.
 */
export type PromptContextValidation =
  | {
      ok: true;
      items: PromptContextDescriptor[];
      invalid: [];
    }
  | {
      ok: false;
      items: [];
      invalid: InvalidPromptContextDescriptor[];
    };

/**
 * Confirmation that an operation which needs a live provider handle can now
 * be followed immediately by another live-session operation. `resumed` is
 * true only when that operation transparently reattached a closed session;
 * false means the existing handle was retained.
 */
export interface SessionActivationResult {
  key: SessionKey;
  resumed: boolean;
}

/**
 * One piece of customization a session in this project will inherit: a rules
 * file, a skill, a hook, a slash command, or an MCP server.
 *
 * Names and one-line summaries only. An MCP entry carries its transport and at
 * most a scheme and host — never env, args, headers, or the rest of a URL —
 * because this inventory exists to answer "what loads here", not to hand the
 * renderer a working connection string.
 */
export type CustomizationScope = "project" | "user";

export interface CustomizationEntry {
  kind: "rules" | "skill" | "hook" | "mcp" | "command";
  /** File name, skill name, hook event, or server name. */
  name: string;
  /** One line: description, matcher, or transport. "" when unknown. */
  detail: string;
  scope: CustomizationScope;
  /** Absolute path of the defining file. */
  sourcePath: string;
  /** Parse or read failure; the entry is still listed, since it still loads. */
  error?: string;
}

export type CustomizationProvider = "claude" | "codex" | "grok" | "qwen" | "opencode";
export type CustomizationCommand =
  | { kind: "read"; root: string; provider: CustomizationProvider }
  | { kind: "source"; root: string; provider: CustomizationProvider; path: string }
  | { kind: "checkMcp"; root: string; provider: CustomizationProvider; path: string; name: string };
export interface ProviderCustomization {
  provider: CustomizationProvider;
  userRoot: string;
  inventory: CustomizationInventory;
  loaded: "unknown";
  sourcePath?: string;
  health?: { path: string; name: string; state: "ready" | "failed" | "unsupported" | "disabled"; message: string };
}

export interface CustomizationInventory {
  root: string;
  /** Capped per kind; see CUSTOMIZATION_LIMITS in src/main/inventory.ts. */
  entries: CustomizationEntry[];
  truncated: boolean;
}

// ---- durable cross-model collaboration -----------------------------------

/** Engines currently covered by the durable collaboration safety contract. */
export type CollaborationEngine = "claude" | "codex" | "acp:grok";
export type CollaborationMode = "consult" | "handoff";
export type CollaborationState =
  | "requested" | "awaiting_approval" | "approved" | "preflight"
  | "preparing_target" | "running_target" | "result_ready"
  | "awaiting_peer_review" | "delivery_pending" | "parking_source"
  | "ownership_transfer" | "activating_target" | "completed"
  | "denied" | "cancelled" | "failed" | "unknown";

/** Human-originated intent. The main process adds origin and authority facts. */
export interface CollaborationSubmitRequest {
  sourceSessionKey: SessionKey;
  mode: CollaborationMode;
  targetEngine: CollaborationEngine;
  objective: string;
  /** Caller-minted idempotency key. Retrying the same request is safe. */
  clientRequestId: string;
}

export interface CollaborationErrorDto {
  code: string;
  safeNextAction: string;
  retryable: boolean;
}

/** Sanitized durable state; provider handles, effect bodies and capabilities stay in main. */
export interface CollaborationReceiptDto {
  version: 1;
  collaborationId: string;
  revision: number;
  state: CollaborationState;
  mode: CollaborationMode;
  sourceEngine: CollaborationEngine;
  targetEngine: CollaborationEngine;
  approvalRequired: boolean;
  cancellationSafe: boolean;
  createdAt: string;
  updatedAt: string;
  error?: CollaborationErrorDto;
}

/** Exact, revision-bound material the person approves before any target starts. */
export interface CollaborationApprovalDto {
  collaborationId: string;
  approvalId: string;
  expectedRevision: number;
  expiresAt: string;
  mode: CollaborationMode;
  sourceEngine: CollaborationEngine;
  sourceSessionKey: SessionKey;
  sourceTitle?: string;
  canonicalCwd: string;
  canonicalProjectRoot: string;
  targetEngine: CollaborationEngine;
  objective: string;
  packetHash: string;
  packetPreview: string;
  packetBytes: number;
  contextBytes: number;
  omissions: number;
  managedWorktree: boolean;
}

/** Held, untrusted peer output. It cannot enter the source until explicitly accepted. */
export interface CollaborationPeerDto {
  state: "awaiting-review" | "delivery-pending";
  collaborationId: string;
  expectedRevision: number;
  resultId: string;
  resultDigest: string;
  resultBytes: number;
  sourceSessionKey: SessionKey;
  sourceEngine: CollaborationEngine;
  targetSessionKey: SessionKey;
  targetEngine: CollaborationEngine;
  objective: string;
  body: string;
}

export interface CollaborationStatusDto {
  receipt: CollaborationReceiptDto;
  sourceSessionKey: SessionKey;
  targetSessionKey?: SessionKey;
  approval?: CollaborationApprovalDto;
  peer?: CollaborationPeerDto;
}

export interface CollaborationApprovalAction {
  collaborationId: string;
  approvalId: string;
  expectedRevision: number;
}

export interface CollaborationPeerAction {
  collaborationId: string;
  resultId: string;
  resultDigest: string;
  expectedRevision: number;
}

/**
 * Fail-closed renderer draft assertion. Revisions are session-local and must
 * increase strictly; the main process invalidates the assertion before every
 * normal send. A source with no assertion, a non-empty draft, or a stale
 * revision reports queued work and cannot park or receive a peer result.
 */
export interface CollaborationDraftStateUpdate {
  sourceSessionKey: SessionKey;
  revision: number;
  empty: boolean;
}

export interface CollaborationEventDto {
  version: 1;
  kind: "changed";
  status: CollaborationStatusDto;
}

/**
 * Bounded renderer projection of durable orchestration state. Provider effect
 * reservations, the full event ledger, exact task briefs, provider session
 * identifiers, and mailbox bodies remain in the host. Bodies and transcripts
 * cross only through their explicit, agent-scoped reads.
 */
export type OrchestrationAgentDto = OrchestrationControlAgentV1;
export type OrchestrationSnapshotDto = OrchestrationControlSnapshotV1;

export interface OrchestrationTimelineRequest {
  /** Return durable host events whose sequence is greater than this cursor. */
  sequence: number;
  /** Oldest-forward page size; the host never returns more than 64 records. */
  limit?: number;
}

export type OrchestrationTimelineDto = OrchestrationEventPageV1;

/**
 * Renderer-visible orchestration intents. The desktop page never receives the
 * host transport, provider handles, writer lease, or a raw mutation endpoint;
 * each write remains a named, host-validated action.
 */
export interface OrchestrationTranscriptRequest {
  agentId: string;
  lines: number;
}

export type OrchestrationTranscriptDto = OrchestrationTranscriptPageV1;

export interface OrchestrationInboxRequest {
  agentId: string;
  threadId?: string;
  beforeMessageId?: string;
  limit?: number;
}

export interface OrchestrationAgentStopRequest {
  agentId: string;
}

export interface OrchestrationAgentSteerRequest {
  agentId: string;
  body: string;
  /** Caller-minted idempotency key; retrying the same intent cannot steer twice. */
  clientRequestId: string;
}

export interface OrchestrationRoleClaimRequest {
  agentId: string;
  role: string;
  responsibility: string;
}

export interface OrchestrationRoleRelinquishRequest {
  claimId: string;
}

export interface OrchestrationWorktreeEscrowRequest {
  leaseId: string;
  sourceAgentId: string;
  targetAgentId: string;
  sourceOwnerRevision: number;
}

/** A reply must bind the exact durable parent; an ordinary send may omit it. */
export type OrchestrationAgentReplyRequest = OrchestrationAgentSendRequestV1 & {
  parentMessageId: string;
};

/**
 * Narrow invalidation notice. The renderer always refetches the authoritative
 * snapshot, so an event cannot smuggle mailbox bodies or stale state into UI.
 */
export interface OrchestrationEventDto {
  version: 1;
  kind: "changed";
  hostId: string;
  runtimeEpoch: string;
  revision: number;
}

// ---- Voice assistant -------------------------------------------------------
//
// The one DTO family on this wire that is snake_case rather than camelCase.
// `crates/interface-contracts/src/voice.rs` carries no `rename_all`, so these
// are the field names serde emits, and they are mirrored here literally: the
// host and this file are one contract, and a rename on either side is a field
// the other silently reads as undefined.

export const VOICE_STATES = [
  "off", "starting", "listening", "thinking", "speaking", "error",
] as const;
export type VoiceState = (typeof VOICE_STATES)[number];

/** On-device speech asset install progress; `status` is installed|installing|failed. */
export interface VoiceAsset {
  status: string;
  progress: number;
}

/**
 * The whole voice indicator in one frame. Every optional field is null rather
 * than absent — Rust `Option` with no skip attribute — so a reader must treat
 * null as "nothing to show", never as a missing key.
 */
export interface VoiceEvent {
  state: VoiceState;
  session_key: string | null;
  cwd: string | null;
  partial: string | null;
  level: number | null;
  message: string | null;
  /** granted|denied|undetermined */
  microphone: string | null;
  asset: VoiceAsset | null;
}

export interface VoiceSetEnabledResponse {
  state: VoiceState;
  session_key: string | null;
  message: string | null;
}

/** One host-issued UI tool call awaiting a `voiceUiResult` with the same id. */
export interface VoiceUiCall {
  call_id: string;
  tool: string;
  input: unknown;
}

// The renderer-visible API. Preload exposes exactly this on `window.iface`;
// every method maps to the same-named channel in CHANNELS below.
export interface IfaceApi {
  remoteAccess(request: RemoteAdministrationRequest): Promise<RemoteAdministrationStatus>;
  inspectTerminalServers(request: { id: string; generation: number }): Promise<import("./harness.js").TerminalServerLink[]>;
  harnessControl(command: import("./harness.js").HarnessCommand): Promise<import("./harness.js").HarnessView>;
  /** Threads surface v1: Rust owns every thread's state; this answers the whole view. */
  threadControl(command: import("./threads.js").ThreadCommand): Promise<import("./threads.js").ThreadControlResponse>;
  /** Protocol 56: reads each thread's journal (pages, message search); Rust writes it. */
  threadJournal(command: import("./thread-journal.js").ThreadJournalCommand): Promise<import("./thread-journal.js").ThreadJournalResponse>;
  /** Protocol 60: each provider's accounts (Claude, Codex, Grok); Rust owns the store. */
  providerAccounts(command: import("./provider-accounts.js").ProviderAccountsCommand): Promise<import("./provider-accounts.js").ProviderAccountsView>;
  /** Protocol 63: a thread's stashed drafts; Rust owns `draft-stash.v1.json`. */
  draftStash(command: import("./draft-stash.js").DraftStashCommand): Promise<import("./draft-stash.js").DraftStashView>;
  /** Protocol 67: the secrets the owner gives agents (a value crosses once, in `give`, never back). */
  threadSecret(command: import("./thread-secrets.js").ThreadSecretCommand): Promise<import("./thread-secrets.js").ThreadSecretsView>;
  agentControl(command: import("./agents.js").AgentCommand): Promise<import("./agents.js").AgentControlResponse>;
  /** Main only, never the host: what is still working, for the quit prompt. */
  reportWorkInFlight(work: import("./quit-guard.js").WorkInFlightV1): void;
  /** Main only, never the host: terminal panes waiting for input, for the Dock badge. */
  reportDockAttention(report: import("./dock-attention.js").DockAttentionV1): void;
  /** Main only, never the host (T3 section 8): register a wrapped visual page
   * and get the sandboxed frame URL main serves it at, or release one. */
  visualFrame(request: import("./visual-frame.js").VisualFrameRequest): Promise<string | null>;
  /** Persistent app appearance; System follows the operating-system setting. */
  getAppearance(): Promise<AppearanceState>;
  setAppearance(preference: AppearanceChange): Promise<AppearanceState>;
  /** Persisted configurable ACP providers; never includes credentials or env. */
  getAcpProfiles(): Promise<AcpProfile[]>;
  /** Save the next startup roster. Running adapters are never hot-swapped. */
  setAcpProfiles(profiles: AcpProfile[]): Promise<AcpProfilesUpdate>;
  /** Saved terminal workspaces, newest arrangement last written wins. */
  getTerminalWorkspaces(): Promise<TerminalWorkspaceRecord[]>;
  /**
   * Replace the whole roster. The host validates before it touches disk.
   * `movedFolders` carries folders the owner confirmed have moved: the host
   * re-proves each one and moves matching restart proofs in the same save.
   */
  setTerminalWorkspaces(
    workspaces: TerminalWorkspaceRecord[],
    movedFolders?: readonly MovedTerminalFolder[]): Promise<TerminalWorkspaceRecord[]>;
  /**
   * Where a saved pane's folder now leads, when it moved and left a link
   * behind; `null` when the saved path is still the real directory.
   */
  resolveMovedTerminalFolder(path: string): Promise<string | null>;
  /** Signed packaged-app update state. Development builds report disabled. */
  updateSessions(command: import("./update-sessions.js").UpdateSessionsCommand): Promise<import("./update-sessions.js").UpdateSessionsView>;
  getUpdateState(): Promise<UpdateState>;
  /** Deduplicated manual check; available updates download in the background. */
  checkForUpdates(): Promise<UpdateState>;
  /** Restart only after the signed update has completely downloaded. */
  restartToUpdate(): Promise<boolean>;
  /** Authoritative owner-private orchestration state; no provider capabilities leak through. */
  getOrchestrationSnapshot(): Promise<OrchestrationSnapshotDto>;
  /** Bounded read-only durable host timeline after an explicit sequence cursor. */
  readOrchestrationTimeline(input: OrchestrationTimelineRequest):
    Promise<OrchestrationTimelineDto>;
  createOrchestrationAgent(input: OrchestrationAgentCreateRequestV1):
    Promise<OrchestrationAgentDto>;
  /** Launch a cross-model, Rust-coordinated builder/reviewer workflow. */
  startAdversarialReview(input: AdversarialReviewStartRequestV1):
    Promise<AdversarialReviewStartV1>;
  sendOrchestrationMessage(input: OrchestrationAgentSendRequestV1):
    Promise<MailboxMessageV1>;
  replyOrchestrationMessage(input: OrchestrationAgentReplyRequest):
    Promise<MailboxMessageV1>;
  readOrchestrationTranscript(input: OrchestrationTranscriptRequest):
    Promise<OrchestrationTranscriptDto>;
  readOrchestrationInbox(input: OrchestrationInboxRequest): Promise<OrchestrationInboxPageV1>;
  stopOrchestrationAgent(input: OrchestrationAgentStopRequest): Promise<OrchestrationAgentDto>;
  steerOrchestrationAgent(input: OrchestrationAgentSteerRequest): Promise<OrchestrationAgentDto>;
  claimOrchestrationRole(input: OrchestrationRoleClaimRequest):
    Promise<OrchestrationSnapshotDto>;
  relinquishOrchestrationRole(input: OrchestrationRoleRelinquishRequest):
    Promise<OrchestrationSnapshotDto>;
  escrowOrchestrationWorktree(input: OrchestrationWorktreeEscrowRequest):
    Promise<OrchestrationSnapshotDto>;
  /** Human designation of an orchestrator and its bounds. The record IS the authorization. */
  setOrchestratorPolicy(input: OrchestratorPolicySetRequestV1):
    Promise<{ policy: OrchestratorPolicyV1 }>;
  /** Removes orchestrator status and its policy. Refused while the agent has workers. */
  clearOrchestratorPolicy(input: OrchestratorPolicyClearRequestV1): Promise<{ agentId: string }>;
  /** Draw or clear one lineage edge. Depth is capped at 1: workers never delegate onward. */
  bindOrchestrationParent(input: OrchestrationAgentBindParentRequestV1):
    Promise<{ agent: OrchestrationAgentDto }>;
  /** Adopt an existing provider session as a root agent node. Idempotent on sessionKey. */
  registerOrchestrationSession(input: OrchestrationAgentRegisterSessionRequestV1):
    Promise<{ agent: OrchestrationAgentDto }>;
  /** Hand one coordinated worker's finished unit to wcp's serialized integrator. */
  submitCoordinatedWorker(input: WorkerSubmitRequestV1): Promise<OrchestratorSubmitResultV1>;
  /**
   * Claim the verify of a submitted bundle — dry-run, or publishing into the
   * canonical working tree. The verify is asynchronous: this answers as soon as
   * the task is claimed, and the outcome arrives on the canvas as a worker notice.
   */
  integrateCoordinatedWorker(input: WorkerIntegrateRequestV1):
    Promise<{ agentId: string; taskId: string; dryRun: boolean; state: "verifying" }>;
  /** Human act: retire a coordinated worker's task and free its write scope. */
  abandonCoordinatedTask(input: WorkerAbandonRequestV1):
    Promise<{ agentId: string; abandoned: true }>;
  /** Human act: acknowledge one pending rebase obligation path. */
  ackCoordinatedObligation(input: WorkerAckRequestV1):
    Promise<{ agentId: string; kept: string; pending: string[] }>;
  /** Whether this project is wcp-registered, and what is waiting on the human. */
  getCoordinationStatus(input: CoordinationStatusRequestV1): Promise<CoordinationStatusV1>;
  /** Live per-engine model and effort catalogue, exactly as iface_models reports it. */
  getOrchestratorModels(): Promise<OrchestratorModelsV1>;
  /** Active durable collaborations, including recovery-held approvals/results. */
  listCollaborations(): Promise<CollaborationStatusDto[]>;
  submitCollaboration(input: CollaborationSubmitRequest): Promise<CollaborationStatusDto>;
  getCollaborationStatus(collaborationId: string): Promise<CollaborationStatusDto>;
  approveCollaboration(input: CollaborationApprovalAction): Promise<CollaborationStatusDto>;
  denyCollaboration(input: CollaborationApprovalAction): Promise<CollaborationStatusDto>;
  acceptCollaborationPeer(input: CollaborationPeerAction): Promise<CollaborationStatusDto>;
  discardCollaborationPeer(input: CollaborationPeerAction): Promise<CollaborationStatusDto>;
  cancelCollaboration(collaborationId: string): Promise<CollaborationStatusDto>;
  setCollaborationDraftState(input: CollaborationDraftStateUpdate): Promise<void>;
  listSessions(): Promise<SessionSummary[]>;
  /** Sessions plus per-provider freshness/completeness diagnostics. */
  listSessionSnapshot(): Promise<SessionListingSnapshot>;
  /** Locally hidden sessions for a restore browser. */
  listArchivedSessions(): Promise<SessionSummary[]>;
  /**
   * Control-plane command discovery for an exact not-yet-created launch scope.
   * An unavailable result is explicit and never creates a provider session.
   */
  listProviderCommands(scope: ProviderCommandScope): Promise<ProviderCommandCatalogue>;
  createSession(opts: LaunchOptions & { recovery?: PromptRecoveryInput }): Promise<{ key: SessionKey }>;
  resumeSession(key: SessionKey): Promise<{ key: SessionKey }>;
  send(key: SessionKey, text: string, recovery?: PromptRecoveryInput): Promise<void>;
  recoveryControl(request: RecoveryRequest): Promise<RecoveryResponse>;
  interrupt(key: SessionKey): Promise<void>;
  /** Close/detach the live provider handle; unlike interrupt, it ends liveness. */
  closeSession(key: SessionKey): Promise<void>;
  /**
   * Drop a session from Interface's registry only. Vendor stores are untouched.
   * Refuses while the session is live — close it first.
   */
  forgetSession(key: SessionKey): Promise<void>;
  respond(key: SessionKey, approvalId: string, verdict: "allow" | "deny",
    note?: string, answers?: Record<string, string>): Promise<void>;
  // A model selection transparently resumes a known closed session first. It
  // resolves only after the live handle accepts the setting; a rejection means
  // the renderer must keep the last confirmed value. `model: undefined`
  // restores the engine default.
  setModel(key: SessionKey, model?: string): Promise<SessionActivationResult>;
  // Effort retains the narrower live-handle-only contract. `null` clears it.
  setEffort(key: SessionKey, effort: string | null): Promise<void>;
  // Fast mode on engines that advertise it (ModelOption.supportsFastMode).
  setFastMode(key: SessionKey, enabled: boolean): Promise<void>;
  /**
   * How much the agent may do without asking, changed mid-session: "default",
   * "plan", "acceptEdits", and whatever else the engine's own
   * EngineInfo.capabilities.permissionModes advertises.
   *
   * The single most useful control when several agents are running — it is the
   * difference between one you supervise and three you let run. A successful
   * adapter publishes access.changed with the effective mode; a rejection is
   * the renderer's cue to retain the last confirmed value.
   */
  setPermissionMode(key: SessionKey, mode: string): Promise<void>;
  /** Interface's OWN view flag. Local, reversible, invisible to the engine. */
  archive(key: SessionKey, archived: boolean): Promise<void>;
  // ---- Session lifecycle in the ENGINE's store (Phase 4) -------------------
  // These write to the engine's own store, so they outlive this app and are
  // visible from the vendor CLI. Every one of them rejects for an engine whose
  // adapter does not implement it — call engineActions() first and do not offer
  // what is not listed.
  /** Retitle the session where the engine keeps it, not just in this sidebar. */
  renameSession(key: SessionKey, title: string): Promise<void>;
  /**
   * Permanently delete the session from the engine's store. IRREVERSIBLE, and
   * nothing downstream asks a second time — the confirmation is the caller's
   * job, and it must be a real one.
   */
  deleteSession(key: SessionKey): Promise<void>;
  /** Copy the session so two approaches can diverge. Resolves the NEW key. */
  forkSession(key: SessionKey): Promise<{ key: SessionKey }>;
  /**
   * Claim an existing session as one of Interface's (`adopt: true`) or release
   * it again (`false`). A metadata stamp in the engine's store, never a copy:
   * the transcript is untouched either way. This is the "import my history"
   * primitive.
   */
  adoptSession(key: SessionKey, adopt: boolean): Promise<void>;
  /** Archive in the ENGINE's store. Distinct from `archive`, the local flag. */
  setEngineArchived(key: SessionKey, archived: boolean): Promise<void>;
  /**
   * Plan usage for every engine that will report it, in ONE call — the whole
   * reason the shape is normalized. Nothing else the user runs can show both
   * subscriptions side by side.
   *
   * An engine that cannot report answers `available: false` with a note rather
   * than being missing from the array, so "not reported" is drawable and never
   * has to render as 0%.
   */
  readUsage(): Promise<EngineUsage[]>;
  /**
   * Which lifecycle actions each engine can actually perform, feature-detected
   * on the live adapters. The UI hides what is not listed; it must not decide
   * from the engine id, because that is a hardcoded capability table by
   * another name and it goes stale the first time an engine gains a method.
   */
  engineOps(): Promise<EngineOpSupport[]>;
  // The session's transcript so far, as the same events a live session streams,
  // oldest first, ending with the NEWEST message. Resolves to an empty page
  // when there is nothing to show — a session the engine has no transcript for,
  // or an engine that cannot read its own store — and rejects only when the
  // read itself failed, with a message written for a person (see bridge.ts).
  //
  // `opts.limit` is how "Load older" is spelled: ask again with a bigger limit
  // and the answer is the same page with the older half in front of it, so the
  // renderer can prepend rather than hold two pages it cannot join. Clamped by
  // `history_read_limit` in crates/interface-contracts/src/ipc.rs, so the
  // default and the ceiling are the Rust host's.
  readHistory(key: SessionKey, opts?: HistoryReadOptions): Promise<HistoryPage>;
  /** First-run workspace: ~/Desktop/Projects when present, otherwise home. */
  getDefaultDirectory(): Promise<string>;
  /**
   * Native folder picker. Optional `preferredPath` is where the dialog starts;
   * omitted or empty falls back to getDefaultDirectory().
   */
  pickDirectory(preferredPath?: string): Promise<string | null>;
  /** Creates a new folder in the Projects folder and grants its workspace root. */
  createProject(name: string): Promise<string>;
  /**
   * The Projects folder New project creates in (protocol 40, on the
   * createProject channel). `choose` opens the native picker; Rust stores
   * the folder picked there.
   */
  projectsFolder(request: import("./projects.js").ProjectsFolderRequest):
    Promise<import("./projects.js").ProjectsFolderView>;
  /**
   * Authorize exact dropped files and folders as prompt context. The drop is
   * the grant, the same way a native folder pick is. Ordinary read/write IPC
   * still requires a session or picked workspace root.
   */
  grantPromptContextPaths(paths: readonly string[]): Promise<PromptContextGrantResult>;
  /**
   * Local path of a dropped File. Implemented in preload via Electron
   * webUtils, not as an IPC channel. Empty when the File has no disk path.
   */
  pathForFile(file: { name: string }): string;
  /**
   * Write pasted clipboard images to an app-owned file and grant them as
   * prompt context. Bytes are admitted only when they sniff as an image AND
   * decode; the filename is minted here, never taken from the clipboard.
   */
  savePastedImages(images: readonly Uint8Array[]): Promise<PromptImagePasteResult>;
  /**
   * A bounded `data:` thumbnail for one already-granted image path. Scoped to
   * prompt-context grants and open roots — deliberately NOT a general
   * read-any-file-as-base64 primitive. Null whenever it cannot be drawn.
   */
  readPromptContextThumbnail(path: string): Promise<string | null>;
  readFile(path: string): Promise<string>;
  readFileSnapshot(path: string): Promise<FileSnapshot>;
  previewOutput(input: OutputPreviewRequest): Promise<OutputPreviewResponse>;
  /** Compare-and-save: rejects without writing when expectedRevision is stale. */
  saveFile(path: string, text: string, expectedRevision: string): Promise<FileSnapshot>;
  /** Prevent disposable-worktree cleanup while Monaco owns an editable tab. */
  setWorkspaceOpen(root: string, open: boolean): Promise<void>;
  listDir(path: string): Promise<Array<{ name: string; dir: boolean }>>;
  /** Metadata-only, bounded file index for Quick Open. Never reads file contents. */
  listProjectFiles(root: string): Promise<ProjectFileListResponse>;
  /** Literal, case-insensitive text search inside an already-open workspace. */
  searchProject(root: string, query: string): Promise<ProjectSearchResponse>;
  /** Bounded literal search of known provider transcripts. */
  searchSessions(query: string): Promise<SessionSearchResponse>;
  /** Open an exact authorized workspace/session root in a terminal app. */
  openTerminal(root: string): Promise<OpenTerminalResult>;
  openWorkspaceTool(input: OpenWorkspaceToolV1): Promise<void>;
  /** Process-lifetime integrated terminal registry, separate from provider sessions. */
  /** `{refresh: true}` (protocol 41) finds the provider CLIs again first:
   * the launcher's Try again. */
  /** `{paneTools}` (protocol 48) switches one CLI's use of Interface's tools
   * first and answers the details listing. */
  listTerminalLaunchProfiles(options?: {
    refresh?: boolean; details?: boolean; paneTools?: TerminalPaneToolsChange;
  }): Promise<TerminalLaunchProfile[]>;
  listTerminals(): Promise<TerminalSnapshot>;
  readTerminalDelegation(input: ReadTerminalDelegation): Promise<TerminalDelegationView>;
  advanceTerminalControls(input: TerminalControlsRequestV1): Promise<TerminalControlsStateV1>;
  createTerminal(input: CreateTerminalRequest): Promise<TerminalSummary>;
  renameTerminal(input: RenameTerminalRequest): Promise<TerminalSummary>;
  resizeTerminal(input: ResizeTerminalRequest): Promise<TerminalSummary>;
  setTerminalWorkspaceActivity(input: TerminalWorkspaceActivityRequest): Promise<TerminalSummary>;
  restartTerminal(input: TerminalGenerationRequest): Promise<TerminalSummary>;
  closeTerminal(input: TerminalGenerationRequest): Promise<TerminalSummary>;
  dismissTerminal(id: TerminalId): Promise<DismissTerminalResult>;
  /** Exact, non-replay-safe PTY input. Throws before enqueue when unavailable. */
  sendTerminalInput(id: TerminalId, generation: TerminalGeneration, data: Uint8Array): void;
  /** Recoverable checkout inventory; cleanup never forces or deletes branches. */
  listManagedWorktrees(): Promise<ManagedWorktreeInfo[]>;
  openManagedWorktree(key: SessionKey): Promise<ManagedWorktreeInfo>;
  cleanupManagedWorktree(key: SessionKey): Promise<ManagedWorktreeCleanup>;
  /** Read-only git status and bounded unified patches for one open workspace. */
  readRepositoryReview(root: string): Promise<RepositoryReview>;
  /** Discover the fixed safe package-script allowlist for one exact open root. */
  discoverReviewChecks(root: string): Promise<ReviewCheckDiscovery>;
  /** Read-only, bounded list of the rules, skills, hooks, commands and MCP
   *  servers a session in this root inherits. Never returns credential bytes. */
  backgroundControl(command: { kind: "read" } | { kind: "set"; enabled: boolean }): Promise<{ enabled: boolean; available: boolean }>;
  customizationControl(command: CustomizationCommand): Promise<ProviderCustomization>;
  listCustomization(root: string): Promise<CustomizationInventory>;
  /** Run only selected names returned by discoverReviewChecks, sequentially. */
  runReviewChecks(root: string,
    selected: readonly ReviewCheckName[]): Promise<ReviewCheckRunResult[]>;
  /**
   * Capture a fresh, main-generated Review snapshot as a private checkpoint.
   * The kind decides its retention — see ReviewCheckpointKind. A "turn"
   * capture that found nothing to save is reported with `fileCount: 0` and is
   * never written to the store.
   */
  createReviewCheckpoint(root: string,
    kind?: RequestedReviewCheckpointKind): Promise<ReviewCheckpointDetail>;
  /** List checkpoints belonging to one exact currently authorized root. */
  listReviewCheckpoints(root: string): Promise<ReviewCheckpointSummary[]>;
  /** Restore selected checkpoint files, or all files when paths is omitted. */
  restoreReviewCheckpoint(root: string, id: string,
    paths?: readonly string[]): Promise<ReviewCheckpointRestoreResult>;
  /** Reauthorize typed context immediately before a restored prompt is sent. */
  validatePromptContext(target: PromptContextValidationTarget,
    items: readonly PromptContextDescriptor[]): Promise<PromptContextValidation>;
  /** `{refresh: true}` (protocol 41) finds the provider CLIs again first:
   * Models & providers → Refresh. */
  probeEngines(options?: { refresh?: boolean }): Promise<EngineProbe[]>;
  /**
   * Protocol 39: the Copy Diagnostics report, this run's set-aside store
   * notices, and Finder reveals of the log, the data folder or a set-aside
   * file. Rust decides and filters everything; see shared/diagnostics.ts.
   */
  diagnostics(command: import("./diagnostics.js").DiagnosticsCommand): Promise<import("./diagnostics.js").DiagnosticsResponse>;
  /** The application menu's requests (main → renderer); see shared/app-menu.ts. */
  onMenuCommand(cb: (command: import("./app-menu.js").MenuCommandV1) => void): () => void;
  /** What main tells the renderer about Browser pages (main → renderer); see shared/browser-events.ts. */
  onBrowserEvent(cb: (event: import("./browser-events.js").BrowserEventV1) => void): () => void;
  /**
   * Turn the voice assistant on or off for one project root. The host owns the
   * helper process and the voice session; this resolves with the state it
   * reached, and `session_key` is the session to select when it is running.
   */
  voiceSetEnabled(enabled: boolean, cwd: string): Promise<VoiceSetEnabledResponse>;
  projectAway(request: import("./away.js").AwayRequest): Promise<import("./away.js").AwayResponse>;
  voiceControl(request: import("./voice-control.js").ControlRequest): Promise<import("./voice-control.js").ControlSnapshot>;
  /** Barge-in: stop speaking and interrupt the voice session's current turn. */
  voiceInterrupt(): Promise<void>;
  /** Answer one `onVoiceUiCall` by its exact call_id. Never resolves the host twice. */
  voiceUiResult(callId: string, ok: boolean, output: string): Promise<void>;
  onEvent(cb: (key: SessionKey, evt: EngineEvent) => void): () => void;
  onRegistry(cb: (sessions: SessionSummary[]) => void): () => void;
  onUpdateState(cb: (state: UpdateState) => void): () => void;
  onCollaborationEvent(cb: (event: CollaborationEventDto) => void): () => void;
  onOrchestrationEvent(cb: (event: OrchestrationEventDto) => void): () => void;
  onTerminalLifecycle(cb: (event: TerminalLifecycleEvent) => void): () => void;
  /** Binary transport v2: complete after consumption or bounded replay/discard. */
  onTerminalData(cb: (event: TerminalDataEvent, complete: () => void) => void): () => void;
  onTerminalDataFailure(cb: (message: string) => void): () => void;
  onVoice(cb: (event: VoiceEvent) => void): () => void;
  onVoiceUiCall(cb: (call: VoiceUiCall) => void): () => void;
  /** A theme file was added, removed or saved: read the appearance again. */
  onAppearanceChanged(cb: () => void): () => void;
  /** The whole Threads view, after every committed change. */
  onThreads(cb: (view: import("./threads.js").ThreadsView) => void): () => void;
}

// One constant per IfaceApi method, plus the two main→renderer push channels
// (`event`, `registry`) behind onEvent/onRegistry.
//
// THE ONLY map of these channel strings. A duplicate, `OPS_CHANNELS`, briefly
// lived in src/main/bridge.ts because this file sat outside that workstream's
// edit scope; the merge instruction that used to stand here has been carried
// out and `grep -rn OPS_CHANNELS src` now matches nothing but this sentence.
// Anything the main process needs on top of these — channels whose payload
// types are main-process types — belongs in bridge.ts and not here.
export const CHANNELS = {
  getAppearance: "iface:getAppearance",
  setAppearance: "iface:setAppearance",
  getAcpProfiles: "iface:getAcpProfiles",
  setAcpProfiles: "iface:setAcpProfiles",
  agentControl: "iface:agentControl",
  harnessControl: "iface:harnessControl",
  threadControl: "iface:threadControl",
  threadJournal: "iface:threadJournal",
  providerAccounts: "iface:providerAccounts",
  draftStash: "iface:draftStash",
  threadSecret: "iface:threadSecret",
  inspectTerminalServers: "iface:inspectTerminalServers",
  getTerminalWorkspaces: "iface:getTerminalWorkspaces",
  setTerminalWorkspaces: "iface:setTerminalWorkspaces",
  resolveMovedTerminalFolder: "iface:resolveMovedTerminalFolder",
  updateSessions: "iface:updateSessions",
  getUpdateState: "iface:getUpdateState",
  checkForUpdates: "iface:checkForUpdates",
  restartToUpdate: "iface:restartToUpdate",
  getOrchestrationSnapshot: "iface:getOrchestrationSnapshot",
  readOrchestrationTimeline: "iface:readOrchestrationTimeline",
  createOrchestrationAgent: "iface:createOrchestrationAgent",
  startAdversarialReview: "iface:startAdversarialReview",
  sendOrchestrationMessage: "iface:sendOrchestrationMessage",
  replyOrchestrationMessage: "iface:replyOrchestrationMessage",
  readOrchestrationTranscript: "iface:readOrchestrationTranscript",
  readOrchestrationInbox: "iface:readOrchestrationInbox",
  stopOrchestrationAgent: "iface:stopOrchestrationAgent",
  steerOrchestrationAgent: "iface:steerOrchestrationAgent",
  claimOrchestrationRole: "iface:claimOrchestrationRole",
  relinquishOrchestrationRole: "iface:relinquishOrchestrationRole",
  escrowOrchestrationWorktree: "iface:escrowOrchestrationWorktree",
  setOrchestratorPolicy: "iface:setOrchestratorPolicy",
  clearOrchestratorPolicy: "iface:clearOrchestratorPolicy",
  bindOrchestrationParent: "iface:bindOrchestrationParent",
  registerOrchestrationSession: "iface:registerOrchestrationSession",
  submitCoordinatedWorker: "iface:submitCoordinatedWorker",
  integrateCoordinatedWorker: "iface:integrateCoordinatedWorker",
  abandonCoordinatedTask: "iface:abandonCoordinatedTask",
  ackCoordinatedObligation: "iface:ackCoordinatedObligation",
  getCoordinationStatus: "iface:getCoordinationStatus",
  getOrchestratorModels: "iface:getOrchestratorModels",
  listCollaborations: "iface:listCollaborations",
  submitCollaboration: "iface:submitCollaboration",
  getCollaborationStatus: "iface:getCollaborationStatus",
  approveCollaboration: "iface:approveCollaboration",
  denyCollaboration: "iface:denyCollaboration",
  acceptCollaborationPeer: "iface:acceptCollaborationPeer",
  discardCollaborationPeer: "iface:discardCollaborationPeer",
  cancelCollaboration: "iface:cancelCollaboration",
  setCollaborationDraftState: "iface:setCollaborationDraftState",
  listSessions: "iface:listSessions",
  listSessionSnapshot: "iface:listSessionSnapshot",
  listArchivedSessions: "iface:listArchivedSessions",
  listProviderCommands: "iface:listProviderCommands",
  createSession: "iface:createSession",
  resumeSession: "iface:resumeSession",
  send: "iface:send",
  recoveryControl: "iface:recoveryControl",
  interrupt: "iface:interrupt",
  closeSession: "iface:closeSession",
  forgetSession: "iface:forgetSession",
  respond: "iface:respond",
  setModel: "iface:setModel",
  setEffort: "iface:setEffort",
  setFastMode: "iface:setFastMode",
  setPermissionMode: "iface:setPermissionMode",
  archive: "iface:archive",
  renameSession: "iface:renameSession",
  deleteSession: "iface:deleteSession",
  forkSession: "iface:forkSession",
  adoptSession: "iface:adoptSession",
  setEngineArchived: "iface:setEngineArchived",
  readUsage: "iface:readUsage",
  engineOps: "iface:engineOps",
  readHistory: "iface:readHistory",
  getDefaultDirectory: "iface:getDefaultDirectory",
  pickDirectory: "iface:pickDirectory",
  createProject: "iface:createProject",
  grantPromptContextPaths: "iface:grantPromptContextPaths",
  savePastedImages: "iface:savePastedImages",
  readPromptContextThumbnail: "iface:readPromptContextThumbnail",
  readFile: "iface:readFile",
  readFileSnapshot: "iface:readFileSnapshot",
  previewOutput: "iface:previewOutput",
  saveFile: "iface:saveFile",
  setWorkspaceOpen: "iface:setWorkspaceOpen",
  listDir: "iface:listDir",
  listProjectFiles: "iface:listProjectFiles",
  searchProject: "iface:searchProject",
  searchSessions: "iface:searchSessions",
  openTerminal: "iface:openTerminal",
  openWorkspaceTool: "iface:openWorkspaceTool",
  listTerminalLaunchProfiles: "iface:listTerminalLaunchProfiles",
  listTerminals: "iface:listTerminals",
  readTerminalDelegation: "iface:readTerminalDelegation",
  advanceTerminalControls: "iface:advanceTerminalControls",
  createTerminal: "iface:createTerminal",
  renameTerminal: "iface:renameTerminal",
  resizeTerminal: "iface:resizeTerminal",
  setTerminalWorkspaceActivity: "iface:setTerminalWorkspaceActivity",
  restartTerminal: "iface:restartTerminal",
  closeTerminal: "iface:closeTerminal",
  dismissTerminal: "iface:dismissTerminal",
  listManagedWorktrees: "iface:listManagedWorktrees",
  openManagedWorktree: "iface:openManagedWorktree",
  cleanupManagedWorktree: "iface:cleanupManagedWorktree",
  readRepositoryReview: "iface:readRepositoryReview",
  discoverReviewChecks: "iface:discoverReviewChecks",
  listCustomization: "iface:listCustomization",
  customizationControl: "iface:customizationControl",
  backgroundControl: "iface:backgroundControl",
  runReviewChecks: "iface:runReviewChecks",
  createReviewCheckpoint: "iface:createReviewCheckpoint",
  listReviewCheckpoints: "iface:listReviewCheckpoints",
  restoreReviewCheckpoint: "iface:restoreReviewCheckpoint",
  validatePromptContext: "iface:validatePromptContext",
  probeEngines: "iface:probeEngines",
  diagnostics: "iface:diagnostics",
  remoteAccess: "iface:remoteAccess",
  voiceSetEnabled: "iface:voiceSetEnabled",
  voiceControl: "iface:voiceControl",
  projectAway: "iface:projectAway",
  voiceInterrupt: "iface:voiceInterrupt",
  voiceUiResult: "iface:voiceUiResult",
  event: "iface:event",
  registry: "iface:registry",
  updateState: "iface:updateState",
  collaborationEvent: "iface:collaborationEvent",
  orchestrationEvent: "iface:orchestrationEvent",
  terminalLifecycle: "iface:terminalLifecycle",
  voice: "iface:voice",
  voiceUiCall: "iface:voiceUiCall",
  appearanceChanged: "iface:appearanceChanged",
  threads: "iface:threads",
} as const satisfies Record<string, string>;

// Every channel is its key under the `iface:` prefix. The sandboxed preload
// cannot import this table at runtime and builds each string from its key
// instead, so this compile-time check is what keeps the two identical.
CHANNELS satisfies { readonly [K in keyof typeof CHANNELS]: `iface:${K}` };

// Inverse of sessionKey. Splits on the FIRST "#" only: session ids may contain
// "#", engine ids may not.
export function parseKey(key: SessionKey): { engine: EngineId; sessionId: string } {
  const at = key.indexOf("#");
  if (at < 0) throw new Error(`Malformed session key: ${key}`);
  return { engine: key.slice(0, at) as EngineId, sessionId: key.slice(at + 1) };
}

declare global {
  interface Window {
    iface: IfaceApi;
    ifaceBuild?: { readonly channel: "dev" | "release" };
  }
}
