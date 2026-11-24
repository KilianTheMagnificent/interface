/**
 * The orchestration request and projection DTOs the renderer exchanges with
 * the Rust host over IPC. The Rust host owns the protocol and the stores.
 *
 * The DTOs deliberately carry plain bounded data. Filesystem paths, provider
 * handles, writer leases, and broker capabilities never cross into a
 * renderer. Every mutating request is validated again by the host that owns
 * the durable stores and provider effects.
 */

import type {
  AgentNodeV1,
  MailboxMessageV1,
  OrchestrationCapabilityFacet,
  OrchestrationEntityKind,
  OrchestratorAccessCeiling,
  OrchestratorAutonomy,
  OrchestratorPolicyV1,
  RoleClaimV1,
  WorktreeLeaseV1,
} from "./orchestration.js";

export interface OrchestrationAgentCreateRequestV1 {
  agentId?: string;
  engineId: string;
  name: string;
  prompt: string;
  canonicalCwd: string;
  accessMode: string;
  model?: string;
  effort?: string;
  parentAgentId?: string;
  role?: string;
  requiredCapabilities?: OrchestrationCapabilityFacet[];
  clientRequestId: string;
}

export interface OrchestrationAgentSendRequestV1 {
  recipientAgentId: string;
  body: string;
  expectReply: boolean;
  clientRequestId: string;
  senderAgentId?: string;
  threadId?: string;
  parentMessageId?: string;
}

/** Start one durable builder -> reviewer -> builder correction workflow. */
export interface AdversarialReviewStartRequestV1 {
  builderEngineId: "claude" | "codex";
  reviewerEngineId: "claude" | "codex";
  canonicalCwd: string;
  objective: string;
  clientRequestId: string;
}

export interface AdversarialReviewStartV1 {
  version: 1;
  builder: OrchestrationControlAgentV1;
  reviewer: OrchestrationControlAgentV1;
}

/**
 * Human designation of one orchestrator. Omitted bounds keep the stored value,
 * or ORCHESTRATOR_POLICY_DEFAULTS on first activation; the host is the only
 * writer of activatedAt and the usage counters.
 */
export interface OrchestratorPolicySetRequestV1 {
  agentId: string;
  autonomy?: OrchestratorAutonomy;
  maxAccessMode?: OrchestratorAccessCeiling;
  maxWorkers?: number;
  maxConcurrent?: number;
  maxDelegations?: number;
  wallClockMs?: number;
  integration?: "orchestrator" | "human";
  coordinationRequired?: boolean;
  /** Refuse a `read-only` worker: the human said the team does the work. */
  workersWrite?: boolean;
  allowUnverifiedFence?: boolean;
  modelCatalogue?: "on-demand" | "on-activate";
  /** `[]` clears the stored allowlist. */
  modelAllowlist?: string[];
  /** `""` clears the stored routing notes. */
  modelNotes?: string;
  clientRequestId: string;
}

export interface OrchestratorPolicyClearRequestV1 {
  agentId: string;
  clientRequestId: string;
}

export interface OrchestrationAgentBindParentRequestV1 {
  agentId: string;
  parentAgentId: string | null;
  clientRequestId: string;
}

export interface OrchestrationAgentRegisterSessionRequestV1 {
  sessionKey: string;
  name?: string;
  clientRequestId: string;
}

/** Host-projected agent identity; provider-private session ids never cross the control wire. */
export type OrchestrationControlAgentV1 = Omit<AgentNodeV1, "providerSessionId">;

export interface OrchestrationControlMessageSummaryV1 {
  version: 1;
  messageId: string;
  threadId: string;
  parentMessageId?: string;
  origin: MailboxMessageV1["origin"];
  senderAgentId?: string;
  recipientAgentId: string;
  expectReply: boolean;
  deliveryState: MailboxMessageV1["receipts"][number]["state"];
  createdAt: string;
  updatedAt: string;
}

export interface OrchestrationControlSnapshotV1 {
  version: 1;
  hostId: string;
  runtimeEpoch: string;
  revision: number;
  createdAt: string;
  updatedAt: string;
  agents: OrchestrationControlAgentV1[];
  messages: OrchestrationControlMessageSummaryV1[];
  roleClaims: RoleClaimV1[];
  worktreeLeases: WorktreeLeaseV1[];
  orchestratorPolicies: OrchestratorPolicyV1[];
  projection: {
    version: 1;
    truncated: boolean;
    eventFloorSequence: number;
    omitted: {
      agents: number;
      messages: number;
      roleClaims: number;
      worktreeLeases: number;
      orchestratorPolicies: number;
    };
  };
}

export interface OrchestrationInboxPageV1 {
  version: 1;
  agentId: string;
  threadId?: string;
  beforeMessageId?: string;
  nextBeforeMessageId?: string;
  hasMore: boolean;
  messages: MailboxMessageV1[];
}

export interface OrchestrationControlEventV1 {
  version: 1;
  sequence: number;
  kind: string;
  entityKind: OrchestrationEntityKind;
  entityId: string;
  at: string;
  summary: string;
}

export interface OrchestrationEventPageV1 {
  version: 1;
  requestedSequence: number;
  floorSequence: number;
  nextSequence: number;
  truncated: boolean;
  hasMore: boolean;
  events: OrchestrationControlEventV1[];
}

export interface OrchestrationTranscriptPageV1 {
  version: 1;
  agentId: string;
  requestedLines: number;
  /** True when the requested tail exceeded the public byte/line window. */
  truncated: boolean;
  lines: string[];
}

export interface OrchestratorSubmitResultV1 {
  submissionId: string;
  changes: number;
  state: string;
}

/** Human coordination acts. None of them require orchestrator autonomy. */
export interface WorkerSubmitRequestV1 { agentId: string; clientRequestId: string }
export interface WorkerIntegrateRequestV1 {
  agentId: string;
  dryRun?: boolean;
  clientRequestId: string;
}
export interface WorkerAbandonRequestV1 { agentId: string; clientRequestId: string }
export interface WorkerAckRequestV1 {
  agentId: string;
  keep: string;
  clientRequestId: string;
}
export interface CoordinationStatusRequestV1 { canonicalProjectRoot: string }

export interface CoordinationStatusV1 {
  registered: boolean;
  activeTasks: number;
  /** Published this run and still uncommitted in the canonical working tree. */
  publishedUncommittedPaths: string[];
  requiredChecks: number;
}

/**
 * The engine, model and effort ids a spawn may name, bounded for an
 * orchestrator's own context. `catalogue` is the honesty field: `unavailable`
 * means the host validated nothing and the spawn passed through.
 */
export interface OrchestratorModelsV1 {
  engines: Array<{
    engineId: string;
    ok: boolean;
    default?: string;
    models: Array<{
      id: string;
      name: string;
      efforts: string[];
      default?: true;
      note?: string;
    }>;
  }>;
  /** Rungs Interface itself maps, with the exact meaning of each. */
  synthetic: { ultracode: string };
  /** `policy.modelNotes`, human-authored and returned verbatim. */
  guidance?: string;
  allowlist?: string[];
  catalogue: "live" | "partial" | "unavailable";
}
