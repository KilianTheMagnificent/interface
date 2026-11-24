import type { PromptImageReference, PromptTurnImage } from "./prompt-images.js";

export type EngineId = "claude" | "codex" | `acp:${string}`;
export type SessionKey = string;                       // `${engine}#${sessionId}`
export type SessionStatus = "working" | "waiting" | "idle" | "failed";
export type SandboxMode = "read-only" | "workspace-write" | "danger-full-access";

/**
 * The permission mode an agent starts in when nobody picked one. A policy that
 * stops to ask strands an unattended worker on its first tool call and reads to
 * the human as a hung agent, so the default is never a blocking one: it is each
 * engine's own word for "run without asking me". Claude and Grok call it
 * `auto` — their own classifier decides each request. Codex calls it `never`,
 * which suppresses prompts while its independent sandbox still fences the
 * writes, so it is the equivalent policy rather than a wider one.
 *
 * An engine that advertises no such mode is deliberately absent and keeps
 * whatever it defaults to; handing it another provider's token would be a
 * launch the adapter rejects.
 */
const AUTO_PERMISSION_MODES: Readonly<Record<string, string>> = Object.freeze({
  claude: "auto",
  codex: "never",
  "acp:grok": "auto",
});

export function autoPermissionMode(engineId: string): string | undefined {
  return AUTO_PERMISSION_MODES[engineId];
}

/**
 * Launch fields an adapter can require for an analysis-only peer consultation.
 * Deliberately excludes `allowedTools`: that field widens what runs without
 * approval and therefore cannot be part of a safety profile.
 */
export type ConsultationLaunchOptions = Pick<LaunchOptions,
  "permissionMode" | "sandboxMode" | "settingSources" | "disallowedTools">;

/**
 * An adapter-owned claim about the strongest consultation boundary it can
 * actually enforce. `read-only` is a provider execution boundary;
 * `approval-gated` means the caller must deny every approval and disclose that
 * it is not an OS sandbox.
 *
 * Absence is treated exactly like `available: false`; the optional capability
 * keeps older/custom adapters fail-closed. A caller must apply every returned
 * launch option unchanged. Providers must not label an approval-only boundary
 * as read-only.
 */
export type ConsultationSafetyEnforcement = "read-only" | "approval-gated";
export type ConsultationSafetyProfile =
  | { available: true; enforcement: ConsultationSafetyEnforcement;
      launchOptions: ConsultationLaunchOptions }
  | { available: false; reason: string };

export interface EngineCapabilities {
  streaming: boolean; approvals: boolean; resume: boolean; fork: boolean;
  cloudSessions: boolean; models: string[] | "dynamic";
  effortLevels: string[]; permissionModes: string[];
  consultationSafety?: ConsultationSafetyProfile;
  // Text can be delivered INTO a turn that is already running — Claude's
  // streaming-input push, Codex's `turn/steer`. ABSENT means it cannot, which
  // is the honest default: an ACP provider has no mid-turn injection at all,
  // and offering the verb there would be a button that quietly queues instead.
  // Only an adapter that can prove the path sets it.
  steer?: boolean;
}
/** Whether a provider's CLI says it is signed in (protocol 41), by its own
 * status command or app-server. Absent when it cannot say. */
export type EngineAuth = "signedIn" | "signedOut";
/** Why a turn failed, when the provider says so in a way the app can act on (protocol 41). */
export type EngineErrorCause = "signedOut";
export interface EngineInfo {
  binaryPath: string; version: string; loginIdentity: string | null;
  auth?: EngineAuth;
  capabilities: EngineCapabilities;
  /** Human label for configurable ACP profiles. Engine ids remain the stable key. */
  displayName?: string;
  /** Provider-reported launch catalogue. Absent when the provider cannot report one. */
  modelCatalog?: ModelOption[];
}
export interface SessionSummary {
  engine: EngineId; sessionId: string; title: string; cwd: string;
  status: SessionStatus; cloud: boolean; model?: string;
  /** Last accepted effort for this chat. Absent means unknown; null means default. */
  effort?: string | null;
  lastActivity: string;                                // ISO 8601
  firstSeen: string;                                   // ISO 8601, ordering anchor
  archived: boolean;
  // Transcript size on disk, in bytes, when the engine's listing reports one
  // (the Agent SDK's SDKSessionInfo.fileSize). ABSENT means the engine did not
  // say — never treat that as small, or an engine that omits it (Codex, today)
  // would have every session hidden.
  //
  // A SECONDARY signal only, and the earlier comment here was wrong to claim it
  // separates real work from throwaway `/doctor` chats: measured across 87 real
  // sessions, /doctor is 287-334 KB (it dumps a diagnostic report), /model is
  // 1.6 MB and /effort 1.9 MB — all above the median. Size cannot find those.
  // `titleFromSummary` below is what does. Size still earns its place for the
  // one case titles miss: a short real-looking prompt that went nowhere.
  size?: number;
  // True when `title` came from the engine's OWN summary (or the user's custom
  // title) rather than from a firstPrompt fallback. This is what makes "the
  // title is a bare slash command" safe to act on: for all 17 such sessions
  // here the SDK's generated summary IS the command, i.e. the engine itself
  // considers the session to be nothing but that invocation. A REAL session
  // that merely opened with `/model` earns a descriptive summary instead, so it
  // never matches. ABSENT means unknown — treat it as "not from summary", which
  // is the conservative direction (shows the session).
  titleFromSummary?: boolean;
  // Where this session came from. "app" = started inside Interface; "store" =
  // found in the engine's own session store, i.e. started from the CLI or by
  // another tool. The sidebar defaults to showing only "app", matching how the
  // Claude Code desktop app lists the chats you started in it rather than every
  // terminal session on the machine.
  //
  // Durability matters here, so this is NOT merely a local flag: the Claude
  // engine stamps its own sessions with the SDK's `tagSession()`, and the tag
  // comes back on every later listing. That survives a lost registry cache and
  // a reinstall, which a renderer- or registry-only marker would not. ABSENT
  // means unknown; treat unknown as "store", because guessing "app" would put
  // every CLI session back in a sidebar the user asked to keep clean.
  origin?: "app" | "store";
  // The project this session belongs to: the nearest ancestor of `cwd` (itself
  // included) that contains a `.git`. ABSENT means the session ran somewhere
  // that is not inside any repository — a scratch directory, a home folder, or
  // the parent folder that merely CONTAINS projects — and the sidebar files
  // those under "Other".
  //
  // Deliberately the git root and not `cwd`: an agent run from
  // `<repo>/src/renderer` is working on that repo, and grouping on raw cwd
  // scattered one project across several sidebar groups. Computed in the main
  // process, where the filesystem is, and never in the renderer.
  projectRoot?: string;
  // The durable home of a session launched in a throwaway worktree. While the
  // agent is live `cwd` is the worktree; once that checkout is removed `cwd`
  // moves back here. Providers persist the original worktree path in their own
  // transcript, so this marker is what prevents a later cold listing from
  // resurrecting a directory Interface has already removed.
  worktreeHome?: string;
  /** The disposable cwd paired with worktreeHome; used to recognize stale listings. */
  worktreePath?: string;
  // Restrictive launch policy that must survive a resume. These are copied
  // into the registry because provider listings do not report them. Omitting a
  // field means "use the provider default"; [] remains meaningful for
  // settingSources (Claude SDK isolation mode).
  settingSources?: string[];
  disallowedTools?: string[];
  /** Which surface this session belongs to. Absent means "code". App-owned:
   *  no vendor listing reports it, so the registry keeps it across merges. */
  kind?: WireSessionKind;
  /** Protocol 68: the permission mode the host runs and resumes this session
   *  in (`default` when it keeps none for it). Sent on registry pushes and
   *  listings for sessions with permission controls outside the agent
   *  roster. Absent means the host did not say (an older host, an agent's
   *  session, Chat or Voice). */
  permissionMode?: string;
}

/** The SDK's fast-mode lifecycle: off, rate-limit cooldown, or on. */
export type FastModeState = "off" | "cooldown" | "on";

/**
 * Which surface a session belongs to. "chat" is a plain conversation with no
 * tools and no folder; "code" is the IDE — every session that existed before
 * this field, and every session a vendor lists that Interface did not start;
 * "agent" is a Cowork-style task run in a folder. ABSENT means "code".
 */
export type SessionKind = "chat" | "code" | "agent";
export const SESSION_KINDS: readonly SessionKind[] = ["chat", "code", "agent"];
/** Kinds the host may put on the wire. "voice" is the voice assistant's own
 *  session: host-started, never offered in the picker, shown on the Chat
 *  surface because it is a spoken conversation. */
export type WireSessionKind = SessionKind | "voice";
export const WIRE_SESSION_KINDS: readonly WireSessionKind[] = [...SESSION_KINDS, "voice"];
export function isSessionKind(v: unknown): v is SessionKind {
  return typeof v === "string" && (SESSION_KINDS as readonly string[]).includes(v);
}
/** The surface a session lives on. Voice sessions live on Chat. */
export function sessionKindOf(s: { kind?: WireSessionKind }): SessionKind {
  return s.kind === "voice" ? "chat" : (s.kind ?? "code");
}

export interface LaunchOptions {
  cwd: string; prompt: string; model?: string; effort?: string;
  permissionMode?: string; cloud?: boolean; name?: string;
  /** Provider sandbox override. Adapters that cannot enforce it must reject. */
  sandboxMode?: SandboxMode;
  /** Start with the provider's fast mode on (engines that support it). */
  fastMode?: boolean;
  // ---- Per-session scoping (Phase 4) --------------------------------------
  // Which on-disk settings the session loads. The Agent SDK's own words for
  // `settingSources`: 'user' = ~/.claude/settings.json, 'project' =
  // .claude/settings.json, 'local' = .claude/settings.local.json; omitted loads
  // ALL of them (the CLI default), and `[]` is "SDK isolation mode".
  //
  // This is the fix for a real, measured problem: an Interface-launched session
  // inherits the user's entire global ~/.claude — every hook, skill and MCP
  // server — so a three-word prompt produced a 58 KB transcript. Running many
  // agents at once makes that cost multiply. Omitted keeps today's behaviour.
  // Note 'project' must be present for CLAUDE.md files to load at all.
  settingSources?: string[];
  // READ THIS BEFORE USING allowedTools. It does NOT restrict anything. The
  // Agent SDK's own words: these are the tools "auto-allowed without prompting
  // for permission", and "to restrict which tools are available, use the
  // `tools` option instead". So setting it WIDENS what runs unattended — it is
  // per-tool bypassPermissions wearing an innocent name.
  //
  // This comment previously said "Restrict the session's tools", and a launch
  // control was built on that reading: a field labelled "allow only" that
  // silently auto-approved whatever the user typed into it. The control has
  // been removed rather than relabelled. Nothing in the renderer sets this
  // today, and nothing should until the contract carries the SDK's `tools`
  // option, which genuinely restricts.
  allowedTools?: string[];
  /** Genuinely restrictive: these are removed from the model's context, so the
   *  agent cannot use them and cannot ask you to approve them. */
  disallowedTools?: string[];
  // Run this session in a throwaway git worktree of `cwd` rather than in the
  // working tree itself. Two agents pointed at one repo otherwise overwrite
  // each other's edits, which is the failure mode that appears the moment this
  // app succeeds at its actual job. Undefined = run in `cwd`, as today.
  worktree?: boolean;
  /** Root/source = 0 (default); peer target = 1 and cannot collaborate again. */
  collaborationDepth?: 0 | 1;
  /** Which surface is launching. Absent means "code". */
  kind?: SessionKind;
  /** Protocol 60: the provider account the session runs on (`"default"` = the
   *  inherited one). Absent = that provider's default account. */
  account?: string;
}

// ---- Usage and plan limits (Phase 4) --------------------------------------
// One normalized shape both engines answer in, so the app can show ONE view of
// how much of each subscription is gone. Nothing else the user runs can show
// them side by side, which is the point.
//
// Everything is optional because the two engines report very different things
// and BOTH may refuse: `available: false` is a first-class answer (an API-key
// session has no plan limits at all), and it must render as "not reported"
// rather than as zero — a usage bar reading 0% when the truth is unknown is
// worse than no bar.
export interface UsageWindow {
  /** Human label for the window: "5-hour", "7-day", "weekly". */
  label: string;
  /** Percent of the window consumed, 0-100. Null when the engine did not say. */
  utilization: number | null;
  /** ISO 8601 instant the window resets, when known. */
  resetsAt: string | null;
}
export interface EngineUsage {
  engine: EngineId;
  /** Whether the engine could report at all. False => windows is empty. */
  available: boolean;
  /** The plan the account is on: "max", "pro", "plus", … Engine's own wording. */
  plan?: string;
  windows: UsageWindow[];
  /** Session-scoped spend, when the engine reports it. */
  costUsd?: number;
  /** Why it is unavailable, or any caveat worth showing. Human-readable. */
  note?: string;
  /** Protocol 60: the added account this row is for; absent on the Default
   *  account's row. Rows for one engine are adjacent. */
  account?: string;
  /** Protocol 60: that account's label, beside `account`. */
  accountLabel?: string;
}

// One row of the engine's OWN model list — never a hardcoded table here. Each
// adapter fills these from its native control plane; `effortLevels` is
// per-model because effort is not a universal axis (a model that reports
// supportsEffort false has no effort UI at all).
export interface ModelOption {
  value: string; displayName: string; description?: string;
  supportsEffort: boolean; effortLevels: string[];
  /** Provider-designated default row, for exact capability checks without an override. */
  isDefault?: boolean;
  /** Provider says this model can run in fast mode (Claude: Opus family). */
  supportsFastMode?: boolean;
}

/**
 * One slash command the selected provider says this exact live session can run.
 * Names never include the leading slash. The list is dynamic because project
 * skills and plugins can appear or disappear while a session is open.
 */
export interface ProviderCommand {
  name: string;
  description: string;
  /** Provider-owned argument copy such as "<branch>" or "[instructions]". */
  argumentHint?: string;
  /** Alternate names accepted by the provider, also without a leading slash. */
  aliases?: string[];
}

/**
 * The exact launch context whose provider command catalogue is being requested.
 *
 * `settingSources` is part of the identity, not decoration: Claude can expose a
 * different set of plugin/skill commands for the same cwd and model when user,
 * project, or local settings are disabled. Omitted means the provider's normal
 * launch default; `[]` is the SDK's explicit isolation mode and must never be
 * collapsed into omission.
 */
export interface ProviderCommandScope {
  engine: EngineId;
  cwd: string;
  model?: string;
  settingSources?: string[];
}

/** A provider-neutral pre-launch command discovery result. */
export interface ProviderCommandCatalogue {
  /** Canonical, validated scope that produced this answer. */
  scope: ProviderCommandScope;
  available: boolean;
  /** A complete replacement snapshot. Empty is meaningful only when available. */
  commands: ProviderCommand[];
  /** Stable user-facing explanation when discovery is unavailable. */
  reason?: string;
}

/**
 * When the bridge observed an event, in ISO-8601. The bridge already computes
 * this per event for `registry.applyEvent` (it is what keeps `lastActivity`
 * honest) and used to drop it on the floor before broadcasting, so a consumer
 * could not tell a replayed history row from one that arrived a second ago.
 *
 * Optional because ADAPTERS never set it: an adapter reports what the engine
 * said, and the observation time is our own bookkeeping. Only the bridge's pump
 * stamps it, so an event read straight off a `SessionHandle` legitimately has
 * none, and every consumer written before the stamp existed keeps compiling.
 */
export interface EngineEventStamp {
  at?: string;
}

// `agentKey` (Phase 1.6) attributes an event to the subagent that produced it:
// it is the tool_use id of the Agent/Task call that spawned that subagent, which
// is exactly what the SDK puts in `parent_tool_use_id` on every message from
// inside it. ABSENT means the main thread — the field is optional so every
// consumer written before subagents existed keeps working unchanged, seeing the
// same flat stream it always saw.
export type EngineEvent = EngineEventStamp & (
  | { kind: "session.meta"; sessionId: string; model: string; cwd: string; title?: string }
  | { kind: "message.delta"; text: string; agentKey?: string }
  // What the USER said. Live sessions never emit this — the app already knows
  // what it just sent — but a replayed transcript must, or the history reads as
  // one side of a phone call. Measured on the real store: 14 of 87 sessions
  // consist ENTIRELY of user turns, so without this kind they map to zero
  // events and still show "No messages yet" after history was supposedly added.
  // `text` is the prompt as sent, already flattened out of the engine's block
  // format by the adapter; renderers must not assume it is short.
  //
  // `images` are the attachments whose BYTES the engine's own store kept, so a
  // pasted screenshot survives a reload instead of living only on the echo this
  // window drew when the message was sent. Absent on every turn that has none —
  // an event without pictures is byte-for-byte the event it always was.
  // Each `src` has already passed isPromptImagePreviewUrl at the mapper, so a
  // renderer may put it straight in an `<img>`; the count is already capped
  // (PROMPT_IMAGE_LIMITS.maxTurnImages). What this does NOT carry is an
  // attachment the engine stored as a PATH — that stays part of `text`, because
  // reading it would be a disk access behind a grant the read path does not
  // have. `imageReferences` identifies those adapter-inserted placeholder lines
  // so compact clients do not have to guess from user-authored Markdown. `text`
  // can be empty when a turn was nothing but a picture.
  | { kind: "user.message"; text: string; agentKey?: string;
      images?: readonly PromptTurnImage[];
      imageReferences?: readonly PromptImageReference[] }
  // A local orchestration result delivered from another provider. Adapters do
  // not emit it: the collaboration controller records it after delivery so a
  // later handoff preserves provenance instead of relabelling peer output as
  // trusted user instruction. The caller bounds every text field.
  | { kind: "peer.result"; operationId: string; sourceEngine: EngineId;
      sourceSessionId: string; objective: string; text: string; untrusted: true }
  // A page the agent showed through team tools' `show_visual` (protocol 61).
  // The event only names it; its bytes come from `threadJournal` `visual`
  // and are drawn ONLY inside the sandboxed visual frame (visual-frame.ts).
  | { kind: "visual.shown"; id: string; title: string; bytes: number }
  | { kind: "thinking.signal"; active: boolean; agentKey?: string }
  | { kind: "tool.call"; callId: string; tool: string; summary: string; input: unknown;
      agentKey?: string }
  | { kind: "tool.result"; callId: string; ok: boolean; summary: string; agentKey?: string }
  | { kind: "approval.requested"; approvalId: string; tool: string; summary: string; input: unknown }
  // `callId` is the tool.call this diff belongs to, and it is REQUIRED because
  // the renderer draws the call and its diffs as one row: without it the pairing
  // falls back to "whatever came next", and two edits announced before either
  // resolves (ACP does exactly that) put one call's successful change under the
  // other call's "failed". Adjacency is not identity. A new emitter that forgets
  // to stamp it is a compile error rather than a row that lies.
  | { kind: "diff.produced"; callId: string; path: string; before: string; after: string;
      agentKey?: string }
  | { kind: "status.changed"; status: SessionStatus }
  | { kind: "usage.updated"; inputTokens: number; outputTokens: number;
      // Protocol 66: `resetsAt`, when the provider said when the limit
      // resets (UTC ISO, the usage windows' form).
      rateLimited?: { message: string; resetsAt?: string } }
  // Exact provider request throughput. End-to-end includes prompt prefill,
  // queueing, and generation; adapters must not estimate token counts.
  | { kind: "throughput.updated"; outputTokens: number; durationMs: number;
      basis: "end-to-end" }
  // Session-window occupancy. Distinct from usage.updated, which is token
  // totals and the rate-limit badge. Unknown occupancy is omitted entirely —
  // a host that cannot prove used/size must not invent a 0% bar.
  | { kind: "context.usage"; usage: ContextUsage }
  // A provider compact boundary. Renderers may draw a seam; they must not
  // treat this as the session going idle or failed.
  | { kind: "context.compact"; trigger: ContextCompactTrigger;
      preTokens?: number; postTokens?: number; ok?: boolean }
  // A retryable provider delay. Not an error: the turn is still in flight.
  | { kind: "turn.retry"; attempt: number; maxAttempts: number;
      delayMs?: number; message: string }
  | { kind: "notice"; message: string; tone: "info" | "warning" }
  | { kind: "error"; message: string; retryable: boolean; cause?: EngineErrorCause }
  // The engine's real model list for this session, pushed once the query is up.
  // Advisory only: it changes nothing about the session, so the status rules and
  // the conversation list both ignore it.
  | { kind: "models.available"; models: ModelOption[] }
  // A complete replacement snapshot, never a delta. Claude pushes this after
  // skills change and ACP calls the equivalent update
  // `available_commands_update`; renderers keep only the newest list.
  | { kind: "commands.available"; commands: ProviderCommand[] }
  // The prior snapshot no longer belongs to the effective model/configuration.
  // This means "unknown while refreshing", not an authoritative empty result.
  // Renderers must stop offering the prior rows and fail closed on slash input
  // until commands.available replaces it; they may draw that transient state
  // separately from a genuine empty snapshot.
  | { kind: "commands.invalidated" }
  // A model/effort change the engine has accepted. Carries the effective values
  // of BOTH, not just the one that moved, so a renderer can render the pair
  // from this one event. `model: undefined` means the engine default; `effort:
  // null` means no effort flag is set.
  | { kind: "config.changed"; model?: string; effort?: string | null }
  // The provider's effective approval/permission policy for this live handle.
  // This is deliberately separate from config.changed: access is a safety
  // fact, and a model refresh must never erase it. Adapters publish it when a
  // run opens (especially after resume) and after an accepted live change so a
  // renderer never has to infer policy from a remembered launch preference.
  | { kind: "access.changed"; mode: string }
  // The provider's own fast-mode truth, distinct from config.changed so a
  // state ping can never clobber the model/effort a config event asserts.
  // "cooldown" is the SDK's rate-limit pause; `reason` carries its
  // disabled-reason vocabulary when the provider names one.
  | { kind: "fastmode.changed"; state: FastModeState; reason?: string }
  // A complete provider plan snapshot. ACP v1 requires all entries on every
  // update, so renderers replace (not append to) the current plan. These are
  // exactly ACP PlanEntry's lossless stable fields; provider metadata is not
  // copied because ACP explicitly reserves it as opaque.
  | { kind: "plan.updated"; entries: PlanEntry[] }
  // Subagent lifecycle. `agentKey` is the spawning tool_use id, so it is the
  // same key every event from inside that subagent carries — that pairing is
  // what lets a renderer nest a transcript under the call that opened it.
  // `model` is present only when the spawn call named one explicitly, or the
  // engine's own agent catalogue names one for this type; absent means
  // "inherits the parent's model", never a guess.
  | { kind: "subagent.started"; agentKey: string; subagentType: string;
      description: string; model?: string }
  | { kind: "subagent.finished"; agentKey: string; ok: boolean; summary: string }
  // The handle's stream ended and the bridge let go of it. Emitted by the
  // bridge, never by an engine adapter: it describes our own bookkeeping, not
  // anything the engine did. It is the renderer's only reliable "this key is no
  // longer live" signal — a non-retryable error usually precedes it but neither
  // implies the other (a clean turn ends the stream with no error at all).
  | { kind: "session.closed" }
);

export type PlanEntryPriority = "high" | "medium" | "low";
export type PlanEntryStatus = "pending" | "in_progress" | "completed";
export interface PlanEntry {
  content: string;
  priority: PlanEntryPriority;
  status: PlanEntryStatus;
}

export type ContextCompactTrigger = "manual" | "auto" | "unknown";

/** One named slice of the current context window. Name is the provider's. */
export interface ContextCategory {
  name: string;
  tokens: number;
}

/**
 * How full this session's context window is. Distinct from EngineUsage, which
 * is account-plan quota. `percent: null` means the provider said it is busy
 * (compacting) or could not compute a trustworthy fill.
 */
export interface ContextUsage {
  used?: number;
  size?: number;
  percent: number | null;
  categories?: readonly ContextCategory[];
  autoCompact?: boolean;
  autoCompactThreshold?: number;
  compacting?: boolean;
}

function completeContextTokens(usage: ContextUsage):
    { used: number; size: number } | undefined {
  if (usage.used === undefined || usage.size === undefined
      || !Number.isFinite(usage.used) || !Number.isFinite(usage.size)
      || usage.used < 0 || usage.size <= 0) return undefined;
  return { used: usage.used, size: usage.size };
}

export function contextWindowRemaining(usage: ContextUsage): number | undefined {
  const complete = completeContextTokens(usage);
  return complete === undefined ? undefined
    : Math.max(0, complete.size - complete.used);
}

export function contextAutoCompactRemaining(usage: ContextUsage): number | undefined {
  const complete = completeContextTokens(usage);
  const threshold = usage.autoCompactThreshold;
  if (complete === undefined || usage.autoCompact !== true
      || threshold === undefined || !Number.isFinite(threshold) || threshold < 0) {
    return undefined;
  }
  return Math.max(0, threshold - complete.used);
}

export function contextUsageChange(previous: ContextUsage | undefined,
    current: ContextUsage): number | undefined {
  if (previous === undefined) return undefined;
  const before = completeContextTokens(previous);
  const after = completeContextTokens(current);
  return before === undefined || after === undefined ? undefined
    : after.used - before.used;
}

export function formatContextTokens(n: number): string {
  if (!Number.isFinite(n) || n < 0) return "";
  if (n < 1000) return String(Math.round(n));
  if (n < 10_000) return `${(n / 1000).toFixed(1).replace(/\.0$/, "")}k`;
  return `${Math.round(n / 1000)}k`;
}

export function sessionKey(engine: EngineId, sessionId: string): SessionKey {
  return `${engine}#${sessionId}`;
}
