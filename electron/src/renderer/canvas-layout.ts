import type { AgentNodeV1 } from "../shared/orchestration";
import { ORCHESTRATOR_ROLE } from "../shared/orchestration";
import type { OrchestrationSnapshotDto } from "../shared/ipc";
import { statusWord } from "./components/status-words";

/**
 * One status, one word — on the orchestration surfaces too. The canvas node and
 * the agent feed printed the host's own tokens (`working`, `starting`,
 * `planned`, `unknown`) while every other surface in the app said "Working" or
 * "Needs attention" for the same idea.
 *
 * The four states an agent shares with a conversation are not named again here:
 * they are asked of `status-words.ts`, which owns those words. What this map
 * adds is the six an orchestrated agent has and a conversation does not — a
 * lifecycle, not a second opinion about the four.
 */
const ORCHESTRATION_ONLY_WORD: Readonly<Record<string, string>> = {
  planned: "Planned",
  starting: "Starting",
  ready: "Ready",
  completed: "Completed",
  cancelled: "Cancelled",
  unknown: "Unknown",
};

export function orchestrationStatusWord(state: AgentNodeV1["state"] | string): string {
  if (state === "waiting" || state === "working" || state === "idle" || state === "failed") {
    return statusWord(state);
  }
  const known = ORCHESTRATION_ONLY_WORD[state];
  if (known !== undefined) return known;
  // A token this build has not heard of is not the host's own `unknown` state,
  // and printing "Unknown" for both loses the only thing the new word carried.
  // Capitalise it and show it: a strange word beats a wrong one.
  return state === "" ? "Unknown" : state.charAt(0).toUpperCase() + state.slice(1);
}

/** The node card's exact drawn box. Ports and edge geometry are derived from it. */
export const CANVAS_NODE_SIZE = { w: 220, h: 96 } as const;

/**
 * A coordinated card is taller than that box: task/scope, state/lease and the
 * fence pill are three more rows, and each of them can wrap. The card grows
 * downward (minHeight, not height), so a row holding one is laid out against
 * this height instead and never lands on the row beneath it.
 */
export const CANVAS_COORDINATED_NODE_H = 168;

/** Renderer-local presentation only. Nothing here carries authority or lineage. */
export const CANVAS_LAYOUT_KEY = "interface.canvas.v1";

const ORIGIN_X = 40;
const ORCHESTRATOR_Y = 40;
const ROW_GAP = 64;
const COLUMN_STEP = 280;
const COORDINATE_LIMIT = 100_000;
const MAX_PROJECTS = 64;
const MAX_NODES = 512;
/** Keys that would reach Object.prototype through a plain-object assignment. */
const UNSAFE_KEYS: ReadonlySet<string> = new Set(["__proto__", "constructor", "prototype"]);

export interface CanvasNodePosition { x: number; y: number }

/** What one project's canvas remembers: dragged positions, and removed nodes. */
export interface CanvasProjectLayout {
  nodes: Record<string, CanvasNodePosition>;
  hidden: string[];
}

export interface CanvasLayoutState {
  version: 1;
  byProject: Record<string, CanvasProjectLayout>;
}

export const EMPTY_CANVAS_LAYOUT: CanvasLayoutState = Object.freeze({
  version: 1, byProject: {},
});

const EMPTY_PROJECT_LAYOUT: CanvasProjectLayout = Object.freeze({ nodes: {}, hidden: [] });

const EMPTY_COORDINATED: ReadonlySet<string> = new Set<string>();

export type EdgeVisualState = "idle" | "inflight" | "settled" | "failed" | "unknown";

function record(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

function coordinate(value: unknown): number | null {
  if (typeof value !== "number" || !Number.isFinite(value)) return null;
  return Math.min(COORDINATE_LIMIT, Math.max(-COORDINATE_LIMIT, Math.round(value)));
}

function position(value: unknown): CanvasNodePosition | null {
  const source = record(value);
  if (source === null) return null;
  const x = coordinate(source.x);
  const y = coordinate(source.y);
  return x === null || y === null ? null : { x, y };
}

/** A stored key is untrusted JSON, so one that would walk up the prototype is dropped. */
function safeKey(key: string): boolean {
  return key.length > 0 && !UNSAFE_KEYS.has(key);
}

/**
 * The first stored shape put a project's positions directly under its root; this
 * one nests them under `nodes` beside the ids the human removed from the canvas.
 * A stored position is never itself a map of positions, so both parse without a
 * version bump and an older canvas keeps every position it had.
 */
function projectLayout(value: unknown): CanvasProjectLayout | null {
  const source = record(value);
  if (source === null) return null;
  const rawHidden: unknown = source.hidden;
  const nested = Array.isArray(rawHidden)
    || (record(source.nodes) !== null && position(source.nodes) === null);
  const stored = (nested ? record(source.nodes) : source) ?? {};
  const nodes: Record<string, CanvasNodePosition> = {};
  for (const [agentId, raw] of Object.entries(stored).slice(0, MAX_NODES)) {
    const at = position(raw);
    if (!safeKey(agentId) || at === null) continue;
    nodes[agentId] = at;
  }
  const hidden: string[] = [];
  if (nested && Array.isArray(rawHidden)) {
    for (const agentId of (rawHidden as unknown[]).slice(0, MAX_NODES)) {
      if (typeof agentId !== "string" || !safeKey(agentId) || hidden.includes(agentId)) continue;
      hidden.push(agentId);
    }
  }
  return Object.keys(nodes).length === 0 && hidden.length === 0 ? null : { nodes, hidden };
}

function normalize(value: unknown): CanvasLayoutState {
  const source = record(value);
  if (source === null || source.version !== 1) return EMPTY_CANVAS_LAYOUT;
  const projects = record(source.byProject);
  if (projects === null) return EMPTY_CANVAS_LAYOUT;
  const byProject: CanvasLayoutState["byProject"] = {};
  for (const [root, entries] of Object.entries(projects).slice(0, MAX_PROJECTS)) {
    const project = projectLayout(entries);
    if (!safeKey(root) || project === null) continue;
    byProject[root] = project;
  }
  return { version: 1, byProject };
}

/**
 * Normalizes untrusted stored layout the way parseWorkspaceState does, and
 * accepts either the stored JSON text or an already-parsed value so callers do
 * not have to guess which side of localStorage they are on.
 */
export function parseCanvasLayout(raw: unknown): CanvasLayoutState {
  if (typeof raw === "string") {
    if (raw.length === 0) return EMPTY_CANVAS_LAYOUT;
    try { return normalize(JSON.parse(raw) as unknown); }
    catch { return EMPTY_CANVAS_LAYOUT; }
  }
  return normalize(raw);
}

/** One canvas per project root: the worktree an isolated worker runs in is not one. */
export function canvasProjectRoot(agent: AgentNodeV1): string {
  return agent.scope.canonicalProjectRoot ?? agent.scope.canonicalCwd;
}

/**
 * Deterministic auto-placement: orchestrators then unlinked agents across the
 * top row, every worker beneath in agentId order. A stored position always
 * wins, so this only ever decides where a node the human has not moved lands.
 */
export function placeCanvasNodes(agents: AgentNodeV1[],
    placed: Record<string, CanvasNodePosition>,
    coordinated: ReadonlySet<string> = EMPTY_COORDINATED): Record<string, CanvasNodePosition> {
  const sorted = [...agents].sort((left, right) => left.agentId.localeCompare(right.agentId));
  const orchestrators = sorted.filter(agent => agent.role === ORCHESTRATOR_ROLE);
  const orchestratorIds = new Set(orchestrators.map(agent => agent.agentId));
  const workers = orchestrators.flatMap(orchestrator =>
    sorted.filter(agent => agent.parentAgentId === orchestrator.agentId
      && !orchestratorIds.has(agent.agentId)));
  const workerIds = new Set(workers.map(agent => agent.agentId));
  const unlinked = sorted.filter(agent => !orchestratorIds.has(agent.agentId)
    && !workerIds.has(agent.agentId));
  const topRow = [...orchestrators, ...unlinked];
  // A coordinated card in the row above grows past the fixed box, so the row
  // below drops by the difference rather than being overlapped by it.
  const workerY = ORCHESTRATOR_Y + ROW_GAP
    + (topRow.some(agent => coordinated.has(agent.agentId))
      ? CANVAS_COORDINATED_NODE_H : CANVAS_NODE_SIZE.h);
  const auto: Record<string, CanvasNodePosition> = {};
  topRow.forEach((agent, index) => {
    auto[agent.agentId] = { x: ORIGIN_X + index * COLUMN_STEP, y: ORCHESTRATOR_Y };
  });
  workers.forEach((agent, index) => {
    auto[agent.agentId] = { x: ORIGIN_X + index * COLUMN_STEP, y: workerY };
  });
  const result: Record<string, CanvasNodePosition> = {};
  for (const agent of sorted) {
    result[agent.agentId] = placed[agent.agentId] ?? auto[agent.agentId]!;
  }
  return result;
}

/** Orchestrator bottom-centre port to worker top-centre port. */
export function edgePath(from: CanvasNodePosition, to: CanvasNodePosition): string {
  const x1 = from.x + CANVAS_NODE_SIZE.w / 2;
  const y1 = from.y + CANVAS_NODE_SIZE.h;
  const x2 = to.x + CANVAS_NODE_SIZE.w / 2;
  const y2 = to.y;
  // Half the vertical gap and never more: a wider bend would push the first
  // control point below the second and draw an S-loop between near nodes.
  const bend = Math.max(0, (y2 - y1) / 2);
  return `M ${x1} ${y1} C ${x1} ${y1 + bend} ${x2} ${y2 - bend} ${x2} ${y2}`;
}

/**
 * The newest delegation the worker's own parent sent it, or null. The control
 * projection carries no message text, so the edge tooltip reads its delivery
 * state and `createdAt` off this and nothing else.
 */
export function newestDelegation(snapshot: OrchestrationSnapshotDto | null,
    workerAgentId: string): OrchestrationSnapshotDto["messages"][number] | null {
  if (snapshot === null) return null;
  const worker = snapshot.agents.find(agent => agent.agentId === workerAgentId);
  const parentAgentId = worker?.parentAgentId;
  if (parentAgentId === undefined) return null;
  let newest: OrchestrationSnapshotDto["messages"][number] | undefined;
  for (const message of snapshot.messages) {
    if (message.recipientAgentId !== workerAgentId
      || message.senderAgentId !== parentAgentId) continue;
    if (newest === undefined || message.createdAt > newest.createdAt
      || (message.createdAt === newest.createdAt
        && message.messageId > newest.messageId)) newest = message;
  }
  return newest ?? null;
}

/**
 * That same delegation as a drawable state. Ambiguity renders as "unknown"
 * rather than as success — the effect semantics this app already keeps for
 * provider outcomes.
 */
export function deliveryEdgeState(
    newest: OrchestrationSnapshotDto["messages"][number] | null): EdgeVisualState {
  if (newest === null) return "idle";
  switch (newest.deliveryState) {
    case "requested": case "queued": case "provider_accepted": return "inflight";
    case "settled": case "processed": return "settled";
    case "failed": return "failed";
    default: return "unknown";
  }
}

function projectOf(layout: CanvasLayoutState, projectRoot: string): CanvasProjectLayout {
  return Object.hasOwn(layout.byProject, projectRoot)
    ? layout.byProject[projectRoot]! : EMPTY_PROJECT_LAYOUT;
}

/** The agents the human took off this project's canvas. */
export function hiddenNodes(layout: CanvasLayoutState,
    projectRoot: string): readonly string[] {
  return projectOf(layout, projectRoot).hidden;
}

/** Records one dragged position. Called on drag end only, never per pointer move. */
export function moveNode(layout: CanvasLayoutState, projectRoot: string, agentId: string,
    pos: CanvasNodePosition): CanvasLayoutState {
  const at = position(pos);
  if (!safeKey(projectRoot) || !safeKey(agentId) || at === null) return layout;
  const project = projectOf(layout, projectRoot);
  return {
    version: 1,
    byProject: {
      ...layout.byProject,
      [projectRoot]: { nodes: { ...project.nodes, [agentId]: at }, hidden: project.hidden },
    },
  };
}

/** Forgets one stored position; auto-placement decides where that node lands next. */
export function forgetNode(layout: CanvasLayoutState, projectRoot: string,
    agentId: string): CanvasLayoutState {
  const project = projectOf(layout, projectRoot);
  if (!Object.hasOwn(project.nodes, agentId)) return layout;
  const { [agentId]: _dropped, ...rest } = project.nodes;
  return { version: 1, byProject: { ...layout.byProject,
    [projectRoot]: { nodes: rest, hidden: project.hidden } } };
}

/**
 * "Remove from canvas" takes the node off this canvas and forgets where it sat.
 * Lineage, registration and the provider session are untouched: nothing here
 * reaches the host, and adding the session back un-hides the same node.
 */
export function hideNode(layout: CanvasLayoutState, projectRoot: string,
    agentId: string): CanvasLayoutState {
  if (!safeKey(projectRoot) || !safeKey(agentId)) return layout;
  const dropped = forgetNode(layout, projectRoot, agentId);
  const project = projectOf(dropped, projectRoot);
  if (project.hidden.includes(agentId)) return dropped;
  return { version: 1, byProject: { ...dropped.byProject,
    [projectRoot]: { nodes: project.nodes, hidden: [...project.hidden, agentId] } } };
}

/** Puts a removed node back on the canvas. */
export function unhideNode(layout: CanvasLayoutState, projectRoot: string,
    agentId: string): CanvasLayoutState {
  const project = projectOf(layout, projectRoot);
  if (!project.hidden.includes(agentId)) return layout;
  return { version: 1, byProject: { ...layout.byProject,
    [projectRoot]: { nodes: project.nodes,
      hidden: project.hidden.filter(id => id !== agentId) } } };
}

/** Auto-arrange: drop every stored position for this project, keep what is hidden. */
export function clearNodes(layout: CanvasLayoutState, projectRoot: string): CanvasLayoutState {
  if (!safeKey(projectRoot)) return layout;
  return { version: 1, byProject: { ...layout.byProject,
    [projectRoot]: { nodes: {}, hidden: projectOf(layout, projectRoot).hidden } } };
}
