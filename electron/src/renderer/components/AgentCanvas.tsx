import { providerLabel } from "../provider-labels";
import { lastSegment } from "../display-paths";
import { withoutIpcErrorShape } from "../ipc-message";
import { useCallback, useId, useMemo, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent, MouseEvent as ReactMouseEvent,
  PointerEvent as ReactPointerEvent } from "react";
import {
  ORCHESTRATION_LIMITS, ORCHESTRATOR_ROLE, WORKER_ROLE,
} from "../../shared/orchestration";
import type { OrchestratorPolicyV1, WorktreeCoordinatorV1 } from "../../shared/orchestration";
import type {
  CoordinationStatusV1,
  OrchestrationAgentBindParentRequestV1,
  OrchestrationAgentCreateRequestV1,
  OrchestrationAgentRegisterSessionRequestV1,
  OrchestratorModelsV1,
  OrchestratorPolicyClearRequestV1,
  OrchestratorPolicySetRequestV1,
  WorkerAbandonRequestV1,
  WorkerAckRequestV1,
  WorkerIntegrateRequestV1,
  WorkerSubmitRequestV1,
} from "../../shared/orchestration-control";
import type { OrchestrationAgentStopRequest, OrchestrationSnapshotDto } from "../../shared/ipc";
import {
  CANVAS_NODE_SIZE, canvasProjectRoot, clearNodes, deliveryEdgeState, edgePath, hiddenNodes,
  hideNode, moveNode, newestDelegation, orchestrationStatusWord, placeCanvasNodes, unhideNode,
  type CanvasLayoutState, type CanvasNodePosition,
} from "../canvas-layout";
import { type AppSelectOption } from "./AppSelect";
import { FormSelect } from "./FormSelect";
import { ProviderLogo } from "./ProviderLogo";

type CanvasAgent = OrchestrationSnapshotDto["agents"][number];
const NO_CANVAS_CHILDREN: CanvasAgent[] = [];

export interface AgentCanvasSession {
  key: string;
  title: string;
  engine: string;
  cwd: string;
  model?: string;
  /** Absent means unknown; null means the provider default. */
  effort?: string | null;
}

export interface AgentCanvasProps {
  error?: string | null;
  snapshot: OrchestrationSnapshotDto | null;
  sessions: readonly AgentCanvasSession[];
  projectRoot: string;
  projectRoots: readonly string[];
  onSelectProjectRoot: (root: string) => void;
  /** The native folder dialog. Resolves null when the human cancels it. */
  onBrowseProjectRoot: () => Promise<string | null>;
  layout: CanvasLayoutState;
  onLayoutChange: (next: CanvasLayoutState) => void;
  onRegisterSession: (request: OrchestrationAgentRegisterSessionRequestV1) => Promise<void>;
  onCreateWorker: (request: OrchestrationAgentCreateRequestV1) => Promise<void>;
  onBindParent: (request: OrchestrationAgentBindParentRequestV1) => Promise<void>;
  onSetPolicy: (request: OrchestratorPolicySetRequestV1) => Promise<void>;
  onClearPolicy: (request: OrchestratorPolicyClearRequestV1) => Promise<void>;
  onStopAgent: (request: OrchestrationAgentStopRequest) => Promise<void>;
  onSubmitWorker: (request: WorkerSubmitRequestV1) => Promise<void>;
  /**
   * The verify is asynchronous: this answers with the claim as soon as the task
   * is claimed, and the accepted paths, receipts or refusal arrive later as a
   * worker notice the orchestrator reads.
   */
  onIntegrateWorker: (request: WorkerIntegrateRequestV1) =>
    Promise<{ taskId: string; state: "verifying" }>;
  onAbandonTask: (request: WorkerAbandonRequestV1) => Promise<void>;
  onAckObligation: (request: WorkerAckRequestV1) => Promise<void>;
  /** Null until getCoordinationStatus answers, and whenever wcp is unavailable. */
  coordination: CoordinationStatusV1 | null;
  models: OrchestratorModelsV1 | null;
  onOpenSession: (sessionKey: string, newPane: boolean) => void;
}

const ZOOM_MIN = .5;
const ZOOM_MAX = 2;
const ZOOM_STEP = .1;
const NUDGE = 8;
const NUDGE_FAST = 32;
const ACCESS_MODES = ["read-only", "coordinated", "isolated", "project"] as const;
const ACCESS_LABELS: Record<(typeof ACCESS_MODES)[number], string> = {
  "read-only": "Read only", coordinated: "Coordinated writes", isolated: "Isolated worktree", project: "Project checkout",
};
// Module constants, not per-render arrays: AppSelect memoizes its option
// metadata on the options' identity.
const ACCESS_ROWS: readonly AppSelectOption[] = ACCESS_MODES.map(value => ({ value, label: ACCESS_LABELS[value] }));
const INTEGRATION_MODES = ["orchestrator", "human"] as const;
const CATALOGUE_MODES = ["on-demand", "on-activate"] as const;
const RETIRED_TASK_STATES = new Set(["accepted", "abandoned", "cancelled", "retired"]);
// Sentinels for rows that are an action rather than a value. A NUL cannot
// occur in a project path or a provider model id, so neither can be shadowed.
const BROWSE_ROW = "\u0000browse";
const BROWSE_OPTION: AppSelectOption = { value: BROWSE_ROW, label: "Browse\u2026", meta: "pick any folder" };
const PROVIDER_DEFAULT_ROW = "";
const MODEL_ALLOWLIST_MAX = 32;
const MODEL_NOTES_MAX_BYTES = 1_024;
const ALLOWLIST_REFUSAL = "Allowlist entries look like codex:gpt-5.6-luna or codex:*.";
const NOTES_REFUSAL = `Routing notes are limited to ${MODEL_NOTES_MAX_BYTES} bytes.`;
const EMPTY_HINT = "Add a session, make it the orchestrator, draw lines to workers.";
const STAR_REFUSAL = "Star topology: an orchestrator cannot be a worker.";
const NO_SESSION_REFUSAL = "That agent has no recoverable provider session yet.";
const LINKED_REFUSAL = "Unlink before removing from the canvas.";
const COORDINATION_REFUSAL = "Integrate, or abandon the task first.";
const USAGE_HINT = "Tab cycles nodes. Arrows move the selected node, Shift for a larger step. "
  + "Enter opens it, Shift+Enter opens it in another pane. L links an orchestrator to a worker, "
  + "M opens the actions menu for the selected node. Delete unlinks a selected edge. "
  + "Minus, plus and zero change the zoom. Escape clears the selection.";

function requestId(): string {
  return globalThis.crypto.randomUUID();
}

/**
 * The launch form for one worker. The engine was never the whole choice — the
 * model is what the work costs and how good it is — so the catalogue the host
 * already probed this run is offered here instead of being left to whatever the
 * provider defaults to. Every choice is derived rather than synced: the probe
 * answers after the first render, and an effect copying it into state would
 * fight the human's own selection.
 */
function NewWorkerForm(props: {
  engines: readonly string[];
  models: OrchestratorModelsV1 | null;
  disabled: boolean;
  onCreate: (fields: { engineId: string; name: string; brief: string; accessMode: string;
    model?: string; effort?: string }) => void;
}) {
  const [engineChoice, setEngineChoice] = useState("");
  const [accessChoice, setAccessChoice] = useState<string>("project");
  const [modelChoice, setModelChoice] = useState(PROVIDER_DEFAULT_ROW);
  const [effortChoice, setEffortChoice] = useState(PROVIDER_DEFAULT_ROW);
  const engineId = props.engines.includes(engineChoice)
    ? engineChoice : props.engines[0] ?? "";
  const modelRows = engineModelOptions(props.models, engineId);
  const model = modelRows.some(row => row.value === modelChoice)
    ? modelChoice : PROVIDER_DEFAULT_ROW;
  const efforts = modelEffortLevels(props.models, engineId, model);
  const effort = efforts.includes(effortChoice) ? effortChoice : PROVIDER_DEFAULT_ROW;
  const noEngine = props.engines.length === 0;
  return <form className="canvas-new-worker" aria-label="New worker" onSubmit={event => {
    event.preventDefault();
    const form = event.currentTarget;
    const data = new FormData(form);
    props.onCreate({
      engineId,
      name: String(data.get("name") ?? "").trim(),
      brief: String(data.get("brief") ?? "").trim(),
      accessMode: accessChoice,
      // The provider-default row is the absence of a choice, so it travels as
      // an absent field rather than as an id no catalogue lists.
      ...(model === PROVIDER_DEFAULT_ROW ? {} : { model }),
      ...(effort === PROVIDER_DEFAULT_ROW ? {} : { effort }),
    });
    form.reset();
    setAccessChoice("project");
  }}>
    <label><span>Name</span><input name="name" required maxLength={120}
      disabled={props.disabled} /></label>
    <FormSelect fieldClassName="canvas-field" label="Engine" value={engineId}
      placeholder="No engine"
      options={props.engines.map(value => ({ value, label: providerLabel(value) }))} onChange={setEngineChoice}
      disabled={props.disabled || noEngine} />
    <FormSelect fieldClassName="canvas-field" label="Model" value={model} options={modelRows}
      onChange={setModelChoice} disabled={props.disabled || noEngine} />
    {efforts.length > 0 && <FormSelect fieldClassName="canvas-field" label="Effort" value={effort}
      options={[{ value: PROVIDER_DEFAULT_ROW, label: "Default" }, ...plainRows(efforts)]}
      onChange={setEffortChoice} disabled={props.disabled} />}
    <FormSelect fieldClassName="canvas-field" label="Access" value={accessChoice}
      options={ACCESS_ROWS}
      onChange={setAccessChoice} disabled={props.disabled} />
    <label className="canvas-wide"><span>Brief</span><input name="brief" required
      maxLength={4_096} disabled={props.disabled} /></label>
    <button className="btn btn-primary canvas-create-worker" type="submit" disabled={props.disabled || noEngine}>Create worker</button>
  </form>;
}

/** Plain rows for a fixed vocabulary the provider owns. */
function plainRows(values: readonly string[]): AppSelectOption[] {
  return values.map(value => ({ value, label: value }));
}

/**
 * A project reads as its folder name; the path above it is context, not the
 * headline. Showing the whole absolute path as the label made a list of two
 * dozen roots unreadable and identical for the first forty characters.
 */
function projectRootOptions(roots: readonly string[]): AppSelectOption[] {
  return roots.map(root => {
    const label = lastSegment(root) || root;
    const parent = root.slice(0, Math.max(0, root.length - label.length - 1));
    return { value: root, label, ...(parent === "" ? {} : { meta: parent }) };
  });
}

/** The models one engine advertised this run, plus its provider default. */
function engineModelOptions(models: OrchestratorModelsV1 | null,
    engineId: string): AppSelectOption[] {
  const rows = models?.engines.find(entry => entry.engineId === engineId)?.models ?? [];
  return [{ value: PROVIDER_DEFAULT_ROW, label: "Provider default",
    meta: "whatever the engine launches with" },
  ...rows.map(model => ({
    value: model.id,
    label: model.name,
    ...(model.name === model.id ? {} : { meta: model.id }),
    ...(model.note === undefined ? {} : { description: model.note }),
  }))];
}

/** The effort ladder the chosen model advertises; empty hides the control. */
function modelEffortLevels(models: OrchestratorModelsV1 | null, engineId: string,
    modelId: string): string[] {
  const rows = models?.engines.find(entry => entry.engineId === engineId)?.models ?? [];
  const chosen = modelId === PROVIDER_DEFAULT_ROW
    ? rows.find(model => model.default === true) : rows.find(model => model.id === modelId);
  return [...(chosen?.efforts ?? [])];
}

function statusTone(state: CanvasAgent["state"]): string {
  if (state === "working" || state === "starting") return "working";
  if (state === "waiting") return "waiting";
  if (state === "failed" || state === "cancelled" || state === "unknown") return "failed";
  return "idle";
}

function agentName(agent: CanvasAgent): string {
  return agent.name ?? agent.agentId;
}

/**
 * Each worker's wcp record, keyed by agent id. A record lives on the worker's
 * own *active* worktree lease, or nowhere. A rebase releases the retired
 * worker's lease without erasing the coordinator it carried, so a released
 * lease still describes a task this agent no longer holds — reading it would
 * keep the retired node coordinated forever.
 */
export function canvasCoordinators(snapshot: OrchestrationSnapshotDto | null):
    Map<string, WorktreeCoordinatorV1> {
  const byAgent = new Map<string, WorktreeCoordinatorV1>();
  for (const lease of snapshot?.worktreeLeases ?? []) {
    if (lease.state !== "active" || lease.coordinator === undefined) continue;
    if (!byAgent.has(lease.ownerAgentId)) byAgent.set(lease.ownerAgentId, lease.coordinator);
  }
  return byAgent;
}

/** One agent's record under the rule above. */
export function canvasCoordinator(snapshot: OrchestrationSnapshotDto | null,
    agentId: string): WorktreeCoordinatorV1 | undefined {
  return canvasCoordinators(snapshot).get(agentId);
}

/** One claimed verify: the worker, the task it named, and the snapshot it saw. */
export interface CanvasVerifyClaim {
  agentId: string;
  taskId: string;
  at: OrchestrationSnapshotDto | null;
}

/**
 * The standing "a verify is running" fact, derived rather than remembered. A
 * publish moves the coordinator to `verifying` and the record answers for the
 * whole run; a dry run is left in `submitted` by wcp — it is deliberately still
 * heartbeatable — so it has no record of its own and stands until the next
 * snapshot lands after the claim. Either way the notice cannot outlive the
 * state it reports, which a local flag reset by the NEXT integrate did.
 */
export function canvasVerifyingNotice(snapshot: OrchestrationSnapshotDto | null,
    claim: CanvasVerifyClaim | null): string | null {
  if (claim === null) return null;
  if (snapshot !== claim.at
    && canvasCoordinator(snapshot, claim.agentId)?.taskState !== "verifying") return null;
  return `Verifying ${claim.taskId} \u2014 the result arrives on this canvas.`;
}

/** Fail closed: a task whose state we cannot name is treated as still holding scope. */
export function coordinatedTaskActive(coordinator: WorktreeCoordinatorV1): boolean {
  return coordinator.taskState === undefined || !RETIRED_TASK_STATES.has(coordinator.taskState);
}

export function scopeSummary(writeScope: readonly string[]): string {
  const first = writeScope[0];
  if (first === undefined) return "no scope";
  return writeScope.length === 1 ? first : `${first} +${writeScope.length - 1}`;
}

/**
 * The canvas navigates by aria-activedescendant, so the group's name is the
 * whole announcement. `fence` is the one card field carrying a safety meaning
 * and the pill states it in colour, so it travels in the name too.
 */
export function coordinationLabel(coordinator: WorktreeCoordinatorV1, now: number): string {
  return `task ${coordinator.taskId}, fence ${coordinator.fence}, `
    + leaseCountdown(coordinator.leaseExpiresAt, now);
}

/** Recomputed on every render from the timestamp; the canvas owns no timer. */
export function leaseCountdown(leaseExpiresAt: string, now: number): string {
  const at = Date.parse(leaseExpiresAt);
  if (Number.isNaN(at)) return "lease unknown";
  const left = at - now;
  if (left <= 0) return "lease expired";
  const minutes = Math.floor(left / 60_000);
  return minutes >= 1 ? `lease ${minutes}m left` : "lease under 1m left";
}

export function engineLine(agent: CanvasAgent,
    session: AgentCanvasSession | undefined): string {
  const parts = [providerLabel(agent.engineId)];
  if (session?.model !== undefined && session.model !== "") parts.push(session.model);
  if (session?.effort !== undefined && session.effort !== null) parts.push(session.effort);
  return parts.join(" \u00b7 ");
}

export function parseModelAllowlist(raw: string):
    { entries: string[] } | { refusal: string } {
  const entries = raw.split("\n").map(line => line.trim()).filter(line => line !== "");
  if (entries.length > MODEL_ALLOWLIST_MAX) {
    return { refusal: `At most ${MODEL_ALLOWLIST_MAX} allowlist entries.` };
  }
  for (const entry of entries) {
    const at = entry.lastIndexOf(":");
    if (at <= 0 || at === entry.length - 1 || /\s/u.test(entry)) {
      return { refusal: ALLOWLIST_REFUSAL };
    }
  }
  return { entries };
}

export function modelNotesRefusal(notes: string): string | null {
  return new TextEncoder().encode(notes).length > MODEL_NOTES_MAX_BYTES ? NOTES_REFUSAL : null;
}

export function modelHint(models: OrchestratorModelsV1): string {
  const ids = models.engines.flatMap(engine =>
    engine.models.map(model => `${engine.engineId}:${model.id}`));
  if (ids.length === 0) return `Model catalogue ${models.catalogue}.`;
  return `Known ids: ${ids.slice(0, 12).join(", ")}${ids.length > 12 ? ", \u2026" : ""}`;
}

export interface CanvasKeyContext {
  order: readonly string[];
  selectedAgentId: string | null;
  selectedEdgeId: string | null;
  orchestratorIds: ReadonlySet<string>;
}

export type CanvasKeyAction =
  | { kind: "ignore" }
  | { kind: "leave" }
  | { kind: "select"; agentId: string }
  | { kind: "clear" }
  | { kind: "unlink"; agentId: string }
  | { kind: "zoom"; delta: number }
  | { kind: "zoom-reset" }
  | { kind: "open"; agentId: string; newPane: boolean }
  | { kind: "link"; agentId: string }
  | { kind: "menu"; agentId: string }
  | { kind: "refuse"; message: string }
  | { kind: "nudge"; agentId: string; dx: number; dy: number };

export function canvasKeyAction(event: { key: string; shiftKey: boolean },
    context: CanvasKeyContext): CanvasKeyAction {
  const step = event.shiftKey ? NUDGE_FAST : NUDGE;
  const order = context.order;
  if (event.key === "Tab") {
    // WCAG 2.1.2: Tab cycles the nodes, but at either end of the cycle it drops
    // the selection and yields, so the viewport is never a keyboard trap.
    const at = context.selectedAgentId === null ? -1 : order.indexOf(context.selectedAgentId);
    const next = order[at + (event.shiftKey ? -1 : 1)];
    return next === undefined ? { kind: "leave" } : { kind: "select", agentId: next };
  }
  if (event.key === "Escape") return { kind: "clear" };
  if (event.key === "Delete" || event.key === "Backspace") {
    // Spec §Keyboard: Delete unlinks a SELECTED EDGE. A selected node's own
    // lineage is cut from its menu, where the verb names what it does.
    return context.selectedEdgeId === null
      ? { kind: "ignore" } : { kind: "unlink", agentId: context.selectedEdgeId };
  }
  if (event.key === "-") return { kind: "zoom", delta: -ZOOM_STEP };
  if (event.key === "+" || event.key === "=") return { kind: "zoom", delta: ZOOM_STEP };
  if (event.key === "0") return { kind: "zoom-reset" };
  const selected = context.selectedAgentId;
  if (selected === null || !order.includes(selected)) return { kind: "ignore" };
  if (event.key === "Enter") return { kind: "open", agentId: selected, newPane: event.shiftKey };
  if (event.key === "l" || event.key === "L") {
    return context.orchestratorIds.has(selected)
      ? { kind: "link", agentId: selected }
      : { kind: "refuse", message: STAR_REFUSAL };
  }
  // Every node action lives in the menu, so the menu needs a key of its own:
  // M, plus the two shortcuts the platform already spells "context menu".
  if (event.key === "m" || event.key === "M" || event.key === "ContextMenu"
    || (event.key === "F10" && event.shiftKey)) return { kind: "menu", agentId: selected };
  const dx = event.key === "ArrowLeft" ? -step : event.key === "ArrowRight" ? step : 0;
  const dy = event.key === "ArrowUp" ? -step : event.key === "ArrowDown" ? step : 0;
  if (dx === 0 && dy === 0) return { kind: "ignore" };
  return { kind: "nudge", agentId: selected, dx, dy };
}

export type CanvasMenuActionKind = "designate" | "policy" | "demote" | "pause" | "unlink"
  | "submit" | "dry-run" | "integrate" | "abandon" | "ack"
  | "stop" | "stop-all" | "open" | "remove";

export interface CanvasMenuItem {
  kind: CanvasMenuActionKind;
  label: string;
  disabled: boolean;
}

/** Only a worker stops as a worker; an orchestrator or a lone node stops as itself. */
export function canvasStopLabel(agent: CanvasAgent): string {
  return agent.role === WORKER_ROLE || agent.parentAgentId !== undefined
    ? "Stop worker" : "Stop session";
}

export function canvasMenuItems(agent: CanvasAgent,
    context: { childCount: number; blocked: boolean;
      autonomy?: OrchestratorPolicyV1["autonomy"]; coordinated?: boolean }): CanvasMenuItem[] {
  const orchestrator = agent.role === ORCHESTRATOR_ROLE;
  const items: CanvasMenuItem[] = [];
  if (!orchestrator && agent.parentAgentId === undefined) {
    items.push({ kind: "designate", label: "Make orchestrator", disabled: context.blocked });
  }
  if (orchestrator) {
    items.push({ kind: "policy", label: "Edit policy…", disabled: context.blocked });
    items.push({ kind: "demote", label: "Demote",
      disabled: context.blocked || context.childCount > 0 });
  }
  if (context.autonomy !== undefined) {
    items.push({ kind: "pause", label: context.autonomy === "active" ? "Pause" : "Resume",
      disabled: context.blocked });
  }
  if (agent.parentAgentId !== undefined) {
    items.push({ kind: "unlink", label: "Unlink", disabled: context.blocked });
  }
  // Integrate is the human's publish and is always offered here; policy.integration
  // gates only iface_integrate_worker, which lives in the main process.
  if (context.coordinated === true) {
    items.push({ kind: "submit", label: "Submit", disabled: context.blocked });
    items.push({ kind: "dry-run", label: "Dry-run verify", disabled: context.blocked });
    items.push({ kind: "integrate", label: "Integrate", disabled: context.blocked });
    items.push({ kind: "abandon", label: "Abandon task", disabled: context.blocked });
    items.push({ kind: "ack", label: "Acknowledge obligation\u2026",
      disabled: context.blocked });
  }
  items.push({ kind: "stop", label: canvasStopLabel(agent), disabled: context.blocked });
  if (orchestrator) {
    items.push({ kind: "stop-all", label: "Stop all workers", disabled: context.blocked });
  }
  items.push({ kind: "open", label: "Open", disabled: false });
  items.push({ kind: "remove", label: "Remove from canvas", disabled: false });
  return items;
}

export type CanvasLinkOutcome =
  | { kind: "link" }
  | { kind: "ignore" }
  | { kind: "refuse"; message: string };

export function canvasLinkOutcome(parentAgentId: string,
    child: CanvasAgent | undefined): CanvasLinkOutcome {
  if (child === undefined || child.agentId === parentAgentId) return { kind: "ignore" };
  if (child.role === ORCHESTRATOR_ROLE) return { kind: "refuse", message: STAR_REFUSAL };
  return { kind: "link" };
}

export function canvasOpenTarget(agent: CanvasAgent):
    { sessionKey: string } | { refusal: string } {
  return agent.sessionKey === undefined
    ? { refusal: NO_SESSION_REFUSAL } : { sessionKey: agent.sessionKey };
}

export function clampZoom(current: number, delta: number): number {
  return Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, Math.round((current + delta) * 10) / 10));
}

export function draggedPosition(origin: CanvasNodePosition, deltaX: number, deltaY: number,
    zoom: number): CanvasNodePosition {
  return { x: Math.round(origin.x + deltaX / zoom), y: Math.round(origin.y + deltaY / zoom) };
}

export function canvasWorldPoint(rect: { left: number; top: number }, clientX: number,
    clientY: number, pan: CanvasNodePosition, zoom: number): CanvasNodePosition {
  return { x: (clientX - rect.left - pan.x) / zoom, y: (clientY - rect.top - pan.y) / zoom };
}

export function refusalText(error: unknown): string {
  return error instanceof Error ? withoutIpcErrorShape(error.message) : String(error);
}

export function AgentCanvas(props: AgentCanvasProps) {
  const hintId = useId();
  const rootRef = useRef<HTMLElement | null>(null);
  const viewportRef = useRef<HTMLDivElement | null>(null);
  const [zoom, setZoom] = useState(1);
  const [pan, setPan] = useState<CanvasNodePosition>({ x: 0, y: 0 });
  const [selectedAgentId, setSelectedAgentId] = useState<string | null>(null);
  const [selectedEdgeId, setSelectedEdgeId] = useState<string | null>(null);
  const [menuAgentId, setMenuAgentId] = useState<string | null>(null);
  const [policyAgentId, setPolicyAgentId] = useState<string | null>(null);
  // The four policy vocabularies the picker owns. A dialog seeds these when it
  // opens, which is what `defaultValue` used to do for the native menus; the
  // numeric and text fields below are still read straight off the form.
  const [policyDraft, setPolicyDraft] = useState({ autonomy: "active",
    maxAccessMode: "project", integration: "orchestrator", modelCatalogue: "on-demand" });
  const [linkTargetId, setLinkTargetId] = useState("");
  const [ackAgentId, setAckAgentId] = useState<string | null>(null);
  const [linkPickerId, setLinkPickerId] = useState<string | null>(null);
  const [linkGhost, setLinkGhost] =
    useState<{ fromAgentId: string; to: CanvasNodePosition } | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [announcement, setAnnouncement] = useState("");
  const [verifying, setVerifying] = useState<CanvasVerifyClaim | null>(null);
  const [busy, setBusy] = useState(false);
  // A pointer gesture outlives the render it started in, so it reads the layout
  // and the zoom from these and never from its own closure: a snapshot that
  // lands mid-drag must not be overwritten by the state the drag began with.
  const layoutRef = useRef(props.layout);
  const zoomRef = useRef(zoom);
  layoutRef.current = props.layout;
  zoomRef.current = zoom;

  // display:none removes the offset parent, so this is the whole hidden-surface
  // guard: no pointer drag, no key handling and no layout write happens while
  // the workspace is showing another destination.
  const surfaceHidden = useCallback((): boolean =>
    rootRef.current === null || rootRef.current.offsetParent === null, []);

  const hidden = useMemo(() => new Set(hiddenNodes(props.layout, props.projectRoot)),
    [props.layout, props.projectRoot]);
  const agents = useMemo(() => (props.snapshot?.agents ?? [])
    .filter(agent => canvasProjectRoot(agent) === props.projectRoot
      && !hidden.has(agent.agentId))
    .sort((left, right) => left.agentId.localeCompare(right.agentId)),
  [hidden, props.projectRoot, props.snapshot]);
  const coordinatorById = useMemo(() => canvasCoordinators(props.snapshot), [props.snapshot]);
  const coordinatedIds = useMemo(() => new Set(agents
    .filter(agent => coordinatorById.has(agent.agentId))
    .map(agent => agent.agentId)), [agents, coordinatorById]);
  const positions = useMemo(() =>
    placeCanvasNodes(agents, props.layout.byProject[props.projectRoot]?.nodes ?? {},
      coordinatedIds),
  [agents, coordinatedIds, props.layout, props.projectRoot]);
  const policies = props.snapshot?.orchestratorPolicies;
  const disabled = busy;

  const policyById = useMemo(() =>
    new Map((policies ?? []).map(policy => [policy.agentId, policy])), [policies]);
  const childrenByParent = useMemo(() => {
    const map = new Map<string, CanvasAgent[]>();
    for (const agent of agents) {
      if (agent.parentAgentId === undefined) continue;
      const siblings = map.get(agent.parentAgentId);
      if (siblings === undefined) map.set(agent.parentAgentId, [agent]);
      else siblings.push(agent);
    }
    return map;
  }, [agents]);
  const agentsById = useMemo(
    () => new Map(agents.map(agent => [agent.agentId, agent])),
    [agents],
  );

  const policyFor = (agentId: string): OrchestratorPolicyV1 | undefined =>
    policyById.get(agentId);
  const isOrchestrator = (agent: CanvasAgent): boolean => agent.role === ORCHESTRATOR_ROLE;
  const childrenOf = (agentId: string): CanvasAgent[] =>
    childrenByParent.get(agentId) ?? NO_CANVAS_CHILDREN;
  const agentById = (agentId: string | null): CanvasAgent | undefined =>
    agentId === null ? undefined : agentsById.get(agentId);

  // Memoized: newestDelegation scans the snapshot's agents and messages per
  // edge, and the viewport re-renders on every pan and link-drag pointermove.
  const edges = useMemo(() => agents.flatMap(agent => {
    const parent = agent.parentAgentId === undefined
      ? undefined : agentsById.get(agent.parentAgentId);
    if (parent === undefined) return [];
    const from = positions[parent.agentId];
    const to = positions[agent.agentId];
    if (from === undefined || to === undefined) return [];
    // The control projection carries no message text, so the hover says
    // what the newest delegation is doing and when it was sent.
    const newest = newestDelegation(props.snapshot, agent.agentId);
    const state = deliveryEdgeState(newest);
    const title = newest === null
      ? `${agentName(parent)} → ${agentName(agent)}: ${state}`
      : `${agentName(parent)} → ${agentName(agent)}: ${newest.deliveryState}`
        + ` · ${newest.createdAt}`;
    return [<path key={agent.agentId} d={edgePath(from, to)}
      className={selectedEdgeId === agent.agentId
        ? `canvas-edge canvas-edge-${state} is-selected`
        : `canvas-edge canvas-edge-${state}`}
      onClick={() => { setSelectedEdgeId(agent.agentId); setSelectedAgentId(null); }}>
      <title>{title}</title>
    </path>];
  }), [agents, agentsById, positions, props.snapshot, selectedEdgeId]);

  const engineOptions = useMemo(() => [...new Set([
    ...agents.map(agent => agent.engineId),
    ...props.sessions.map(session => session.engine),
  ])].sort(), [agents, props.sessions]);
  const sessionByKey = useMemo(() =>
    new Map(props.sessions.map(session => [session.key, session])), [props.sessions]);
  const addableSessions = useMemo(() => {
    const onCanvasKeys = new Set(agents
      .map(agent => agent.sessionKey)
      .filter((key): key is string => key !== undefined));
    return props.sessions.filter(session => !onCanvasKeys.has(session.key));
  }, [agents, props.sessions]);
  // Memoized: AppSelect disambiguates every option carrying a meta against
  // every other option, keyed on the list's identity. Fresh lists made that
  // quadratic scan run on each pan and link-drag pointermove.
  const projectOptions = useMemo(() => [...projectRootOptions(props.projectRoots), BROWSE_OPTION],
    [props.projectRoots]);
  const sessionOptions = useMemo(() => addableSessions.map(session => ({
    value: session.key,
    label: session.title,
    ...(session.cwd === "" ? {} : { meta: session.cwd }),
  })), [addableSessions]);

  const run = useCallback((label: string, operation: () => Promise<void>,
      done?: string): void => {
    setNotice(null);
    setBusy(true);
    void operation()
      .then(() => { setAnnouncement(done ?? `${label} done.`); })
      .catch((error: unknown) => {
        const message = refusalText(error);
        setNotice(message);
        setAnnouncement(`${label} refused. ${message}`);
      })
      .finally(() => { setBusy(false); });
  }, []);

  const refuse = (message: string): void => { setNotice(message); setAnnouncement(message); };

  const verifyingNotice = canvasVerifyingNotice(props.snapshot, verifying);

  /** A standing fact while the verify runs, never the alert slot the refusal owns. */
  const integrate = (agentId: string, dryRun: boolean): void => {
    setVerifying(null);
    // The snapshot the click saw. What the notice stands on is the coordinator's
    // own state, so nothing here has to be reset by a later act.
    const at = props.snapshot;
    run(dryRun ? "Dry-run verify" : "Integrate", async () => {
      const claim = await props.onIntegrateWorker({ agentId,
        ...(dryRun ? { dryRun: true } : {}), clientRequestId: requestId() });
      setVerifying({ agentId, taskId: claim.taskId, at });
    });
  };

  const worldPoint = (clientX: number, clientY: number): CanvasNodePosition => {
    const rect = viewportRef.current?.getBoundingClientRect();
    if (rect === undefined) return { x: 0, y: 0 };
    return canvasWorldPoint(rect, clientX, clientY, pan, zoom);
  };

  const changeZoom = (delta: number): void => {
    setZoom(current => clampZoom(current, delta));
  };

  const openAgent = (agent: CanvasAgent, newPane: boolean): void => {
    const target = canvasOpenTarget(agent);
    if ("refusal" in target) { refuse(target.refusal); return; }
    props.onOpenSession(target.sessionKey, newPane);
  };

  const linkTo = (parentAgentId: string, agentId: string): void => {
    const child = agentById(agentId);
    const outcome = canvasLinkOutcome(parentAgentId, child);
    if (outcome.kind === "ignore" || child === undefined) return;
    if (outcome.kind === "refuse") { refuse(outcome.message); return; }
    run(`Link ${agentName(child)}`, () => props.onBindParent({
      agentId, parentAgentId, clientRequestId: requestId() }));
  };

  const unlink = (agentId: string): void => {
    const child = agentById(agentId);
    if (child === undefined || child.parentAgentId === undefined) return;
    run(`Unlink ${agentName(child)}`, () => props.onBindParent({
      agentId, parentAgentId: null, clientRequestId: requestId() }));
  };

  const startNodeDrag = (event: ReactPointerEvent<HTMLDivElement>, agentId: string): void => {
    if (surfaceHidden() || event.button !== 0) return;
    event.preventDefault();
    setSelectedAgentId(agentId);
    setSelectedEdgeId(null);
    setMenuAgentId(null);
    const origin = positions[agentId] ?? { x: 0, y: 0 };
    const startX = event.clientX;
    const startY = event.clientY;
    let landed = origin;
    const paint = (at: CanvasNodePosition): void => {
      const card = viewportRef.current
        ?.querySelector<HTMLElement>(`[data-agent-id="${agentId}"]`);
      if (card !== null && card !== undefined) {
        card.style.left = `${at.x}px`;
        card.style.top = `${at.y}px`;
      }
    };
    const move = (moved: PointerEvent): void => {
      landed = draggedPosition(origin, moved.clientX - startX, moved.clientY - startY,
        zoomRef.current);
      paint(landed);
    };
    const unbind = (): void => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", stop);
      window.removeEventListener("pointercancel", cancel);
    };
    const stop = (): void => {
      unbind();
      // A click is a zero-distance drag: it selects the node, it does not
      // write a position the human never chose.
      if (landed.x === origin.x && landed.y === origin.y) return;
      props.onLayoutChange(moveNode(layoutRef.current, props.projectRoot, agentId, landed));
    };
    const cancel = (): void => { unbind(); paint(origin); };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", stop, { once: true });
    window.addEventListener("pointercancel", cancel, { once: true });
  };

  const startPortDrag = (event: ReactPointerEvent<HTMLButtonElement>, agentId: string): void => {
    // Before the guards: pressing the port is never also a press on the card.
    event.stopPropagation();
    if (surfaceHidden() || event.button !== 0) return;
    event.preventDefault();
    const move = (moved: PointerEvent): void => {
      setLinkGhost({ fromAgentId: agentId, to: worldPoint(moved.clientX, moved.clientY) });
    };
    const unbind = (): void => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", stop);
      window.removeEventListener("pointercancel", cancel);
    };
    const stop = (dropped: PointerEvent): void => {
      unbind();
      setLinkGhost(null);
      const target = document.elementFromPoint(dropped.clientX, dropped.clientY)
        ?.closest<HTMLElement>("[data-agent-id]")?.dataset.agentId;
      if (target !== undefined) linkTo(agentId, target);
    };
    const cancel = (): void => { unbind(); setLinkGhost(null); };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", stop, { once: true });
    window.addEventListener("pointercancel", cancel, { once: true });
  };

  const startPan = (event: ReactPointerEvent<HTMLDivElement>): void => {
    if (surfaceHidden() || event.button !== 0
      || (event.target as HTMLElement).closest("[data-agent-id]") !== null) return;
    setSelectedAgentId(null);
    setSelectedEdgeId(null);
    setMenuAgentId(null);
    const startX = event.clientX;
    const startY = event.clientY;
    const origin = pan;
    const move = (moved: PointerEvent): void => {
      setPan({ x: origin.x + moved.clientX - startX, y: origin.y + moved.clientY - startY });
    };
    const unbind = (): void => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", stop);
      window.removeEventListener("pointercancel", cancel);
    };
    const stop = (): void => { unbind(); };
    const cancel = (): void => { unbind(); setPan(origin); };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", stop, { once: true });
    window.addEventListener("pointercancel", cancel, { once: true });
  };

  const handleKey = (event: ReactKeyboardEvent<HTMLDivElement>): void => {
    if (surfaceHidden()) return;
    // Focus inside a card belongs to that card's control: Enter and Space are
    // how a button is pressed, so the viewport's own verbs stay out of the way.
    if (event.target !== event.currentTarget
      && (event.key === "Enter" || event.key === " ")) return;
    // With the menu open, Tab belongs to the menu: it is the next focusable
    // thing after the viewport, and reaching it is the point of having opened
    // it. Escape closes the menu and hands the cycle back.
    if (menuAgentId !== null && event.key === "Tab" && !event.shiftKey) return;
    const action = canvasKeyAction(event, {
      order: agents.map(agent => agent.agentId),
      selectedAgentId,
      selectedEdgeId,
      orchestratorIds: new Set(agents.filter(isOrchestrator).map(agent => agent.agentId)),
    });
    // "leave" is the keyboard-trap escape hatch: no preventDefault, so the
    // browser moves focus out of the viewport as it normally would.
    if (action.kind === "leave") { setSelectedAgentId(null); return; }
    if (action.kind === "ignore") return;
    event.preventDefault();
    switch (action.kind) {
      case "select": setSelectedAgentId(action.agentId); setSelectedEdgeId(null); return;
      case "clear":
        setSelectedAgentId(null); setSelectedEdgeId(null); setMenuAgentId(null);
        setLinkPickerId(null); setPolicyAgentId(null); setAckAgentId(null); setNotice(null);
        return;
      case "unlink": unlink(action.agentId); return;
      case "zoom": changeZoom(action.delta); return;
      case "zoom-reset": setZoom(1); setPan({ x: 0, y: 0 }); return;
      case "link": setLinkTargetId(""); setLinkPickerId(action.agentId); return;
      case "menu": setMenuAgentId(action.agentId); setSelectedEdgeId(null); return;
      case "refuse": refuse(action.message); return;
      case "open": {
        const agent = agentById(action.agentId);
        if (agent !== undefined) openAgent(agent, action.newPane);
        return;
      }
      default: {
        const at = positions[action.agentId] ?? { x: 0, y: 0 };
        props.onLayoutChange(moveNode(props.layout, props.projectRoot, action.agentId,
          { x: at.x + action.dx, y: at.y + action.dy }));
      }
    }
  };

  const openMenu = (event: ReactMouseEvent<HTMLDivElement>, agentId: string): void => {
    event.preventDefault();
    setSelectedAgentId(agentId);
    setMenuAgentId(agentId);
  };

  const designate = (agentId: string, patch: Partial<OrchestratorPolicySetRequestV1>): void => {
    run("Policy update", () => props.onSetPolicy({
      agentId, ...patch, clientRequestId: requestId() }));
    setMenuAgentId(null);
  };

  const togglePause = (agent: CanvasAgent, policy: OrchestratorPolicyV1): void => {
    const pausing = policy.autonomy === "active";
    const name = agentName(agent);
    setMenuAgentId(null);
    run(pausing ? `Pause ${name}` : `Resume ${name}`,
      () => props.onSetPolicy({ agentId: agent.agentId,
        autonomy: pausing ? "paused" : "active", clientRequestId: requestId() }),
      pausing ? `Paused ${name}` : `Resumed ${name}`);
  };

  // Renderer-local and reversible: the node leaves this canvas, and nothing
  // about the agent changes. Cutting lineage is a host mutation this label does
  // not promise, so a linked node is refused instead of quietly unlinked.
  const removeFromCanvas = (agent: CanvasAgent): void => {
    setMenuAgentId(null);
    const coordination = coordinatorById.get(agent.agentId);
    if (coordination !== undefined && coordinatedTaskActive(coordination)) {
      refuse(COORDINATION_REFUSAL); return;
    }
    if (agent.parentAgentId !== undefined) { refuse(LINKED_REFUSAL); return; }
    // A worker exists because this orchestrator made it, so removing the lead
    // takes them with it. Leaving them behind left the human unlinking each one
    // by hand before it would let them go — every node refusing on the same
    // link the lead they just removed had put there.
    const children = childrenOf(agent.agentId);
    // One holding a live coordinator task is the exception: abandoning it would
    // strand a wcp lease nothing is left to release.
    const held = children.find(child => {
      const task = coordinatorById.get(child.agentId);
      return task !== undefined && coordinatedTaskActive(task);
    });
    if (held !== undefined) {
      refuse(`${agentName(held)} still holds its coordinator task. ${COORDINATION_REFUSAL}`);
      return;
    }
    let layout = props.layout;
    for (const child of children) {
      layout = hideNode(layout, props.projectRoot, child.agentId);
    }
    props.onLayoutChange(hideNode(layout, props.projectRoot, agent.agentId));
    setSelectedAgentId(current =>
      current === agent.agentId || children.some(child => child.agentId === current)
        ? null : current);
    if (children.length > 0) {
      // The records outlive the canvas, so the lineage is severed too: a worker
      // whose lead is gone would otherwise refuse every later removal on a link
      // to a node that is no longer drawn.
      run(`Remove ${agentName(agent)} and its workers`, async () => {
        for (const child of children) {
          await props.onBindParent({ agentId: child.agentId, parentAgentId: null,
            clientRequestId: requestId() });
        }
        await props.onClearPolicy({ agentId: agent.agentId,
          clientRequestId: requestId() });
      }, `${agentName(agent)} and ${children.length} worker`
        + `${children.length === 1 ? "" : "s"} removed from the canvas. `
        + "Their sessions are untouched.");
      return;
    }
    setAnnouncement(`${agentName(agent)} removed from the canvas. Its session is untouched.`);
  };

  const runMenuAction = (kind: CanvasMenuActionKind, agent: CanvasAgent): void => {
    switch (kind) {
      case "designate": designate(agent.agentId, {}); return;
      case "policy": {
        const current = policyFor(agent.agentId);
        setPolicyDraft({
          autonomy: current?.autonomy ?? "active",
          maxAccessMode: current?.maxAccessMode ?? "project",
          integration: current?.integration ?? "orchestrator",
          modelCatalogue: current?.modelCatalogue ?? "on-demand",
        });
        setPolicyAgentId(agent.agentId);
        setMenuAgentId(null);
        return;
      }
      case "pause": {
        const policy = policyFor(agent.agentId);
        if (policy !== undefined) togglePause(agent, policy);
        return;
      }
      case "demote":
        run("Demote", () => props.onClearPolicy({
          agentId: agent.agentId, clientRequestId: requestId() }));
        setMenuAgentId(null);
        return;
      case "unlink": unlink(agent.agentId); setMenuAgentId(null); return;
      case "submit":
        run("Submit", () => props.onSubmitWorker({
          agentId: agent.agentId, clientRequestId: requestId() }));
        setMenuAgentId(null);
        return;
      case "dry-run":
        integrate(agent.agentId, true);
        setMenuAgentId(null);
        return;
      case "integrate":
        integrate(agent.agentId, false);
        setMenuAgentId(null);
        return;
      case "abandon":
        run("Abandon task", () => props.onAbandonTask({
          agentId: agent.agentId, clientRequestId: requestId() }));
        setMenuAgentId(null);
        return;
      case "ack": setAckAgentId(agent.agentId); setMenuAgentId(null); return;
      case "stop":
        run(canvasStopLabel(agent), () => props.onStopAgent({ agentId: agent.agentId }));
        setMenuAgentId(null);
        return;
      case "stop-all": {
        const children = childrenOf(agent.agentId);
        run("Stop all workers", async () => {
          for (const child of children) await props.onStopAgent({ agentId: child.agentId });
        });
        setMenuAgentId(null);
        return;
      }
      case "open": openAgent(agent, false); setMenuAgentId(null); return;
      default: removeFromCanvas(agent);
    }
  };

  const menuAgent = agentById(menuAgentId);
  const ackAgent = agentById(ackAgentId);
  const ackCoordinator = ackAgent === undefined
    ? undefined : coordinatorById.get(ackAgent.agentId);
  const policyAgent = agentById(policyAgentId);
  const policyCurrent = policyAgent === undefined ? undefined : policyFor(policyAgent.agentId);
  const linkSource = agentById(linkPickerId);
  const alertMessage = notice ?? props.error ?? null;
  const clock = Date.now();

  return <section className="canvas" ref={rootRef} aria-label="Coordination canvas"
    aria-busy={busy || undefined}>
    <style>{AGENT_CANVAS_CSS}</style>
    <header className="canvas-heading">
      <h2>Coordination canvas</h2>
      <p>Arrange coding sessions and connect workers to an orchestrator.</p>
    </header>
    <div className="canvas-strip">
      <div className="canvas-project-controls">
        {/* Every cwd the app has ever seen is not a menu; it is a haystack. The
            filter and the folder dialog are what make a long list usable. */}
        <FormSelect label="Project" value={props.projectRoot}
          fieldClassName="canvas-field canvas-project"
          placeholder={props.projectRoots.length === 0 ? "No project" : "Choose a project…"}
          options={projectOptions}
          onChange={value => {
            if (value !== BROWSE_ROW) { props.onSelectProjectRoot(value); return; }
            void props.onBrowseProjectRoot().then(root => {
              if (root !== null && root !== "") props.onSelectProjectRoot(root);
            }, () => undefined);
          }} />
        <FormSelect fieldClassName="canvas-field" label="Add session" value="" disabled={disabled}
          placeholder="Choose a session…"
          options={sessionOptions}
          onChange={key => {
            if (key === "") return;
            const session = props.sessions.find(item => item.key === key);
            // Putting a session back on the canvas undoes an earlier Remove.
            const removed = (props.snapshot?.agents ?? []).find(agent =>
              agent.sessionKey === key && hidden.has(agent.agentId));
            if (removed !== undefined) {
              props.onLayoutChange(unhideNode(props.layout, props.projectRoot, removed.agentId));
            }
            run(`Add ${session?.title ?? key}`, () => props.onRegisterSession({
              sessionKey: key,
              ...(session === undefined ? {} : { name: session.title }),
              clientRequestId: requestId(),
            }));
          }} />
      </div>
      <div className="canvas-strip-actions">
        <button className="btn" type="button" onClick={() => {
          props.onLayoutChange(clearNodes(props.layout, props.projectRoot));
          setAnnouncement("Canvas auto-arranged.");
        }}>Auto-arrange</button>
        <button type="button" aria-label="Zoom out"
          onClick={() => { changeZoom(-ZOOM_STEP); }}>−</button>
        <button type="button" aria-label="Zoom in"
          onClick={() => { changeZoom(ZOOM_STEP); }}>+</button>
        <button type="button" aria-label="Reset zoom"
          onClick={() => { setZoom(1); setPan({ x: 0, y: 0 }); }}>{Math.round(zoom * 100)}%</button>
      </div>
      <details className="canvas-worker-section">
        <summary>New worker</summary>
        <NewWorkerForm engines={engineOptions} models={props.models} disabled={disabled}
          onCreate={fields => {
            const parent = agents.find(agent => agent.agentId === selectedAgentId
              && agent.role === ORCHESTRATOR_ROLE);
            run(`New worker ${fields.name}`, () => props.onCreateWorker({
              engineId: fields.engineId,
              name: fields.name,
              prompt: fields.brief,
              canonicalCwd: props.projectRoot,
              accessMode: fields.accessMode,
              ...(fields.model === undefined ? {} : { model: fields.model }),
              ...(fields.effort === undefined ? {} : { effort: fields.effort }),
              ...(parent === undefined ? {} : { parentAgentId: parent.agentId, role: WORKER_ROLE }),
              clientRequestId: requestId(),
            }));
          }} />
      </details>
    </div>

    {/* wcp facts, not alerts: they stand while the state stands, and they never
        take the single alert slot the live refusal owns. */}
    {props.coordination !== null && props.coordination.publishedUncommittedPaths.length > 0
      && <p className="canvas-coordination-notice">
        {`${props.coordination.publishedUncommittedPaths.length} published change`
          + `${props.coordination.publishedUncommittedPaths.length === 1 ? "" : "s"}`
          + " awaiting your commit."}</p>}
    {props.coordination !== null && props.coordination.activeTasks > 0
      && <p className="canvas-coordination-notice">
        {`canonical is integrator-only while ${props.coordination.activeTasks} task`
          + `${props.coordination.activeTasks === 1 ? " is" : "s are"} active. `
          + "WCP_ROLE=integrator is the documented override."}</p>}

    {verifyingNotice !== null
      && <p className="canvas-coordination-notice">{verifyingNotice}</p>}

    {/* One alert at a time, and the transient refusal wins: a standing load
        error must not stand in front of the answer to what was just tried. */}
    {alertMessage !== null
      && <p className="canvas-notice" role="alert">{alertMessage}</p>}

    <div className="canvas-viewport" ref={viewportRef}
      role="application" aria-describedby={hintId} tabIndex={0}
      aria-activedescendant={selectedAgentId === null
        ? undefined : `canvas-node-${selectedAgentId}`}
      onKeyDown={handleKey} onPointerDown={startPan}>
      {agents.length === 0 && <p className="canvas-empty">{EMPTY_HINT}</p>}
      <div className="canvas-world"
        style={{ transform: `translate(${pan.x}px, ${pan.y}px) scale(${zoom})` }}>
        <svg className="canvas-edges" aria-hidden="true">
          {edges}
          {linkGhost !== null && positions[linkGhost.fromAgentId] !== undefined
            && <path className="canvas-edge canvas-edge-ghost"
              d={edgePath(positions[linkGhost.fromAgentId]!,
                { x: linkGhost.to.x - CANVAS_NODE_SIZE.w / 2, y: linkGhost.to.y })} />}
        </svg>
        {agents.map(agent => {
          const at = positions[agent.agentId] ?? { x: 0, y: 0 };
          const policy = policyFor(agent.agentId);
          const coordination = coordinatorById.get(agent.agentId);
          const children = childrenOf(agent.agentId);
          const running = children.filter(child =>
            child.state === "working" || child.state === "starting").length;
          return <div key={agent.agentId} id={`canvas-node-${agent.agentId}`}
            data-agent-id={agent.agentId}
            className={selectedAgentId === agent.agentId
              ? "canvas-node is-selected" : "canvas-node"}
            style={{ left: at.x, top: at.y,
              width: CANVAS_NODE_SIZE.w, minHeight: CANVAS_NODE_SIZE.h }}
            role="group" aria-label={coordination === undefined
              ? `${agentName(agent)}, ${providerLabel(agent.engineId)}, ${orchestrationStatusWord(agent.state)}`
              : `${agentName(agent)}, ${providerLabel(agent.engineId)}, `
                + `${orchestrationStatusWord(agent.state)}, `
                + coordinationLabel(coordination, clock)}
            onPointerDown={event => { startNodeDrag(event, agent.agentId); }}
            onDoubleClick={() => { openAgent(agent, false); }}
            onContextMenu={event => { openMenu(event, agent.agentId); }}>
            <span className="canvas-node-mark">
              <ProviderLogo engine={agent.engineId} label={agentName(agent)} size={16} />
            </span>
            <span className="canvas-node-name">{agentName(agent)}</span>
            <span className="canvas-node-engine">
              {engineLine(agent, sessionByKey.get(agent.sessionKey ?? ""))}</span>
            <span className={`canvas-status canvas-status-${statusTone(agent.state)}`}>
              {orchestrationStatusWord(agent.state)}</span>
            <span className="canvas-role">{isOrchestrator(agent)
              ? "Orchestrator" : agent.parentAgentId === undefined ? "Session" : "Worker"}</span>
            <span className="canvas-access">{agent.scope.accessMode}</span>
            {coordination !== undefined && <span className="canvas-coordination">
              {`task ${coordination.taskId} \u00b7 ${scopeSummary(coordination.writeScope)}`}
            </span>}
            {coordination !== undefined && <span className="canvas-coordination">
              {`${coordination.taskState ?? "claimed"}`
                + `${coordination.submissionId === undefined
                  ? "" : ` \u00b7 submission ${coordination.submissionId}`}`
                + ` \u00b7 ${leaseCountdown(coordination.leaseExpiresAt, clock)}`}</span>}
            {coordination !== undefined && <span className={coordination.fence === "unverified"
              ? "canvas-fence badge badge-quiet is-unverified"
              : "canvas-fence badge badge-quiet"}>
              {`fence ${coordination.fence}`}</span>}
            {policy !== undefined && <span className="canvas-counters">
              {`workers ${children.length}/${policy.maxWorkers}`
                + ` · running ${running}/${policy.maxConcurrent}`
                + ` · delegations ${policy.delegationsUsed}/${policy.maxDelegations}`}</span>}
            {policy !== undefined && <button type="button" className="canvas-node-btn"
              disabled={disabled}
              onPointerDown={event => { event.stopPropagation(); }}
              onClick={() => { togglePause(agent, policy); }}>
              {policy.autonomy === "active" ? "Pause" : "Resume"}</button>}
            {isOrchestrator(agent) && <button type="button" className="canvas-port"
              aria-label={`Link ${agentName(agent)} to a worker`} disabled={disabled}
              onPointerDown={event => { startPortDrag(event, agent.agentId); }} />}
          </div>;
        })}
      </div>
    </div>

    {menuAgent !== undefined && <ul className="canvas-menu is-floating"
      aria-label={`Actions for ${agentName(menuAgent)}`}>
      {canvasMenuItems(menuAgent, { childCount: childrenOf(menuAgent.agentId).length,
        blocked: disabled, autonomy: policyFor(menuAgent.agentId)?.autonomy,
        coordinated: coordinatorById.has(menuAgent.agentId),
      }).map(item => <li key={item.kind}>
        <button type="button" disabled={item.disabled}
          onClick={() => { runMenuAction(item.kind, menuAgent); }}>{item.label}</button></li>)}
    </ul>}

    {linkSource !== undefined && (() => {
      const targets = agents.filter(agent => agent.agentId !== linkSource.agentId
        && !isOrchestrator(agent));
      // An unopened picker has no selection of its own yet, so the first row is
      // what Link would send — exactly what the native menu showed.
      const target = targets.some(agent => agent.agentId === linkTargetId)
        ? linkTargetId : targets[0]?.agentId ?? "";
      return <form className="canvas-link-picker is-floating" aria-label="Link to"
        onSubmit={event => {
          event.preventDefault();
          setLinkPickerId(null);
          if (target !== "") linkTo(linkSource.agentId, target);
        }}>
        <FormSelect fieldClassName="canvas-field" label="Link to" value={target} disabled={disabled}
          placeholder="No linkable agent"
          options={targets.map(agent => ({ value: agent.agentId, label: agentName(agent),
            meta: providerLabel(agent.engineId) }))}
          onChange={setLinkTargetId} />
        <button type="submit" disabled={disabled || target === ""}>Link</button>
        <button type="button" onClick={() => { setLinkPickerId(null); }}>Cancel</button>
      </form>;
    })()}

    {policyAgent !== undefined && <form className="canvas-policy is-floating" aria-label="Edit policy"
      onSubmit={event => {
        event.preventDefault();
        const data = new FormData(event.currentTarget);
        const allowlist = parseModelAllowlist(String(data.get("modelAllowlist") ?? ""));
        if ("refusal" in allowlist) { refuse(allowlist.refusal); return; }
        const notes = String(data.get("modelNotes") ?? "").trim();
        const notesRefusal = modelNotesRefusal(notes);
        if (notesRefusal !== null) { refuse(notesRefusal); return; }
        setPolicyAgentId(null);
        designate(policyAgent.agentId, {
          autonomy: String(data.get("autonomy")) as OrchestratorPolicySetRequestV1["autonomy"],
          maxAccessMode:
            String(data.get("maxAccessMode")) as OrchestratorPolicySetRequestV1["maxAccessMode"],
          maxWorkers: Number(data.get("maxWorkers")),
          maxConcurrent: Number(data.get("maxConcurrent")),
          maxDelegations: Number(data.get("maxDelegations")),
          wallClockMs: Number(data.get("wallClockHours")) * 3_600_000,
          integration:
            String(data.get("integration")) as OrchestratorPolicySetRequestV1["integration"],
          coordinationRequired: data.get("coordinationRequired") === "on",
          allowUnverifiedFence: data.get("allowUnverifiedFence") === "on",
          modelCatalogue: String(data.get("modelCatalogue")) as
            OrchestratorPolicySetRequestV1["modelCatalogue"],
          // An empty list is the explicit "unrestricted" the human just chose.
          modelAllowlist: allowlist.entries,
          // `""` is the clear signal the host reads, symmetric with that empty list.
          modelNotes: notes,
        });
      }}>
      <FormSelect fieldClassName="canvas-field" label="Autonomy" name="autonomy"
        value={policyDraft.autonomy}
        options={[{ value: "active", label: "Active" }, { value: "paused", label: "Paused" }]}
        onChange={value => { setPolicyDraft(draft => ({ ...draft, autonomy: value })); }} />
      <FormSelect fieldClassName="canvas-field" label="Access ceiling" name="maxAccessMode"
        value={policyDraft.maxAccessMode} options={ACCESS_ROWS}
        onChange={value => { setPolicyDraft(draft => ({ ...draft, maxAccessMode: value })); }} />
      <label><span>Max workers</span><input name="maxWorkers" type="number" min={1} max={32}
        defaultValue={policyCurrent?.maxWorkers ?? 8} /></label>
      <label><span>Max running</span><input name="maxConcurrent" type="number" min={1}
        max={ORCHESTRATION_LIMITS.maxRunningTargets}
        defaultValue={policyCurrent?.maxConcurrent ?? 4} /></label>
      <label><span>Max delegations</span><input name="maxDelegations" type="number" min={1}
        max={1_024} defaultValue={policyCurrent?.maxDelegations ?? 64} /></label>
      <label><span>Window (hours)</span><input name="wallClockHours" type="number" min={1}
        max={24} defaultValue={Math.round(
          (policyCurrent?.wallClockMs ?? 14_400_000) / 3_600_000)} /></label>
      <FormSelect fieldClassName="canvas-field" label="Integration" name="integration"
        value={policyDraft.integration}
        options={plainRows([...INTEGRATION_MODES])}
        onChange={value => { setPolicyDraft(draft => ({ ...draft, integration: value })); }} />
      <label className="canvas-policy-check">
        <input name="coordinationRequired" type="checkbox"
          defaultChecked={policyCurrent?.coordinationRequired ?? false} />
        <span>Coordination required</span></label>
      <label className="canvas-policy-check">
        <input name="allowUnverifiedFence" type="checkbox"
          defaultChecked={policyCurrent?.allowUnverifiedFence ?? false} />
        <span>Allow unverified fence</span></label>
      <FormSelect fieldClassName="canvas-field" label="Model catalogue" name="modelCatalogue"
        value={policyDraft.modelCatalogue} options={plainRows([...CATALOGUE_MODES])}
        onChange={value => { setPolicyDraft(draft => ({ ...draft, modelCatalogue: value })); }} />
      <label><span>Model allowlist</span><textarea name="modelAllowlist" rows={3}
        aria-describedby={props.models === null ? undefined : `${hintId}-catalogue`}
        defaultValue={(policyCurrent?.modelAllowlist ?? []).join("\n")} /></label>
      {props.models !== null && <p id={`${hintId}-catalogue`} className="canvas-policy-hint">
        {modelHint(props.models)}</p>}
      <label><span>Routing notes</span><textarea name="modelNotes" rows={3}
        defaultValue={policyCurrent?.modelNotes ?? ""} /></label>
      <button type="submit" disabled={disabled}>Save policy</button>
      <button type="button" onClick={() => { setPolicyAgentId(null); }}>Cancel</button>
    </form>}

    {ackAgent !== undefined && ackCoordinator !== undefined
      && <form className="canvas-ack is-floating" aria-label="Acknowledge obligation"
      onSubmit={event => {
        event.preventDefault();
        const keep = String(new FormData(event.currentTarget).get("keep") ?? "").trim();
        if (keep === "") return;
        setAckAgentId(null);
        run("Acknowledge obligation", () => props.onAckObligation({
          agentId: ackAgent.agentId, keep, clientRequestId: requestId() }));
      }}>
      {/* wcp reports obligations as paths; the write scope is a pattern list and
          typing one of those back would match nothing. */}
      {(ackCoordinator.pendingObligations ?? []).length === 0
        ? <p className="canvas-ack-pending">No pending obligations on this task.</p>
        : <ul className="canvas-ack-pending">
          {(ackCoordinator.pendingObligations ?? []).map(path =>
            <li key={path}>{path}</li>)}
        </ul>}
      <label><span>Keep path</span><input name="keep" list={`${hintId}-pending`} required
        maxLength={256} disabled={disabled} /></label>
      <datalist id={`${hintId}-pending`}>
        {(ackCoordinator.pendingObligations ?? []).map(path =>
          <option key={path} value={path} />)}
      </datalist>
      <button type="submit" disabled={disabled}>Acknowledge</button>
      <button type="button" onClick={() => { setAckAgentId(null); }}>Cancel</button>
    </form>}

    <p id={hintId} className="canvas-hint">{USAGE_HINT}</p>
    <p className="canvas-live" role="status" aria-live="polite">{announcement}</p>
  </section>;
}

export const AGENT_CANVAS_CSS = `
.canvas { display:flex; flex-direction:column; width:100%; height:100%; min-width:0;
  min-height:0; color:var(--fg); background:var(--editor-bg); font-family:var(--font-ui); }
.canvas *, .canvas *::before, .canvas *::after { box-sizing:border-box; }
.canvas button:not(.btn), .canvas input, .canvas select { color:inherit; font:inherit; }
.canvas button { cursor:pointer; }
/* Disabled is a colour, not a dimmer: an opacity fades the control's border and
   its background along with its label, so a refused control reads as a
   half-painted one. */
.canvas button:disabled, .canvas input:disabled, .canvas select:disabled { cursor:default;
  color:var(--fg-faint); }
.canvas button:focus-visible, .canvas input:focus-visible, .canvas select:focus-visible,
.canvas-viewport:focus-visible { outline:var(--focus-ring-w) solid var(--focus); outline-offset:var(--focus-ring-offset); }
.canvas-heading { flex:none; padding:18px 20px 12px; background:var(--chrome-bg); }
.canvas-heading h2 { margin:0; font-size:var(--text-h2); font-weight:var(--wght-strong); }
.canvas-heading p { margin:5px 0 0; color:var(--fg-muted); font-size:var(--text-ui); }
.canvas-strip { display:flex; flex:none; flex-wrap:wrap; align-items:flex-end; gap:12px; padding:0 20px 16px;
  border-bottom:var(--border-w) solid var(--border); background:var(--chrome-bg); }
.canvas-field, .canvas-new-worker label { display:flex; flex-direction:column; gap:4px;
  min-width:0; color:var(--fg-dim); font-size:var(--text-meta); }
.canvas-new-worker input {
  height:var(--control-h); padding:0 7px; border:var(--border-w) solid var(--border-strong);
  border-radius:var(--radius-control); background:var(--input-bg); color:var(--fg); }
/* The app's own picker, given the field shape its neighbours have: AppSelect is
   chrome-transparent by default because it usually sits in a toolbar, and next
   to a bordered text input that reads as a missing control. */
.canvas-field .app-select { width:100%; }
.canvas-field .app-select-trigger { width:100%; justify-content:space-between;
  border-color:var(--border-strong); background:var(--input-bg); color:var(--fg);
  font-size:var(--text-ui); }
.canvas-field .app-select-trigger:hover:not(:disabled),
.canvas-field .app-select-trigger[aria-expanded="true"] { background:var(--input-bg);
  border-color:var(--focus); }
.canvas-project-controls { display:flex; flex:1 1 340px; flex-wrap:wrap; gap:12px; max-width:640px; }
.canvas-project-controls > * { flex:1 1 160px; }
.canvas-new-worker { display:grid; width:100%; max-width:920px;
  grid-template-columns:repeat(4, minmax(0,1fr)); gap:12px; align-items:end;
  padding-top:14px; border-top:var(--border-w) solid var(--border); }
.canvas-worker-section { width:100%; min-width:0; }
.canvas-worker-section > summary { width:fit-content; padding:4px 0; cursor:pointer; color:var(--fg); font-size:var(--text-ui); font-weight:var(--wght-strong); }
.canvas-worker-section > summary:focus-visible { outline:var(--focus-ring-w) solid var(--focus); outline-offset:var(--focus-ring-offset); }
.canvas-create-worker { grid-column:1 / -1; justify-self:end; }
.canvas-new-worker .canvas-wide { grid-column:1 / -1; }
.canvas-strip-actions button { height:var(--control-h); padding:0 10px;
  border:var(--border-w) solid var(--border-strong); border-radius:var(--radius-control);
  background:var(--surface); color:var(--fg); }
.canvas-strip-actions { display:flex; align-items:center; gap:6px; margin-left:auto; }
.canvas-notice { margin:0; padding:8px 12px; border-bottom:var(--border-w) solid var(--border);
  background:var(--danger-bg); color:var(--danger-fg); font-size:var(--text-ui); }
.canvas-viewport { position:relative; flex:1; min-height:160px; overflow:hidden; touch-action:none;
  background:var(--editor-bg); }
.canvas-empty { position:absolute; inset:0; display:flex; align-items:center;
  justify-content:center; margin:0; padding:0 24px; color:var(--fg-dim);
  font-size:var(--text-body); text-align:center; }
.canvas-world { position:absolute; inset:0; transform-origin:0 0; }
.canvas-edges { position:absolute; inset:0; width:100%; height:100%; overflow:visible;
  pointer-events:none; }
.canvas-edge { fill:none; stroke-width:2; stroke:var(--border-strong); pointer-events:stroke; }
.canvas-edge-idle { stroke:var(--border-strong); }
/* A settled lineage is history, so it is quiet; only in-flight work is live. */
.canvas-edge-settled { stroke:var(--fg-faint); }
/* In flight is a live lineage, not an agent blocked on you, so it takes the
   working colour and says "in transit" with the moving dash instead of amber. */
.canvas-edge-inflight { stroke:var(--status-working); stroke-dasharray:6 5;
  animation:canvas-edge-flow 900ms steps(11) infinite; animation-play-state:var(--ambient-play-state); }
.canvas-edge-failed { stroke:var(--danger-fg); }
.canvas-edge-unknown { stroke:var(--fg-muted); stroke-dasharray:2 6; }
.canvas-edge-ghost { stroke:var(--accent); stroke-dasharray:4 4; }
.canvas-edge.is-selected { stroke-width:3; }
@keyframes canvas-edge-flow { to { stroke-dashoffset:-22; } }
@media (prefers-reduced-motion: reduce) { .canvas-edge-inflight { animation:none; } }
.canvas-node { position:absolute; display:grid;
  grid-template-columns:16px minmax(0,1fr) auto; gap:2px 7px; align-content:start;
  padding:8px 9px; border:var(--border-w) solid var(--border-strong);
  border-radius:var(--radius-container); background:var(--surface);
  box-shadow:var(--composer-lift); font-size:var(--text-meta); }
.canvas-node.is-selected { border-color:var(--selection-border);
  box-shadow:inset 0 0 0 1px var(--accent); }
.canvas-node-mark { display:inline-flex; width:16px; height:16px; }
.canvas-node-name { min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap;
  color:var(--fg-strong); font-size:var(--text-ui); font-weight:var(--wght-medium); }
.canvas-node-engine, .canvas-access { grid-column:2 / -1; color:var(--fg-dim); }
.canvas-status { grid-column:3; justify-self:end; }
.canvas-status-working { color:var(--status-working); }
.canvas-status-waiting { color:var(--status-waiting); }
.canvas-status-failed { color:var(--status-failed); }
/* Idle is the one status with no text tier: --status-idle answers to WCAG
   1.4.11's 3:1 graphical floor and falls under 4.5:1 as text in eight shipped
   themes (3.69 in solarized-dark), so the pill takes the tier theme.css and
   theme-accessibility.test.ts route idle text to. */
.canvas-status-idle { color:var(--fg-dim); }
.canvas-role { grid-column:1 / 3; color:var(--fg-muted); }
.canvas-counters, .canvas-coordination { grid-column:1 / -1; color:var(--fg-dim);
  overflow-wrap:anywhere; }
/* Placement only: the capsule itself is theme.css's .badge-quiet, and the
   unverified fence swaps that rule's ring and text for the danger tier. */
.canvas-fence { grid-column:1 / -1; justify-self:start; }
.canvas-fence.is-unverified { box-shadow:inset 0 0 0 var(--border-w) var(--danger-fg);
  color:var(--danger-fg); }
.canvas-coordination-notice { display:flex; flex-wrap:wrap; align-items:center; gap:8px;
  margin:0; padding:6px 12px; border-bottom:var(--border-w) solid var(--border);
  background:var(--chrome-bg); color:var(--fg-dim); font-size:var(--text-meta); }
.canvas-coordination-notice button { height:24px; padding:0 8px;
  border:var(--border-w) solid var(--border-strong); border-radius:var(--radius-control);
  background:var(--surface); color:var(--fg); }
.canvas-node-btn { grid-column:1 / -1; justify-self:start; height:24px; padding:0 8px;
  border:var(--border-w) solid var(--border-strong); border-radius:var(--radius-control);
  background:var(--surface-raised); color:var(--fg); }
.canvas-port { position:absolute; left:50%; bottom:-7px; width:14px; height:14px;
  margin-left:-7px; padding:0; border:var(--border-w) solid var(--border-strong);
  border-radius:var(--radius-pill); background:var(--accent); cursor:pointer; }
.canvas-port:hover:not(:disabled) { transform:scale(1.15); }
.canvas-policy-check { flex-direction:row; align-items:center; gap:6px; }
.canvas-policy-hint { margin:0; max-width:260px; color:var(--fg-dim);
  font-size:var(--text-meta); overflow-wrap:anywhere; }
.canvas-policy textarea, .canvas-ack input { padding:4px 7px;
  border:var(--border-w) solid var(--border-strong); border-radius:var(--radius-control);
  background:var(--input-bg); color:var(--fg); }
.canvas-ack label { display:flex; flex-direction:column; gap:4px; color:var(--fg-dim);
  font-size:var(--text-meta); }
.canvas-ack-pending { margin:0; padding:0; max-width:260px; max-height:120px; overflow-y:auto;
  list-style:none; color:var(--fg-dim); font-size:var(--text-meta); overflow-wrap:anywhere; }
.canvas-ack button { height:var(--control-h); padding:0 10px;
  border:var(--border-w) solid var(--border-strong); border-radius:var(--radius-control);
  background:var(--surface); color:var(--fg); }
.canvas-ack button:hover:not(:disabled), .canvas-link-picker button:hover:not(:disabled),
.canvas-policy button:hover:not(:disabled), .canvas-coordination-notice button:hover:not(:disabled),
.canvas-node-btn:hover:not(:disabled) { background:var(--row-hover); color:var(--fg-strong); }
.canvas-menu, .canvas-link-picker, .canvas-policy, .canvas-ack { position:absolute; right:16px;
  bottom:52px; max-height:calc(100% - 68px); overflow-y:auto;
  z-index:2; display:flex; flex-direction:column; gap:5px; margin:0; padding:8px;
  list-style:none; border:var(--border-w) solid var(--border-strong);
  border-radius:var(--radius-overlay); background:var(--menu-bg);
  box-shadow:var(--overlay-shadow); font-size:var(--text-ui); }
.canvas-menu button { width:100%; padding:5px 8px; border:none; border-radius:var(--radius-control);
  background:transparent; color:var(--fg); text-align:left; }
.canvas-menu button:hover:not(:disabled) { background:var(--row-hover); }
.canvas-link-picker label, .canvas-policy label { display:flex; flex-direction:column; gap:4px;
  color:var(--fg-dim); font-size:var(--text-meta); }
.canvas-policy input { height:var(--control-h);
  padding:0 7px; border:var(--border-w) solid var(--border-strong);
  border-radius:var(--radius-control); background:var(--input-bg); color:var(--fg); }
.canvas-link-picker .canvas-field, .canvas-policy .canvas-field { min-width:180px; }
.canvas-link-picker button, .canvas-policy button { height:var(--control-h); padding:0 10px;
  border:var(--border-w) solid var(--border-strong); border-radius:var(--radius-control);
  background:var(--surface); color:var(--fg); }
.canvas-hint { margin:0; padding:6px 12px; border-top:var(--border-w) solid var(--border);
  color:var(--fg-dim); font-size:var(--text-meta); }
.canvas-live { position:absolute; width:1px; height:1px; margin:-1px; padding:0; overflow:hidden;
  clip-path:inset(50%); white-space:nowrap; }
@media (max-width:900px) { .canvas-new-worker { grid-template-columns:repeat(2, minmax(0,1fr)); }
  .canvas-strip-actions { margin-left:0; } }
@media (max-width:460px) { .canvas-new-worker { grid-template-columns:1fr; } }
`;
