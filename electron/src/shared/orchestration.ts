/**
 * Provider-neutral orchestration records mirrored from the Rust contract
 * (`interface_contracts::orchestration`) for the renderer's projected DTOs.
 * This module intentionally contains no Electron or Node imports so it
 * compiles into both bundles.
 */
export const ORCHESTRATION_LIMITS = Object.freeze({
  agents: 2_000,
  messages: 20_000,
  receiptsPerMessage: 32,
  roleClaims: 5_000,
  effects: 40_000,
  worktreeLeases: 4_000,
  orchestratorPolicies: 256,
  events: 50_000,
  capabilitiesPerAgent: 16,
  // Live provider sessions this host will drive at once, ACROSS every
  // orchestrator, and it counts the orchestrator itself. At 4 a lead could run
  // three workers and no more — a four-way review, the ordinary case this
  // exists for, could not be spawned four-wide at all. 12 leaves a lead the
  // eight workers `ORCHESTRATOR_POLICY_DEFAULTS.maxWorkers` already promises,
  // plus room for a second team or a session the human started. It is still a
  // real ceiling: each target is a CLI process with its own context.
  maxRunningTargets: 12,
  identifiersBytes: 1_024,
  shortTextBytes: 4 * 1_024,
  bodyBytes: 64 * 1_024,
  pathBytes: 16 * 1_024,
} as const);

export const ORCHESTRATION_CAPABILITY_FACETS = Object.freeze([
  "reference", "transcript", "message", "wake", "spawn", "worktree",
] as const);
export type OrchestrationCapabilityFacet =
  (typeof ORCHESTRATION_CAPABILITY_FACETS)[number];
export type OrchestrationCapabilityState = "available" | "unavailable";

export interface AgentCapabilityV1 {
  facet: OrchestrationCapabilityFacet;
  state: OrchestrationCapabilityState;
  reason?: string;
}

export type AgentNodeState =
  | "planned"
  | "starting"
  | "ready"
  | "working"
  | "waiting"
  | "idle"
  | "completed"
  | "failed"
  | "cancelled"
  | "unknown";

export interface AgentScopeV1 {
  canonicalCwd: string;
  canonicalProjectRoot?: string;
  accessMode: string;
  scopeDigest: string;
  worktreeLeaseId?: string;
}

export interface AgentNodeV1 {
  version: 1;
  agentId: string;
  name?: string;
  engineId: string;
  sessionKey?: string;
  providerSessionId?: string;
  runtimeEpoch: string;
  parentAgentId?: string;
  role?: string;
  depth: number;
  state: AgentNodeState;
  capabilities: AgentCapabilityV1[];
  scope: AgentScopeV1;
  createdAt: string;
  updatedAt: string;
}

export type OrchestratorAutonomy = "active" | "paused";
/**
 * Ordered widest-last. `coordinated` is `isolated` plus a coordinator task that
 * fences the worker's writes to a declared scope, so it is narrower than a
 * private checkout with no scope at all.
 */
export const ORCHESTRATOR_ACCESS_CEILING_ORDER = Object.freeze([
  "read-only", "coordinated", "isolated", "project",
] as const);
export type OrchestratorAccessCeiling =
  (typeof ORCHESTRATOR_ACCESS_CEILING_ORDER)[number];
export const ORCHESTRATOR_ROLE = "orchestrator" as const;
export const WORKER_ROLE = "worker" as const;

/**
 * Durable human authorization for one orchestrator. The record is the standing
 * approval for every provider effect the orchestrator causes inside its bounds;
 * removing it removes the authority.
 */
export interface OrchestratorPolicyV1 {
  version: 1;
  agentId: string;
  autonomy: OrchestratorAutonomy;
  maxAccessMode: OrchestratorAccessCeiling;
  maxWorkers: number;
  maxConcurrent: number;
  maxDelegations: number;
  wallClockMs: number;
  activatedAt: string;
  updatedAt: string;
  delegationsUsed: number;
  spawnsUsed: number;
  // Rust leaves integration, coordinationRequired, allowUnverifiedFence and
  // modelCatalogue off the wire when unset (`skip_serializing_if`).
  /** Who may publish verified work into the canonical working tree. */
  integration?: "orchestrator" | "human";
  /** When true, only coordinator-fenced workers may write. */
  coordinationRequired?: boolean;
  /** Allow a coordinated worker whose engine fence is unproven. */
  allowUnverifiedFence?: boolean;
  /**
   * The human said the workers do the work. A `read-only` spawn is then refused
   * rather than allowed through: an orchestrator asked to build something has
   * been observed spawning four read-only "reviewers" and writing every file
   * itself, which is a different shape of work than the one that was asked for
   * and quietly wastes the team. Absent means the orchestrator decides.
   */
  workersWrite?: boolean;
  modelCatalogue?: "on-demand" | "on-activate";
  /** `<engineId>:<modelKey>` or `<engineId>:*`; absent means unrestricted. */
  modelAllowlist?: string[];
  /** Human-authored routing guidance, returned verbatim to the orchestrator. */
  modelNotes?: string;
}

export const ORCHESTRATOR_POLICY_DEFAULTS = Object.freeze({
  maxAccessMode: "project",
  maxWorkers: 8,
  maxConcurrent: 4,
  maxDelegations: 64,
  wallClockMs: 4 * 60 * 60 * 1000,
  integration: "orchestrator",
  coordinationRequired: false,
  allowUnverifiedFence: false,
  modelCatalogue: "on-demand",
} as const);

export type MessageOrigin = "human" | "agent" | "system";
export type MessageDeliveryState =
  | "requested"
  | "queued"
  | "provider_accepted"
  | "settled"
  | "processed"
  | "failed"
  | "unknown";

export interface MessageDeliveryReceiptV1 {
  version: 1;
  receiptId: string;
  state: MessageDeliveryState;
  at: string;
  providerReceiptId?: string;
  errorCode?: string;
  detail?: string;
}

export interface MailboxMessageV1 {
  version: 1;
  messageId: string;
  threadId: string;
  parentMessageId?: string;
  origin: MessageOrigin;
  senderAgentId?: string;
  recipientAgentId: string;
  bodyDigest: string;
  bodyBytes: number;
  /** Owner-private body. It may be pruned after processing. */
  body?: string;
  expectReply: boolean;
  effectId?: string;
  /**
   * The worker's own bounded summary, carried beside a composed system notice
   * so a roster can report what the worker said rather than the notice text.
   */
  noticeSummary?: string;
  receipts: MessageDeliveryReceiptV1[];
  createdAt: string;
  updatedAt: string;
}

export type RoleClaimState = "active" | "relinquished";

/** Role claims communicate responsibility and awareness; they are not locks. */
export interface RoleClaimV1 {
  version: 1;
  claimId: string;
  agentId: string;
  role: string;
  responsibility: string;
  scopeDigest: string;
  state: RoleClaimState;
  claimedAt: string;
  updatedAt: string;
  relinquishedAt?: string;
}

export type WorktreeLeaseState = "active" | "escrow" | "released" | "failed" | "unknown";
export type WorktreeEscrowState = "prepared" | "completed" | "failed" | "unknown";

export interface WorktreeEscrowV1 {
  escrowId: string;
  sourceAgentId: string;
  targetAgentId: string;
  sourceOwnerRevision: number;
  state: WorktreeEscrowState;
  preparedAt: string;
  settledAt?: string;
}

export const WORKTREE_COORDINATOR_FENCES = Object.freeze([
  "hook", "sandbox", "hook+sandbox", "unverified",
] as const);
export type WorktreeCoordinatorFence = (typeof WORKTREE_COORDINATOR_FENCES)[number];

/**
 * One external coordinator task fencing a worker's writes. `fence` states what
 * is actually proven for the engine that was launched, never what is hoped:
 * `unverified` means the worker itself is unfenced and only the coordinator's
 * own submit gate protects the canonical checkout.
 */
export interface WorktreeCoordinatorV1 {
  kind: "wcp";
  taskId: string;
  owner: string;
  epoch: number;
  leaseExpiresAt: string;
  writeScope: string[];
  resources: string[];
  taskState?: string;
  submissionId?: string;
  /** Paths `wcp task rebase` reported as still owed after a rebase; cleared by ack. */
  pendingObligations?: string[];
  /** Paths the last publish wrote into the canonical working tree. */
  publishedPaths?: string[];
  fence: WorktreeCoordinatorFence;
}

export interface WorktreeLeaseV1 {
  version: 1;
  leaseId: string;
  canonicalPath: string;
  /** Verified provider session cwd, which may be nested below the checkout root. */
  canonicalCwd?: string;
  repositoryRoot: string;
  branch?: string;
  head?: string;
  dirtyStateDigest: string;
  ownerAgentId: string;
  ownerRevision: number;
  state: WorktreeLeaseState;
  escrow?: WorktreeEscrowV1;
  coordinator?: WorktreeCoordinatorV1;
  acquiredAt: string;
  updatedAt: string;
  releasedAt?: string;
}

export type OrchestrationEntityKind =
  | "host"
  | "agent"
  | "message"
  | "role-claim"
  | "effect"
  | "worktree-lease";

