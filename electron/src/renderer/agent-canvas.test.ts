import { existsSync, readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import {
  ORCHESTRATOR_POLICY_DEFAULTS,
  type AgentNodeV1, type OrchestratorPolicyV1, type WorktreeCoordinatorV1,
  type WorktreeLeaseV1,
} from "../shared/orchestration";
import type { OrchestrationSnapshotDto } from "../shared/ipc";
import type { SessionSummary } from "../shared/engine";
import { canvasSessionRows } from "./app-session-projection";
import {
  AgentCanvas, canvasCoordinator, canvasKeyAction, canvasLinkOutcome, canvasMenuItems,
  canvasOpenTarget, canvasVerifyingNotice, canvasWorldPoint, clampZoom,
  coordinatedTaskActive, coordinationLabel,
  draggedPosition, engineLine, leaseCountdown, modelHint, modelNotesRefusal,
  parseModelAllowlist, refusalText, scopeSummary,
  type AgentCanvasProps, type CanvasKeyContext,
} from "./components/AgentCanvas";
import { EMPTY_CANVAS_LAYOUT } from "./canvas-layout";

const source = readFileSync(new URL("./components/AgentCanvas.tsx", import.meta.url), "utf8");

const now = "2026-08-25T12:00:00.000Z";
const digest = "a".repeat(64);
const scope = { canonicalCwd: "/work/interface", canonicalProjectRoot: "/work/interface",
  accessMode: "isolated", scopeDigest: digest };

function node(over: Partial<AgentNodeV1> = {}): AgentNodeV1 {
  return {
    version: 1, agentId: "agent-lead", name: "Lead", engineId: "claude",
    sessionKey: "claude#lead", runtimeEpoch: "epoch-one", depth: 0, state: "idle",
    capabilities: [{ facet: "message", state: "available" }], scope,
    createdAt: now, updatedAt: now, ...over,
  };
}

const policy: OrchestratorPolicyV1 = {
  version: 1, agentId: "agent-lead", autonomy: "active", maxAccessMode: "isolated",
  maxWorkers: 8, maxConcurrent: 4, maxDelegations: 64, wallClockMs: 14_400_000,
  activatedAt: now, updatedAt: now, delegationsUsed: 3, spawnsUsed: 2,
  integration: "orchestrator", coordinationRequired: false, allowUnverifiedFence: false,
  modelCatalogue: "on-demand",
};

function message(over: Record<string, unknown>): OrchestrationSnapshotDto["messages"][number] {
  return {
    version: 1, messageId: "message-one", threadId: "thread-one", origin: "agent",
    senderAgentId: "agent-lead", recipientAgentId: "agent-w1", expectReply: true,
    deliveryState: "settled", createdAt: now, updatedAt: now, ...over,
  } as OrchestrationSnapshotDto["messages"][number];
}

function snapshot(agents: AgentNodeV1[], messages: OrchestrationSnapshotDto["messages"] = [],
    policies: OrchestratorPolicyV1[] = [],
    leases: WorktreeLeaseV1[] = []): OrchestrationSnapshotDto {
  return {
    version: 1, hostId: "host", runtimeEpoch: "epoch-one", revision: 1,
    createdAt: now, updatedAt: now, agents, messages, orchestratorPolicies: policies,
    roleClaims: [], worktreeLeases: leases,
    projection: { version: 1, truncated: false, eventFloorSequence: 0,
      omitted: { agents: 0, messages: 0, roleClaims: 0,
        worktreeLeases: 0, orchestratorPolicies: 0 } },
  } as OrchestrationSnapshotDto;
}

const BASE: AgentCanvasProps = {
  error: null,
  snapshot: snapshot([]),
  sessions: [{ key: "claude#lead", title: "Lead", engine: "claude", cwd: "/work/interface" }],
  projectRoot: "/work/interface",
  projectRoots: ["/work/interface"],
  onBrowseProjectRoot: async () => null,
  onSelectProjectRoot: () => undefined,
  layout: EMPTY_CANVAS_LAYOUT,
  onLayoutChange: () => undefined,
  onRegisterSession: () => Promise.resolve(),
  onCreateWorker: () => Promise.resolve(),
  onBindParent: () => Promise.resolve(),
  onSetPolicy: () => Promise.resolve(),
  onClearPolicy: () => Promise.resolve(),
  onStopAgent: () => Promise.resolve(),
  onSubmitWorker: () => Promise.resolve(),
  onIntegrateWorker: () => Promise.resolve({ taskId: "task-7", state: "verifying" as const }),
  onAbandonTask: () => Promise.resolve(),
  onAckObligation: () => Promise.resolve(),
  coordination: null,
  models: null,
  onOpenSession: () => undefined,
};

const coordinator: WorktreeCoordinatorV1 = {
  kind: "wcp", taskId: "task-7", owner: "iface:agent-lead:agent-w1", epoch: 1,
  leaseExpiresAt: "2026-08-25T12:15:00.000Z",
  writeScope: ["src/foo/**", "src/bar/**"], resources: [], taskState: "claimed",
  fence: "hook",
};

function lease(over: Partial<WorktreeLeaseV1> = {}): WorktreeLeaseV1 {
  return {
    version: 1, leaseId: "lease-one", canonicalPath: "/work/interface/.wcp/task-7-e1",
    repositoryRoot: "/work/interface", head: "b".repeat(40), dirtyStateDigest: digest,
    ownerAgentId: "agent-w1", ownerRevision: 1, state: "active",
    acquiredAt: now, updatedAt: now, coordinator, ...over,
  };
}

const markup = (over: Partial<AgentCanvasProps> = {}): string =>
  renderToStaticMarkup(createElement(AgentCanvas, { ...BASE, ...over }));

const populated = snapshot(
  [
    node({ agentId: "agent-lead", name: "Lead", role: "orchestrator" }),
    node({ agentId: "agent-w1", name: "Worker one", engineId: "codex",
      sessionKey: "codex#w1", role: "worker", parentAgentId: "agent-lead", depth: 1,
      state: "working" }),
    node({ agentId: "agent-w2", name: "Worker two", engineId: "acp:grok",
      sessionKey: "acp:grok#w2", role: "worker", parentAgentId: "agent-lead", depth: 1,
      state: "failed" }),
  ],
  [
    message({ messageId: "m-one", recipientAgentId: "agent-w1", deliveryState: "settled" }),
    message({ messageId: "m-two", recipientAgentId: "agent-w2", deliveryState: "failed" }),
  ],
  [policy],
);

describe("AgentCanvas states", () => {
  it("shows a load error as one inline alert beside the live graph", () => {
    const out = markup({ error: "orchestration_unavailable: no host" });
    expect(out).toContain('<p class="canvas-notice" role="alert">');
    expect(out).toContain("orchestration_unavailable: no host");
    expect(out.match(/role="alert"/gu)?.length).toBe(1);
    expect(out).toContain('role="application"');
  });

  it("tells a first-time user the three moves that build a star", () => {
    const out = markup();
    expect(out).toContain("Add a session, make it the orchestrator, draw lines to workers.");
    expect(out).toContain("canvas-world");
  });

  it("starts new workers in the selected project unless isolation is chosen", () => {
    const out = markup();
    expect(ORCHESTRATOR_POLICY_DEFAULTS.maxAccessMode).toBe("project");
    expect(out).toContain('aria-label="Access: Project checkout"');
    expect(out).not.toContain('aria-label="Access: Isolated worktree"');
    const form = source.slice(source.indexOf("function NewWorkerForm"),
      source.indexOf("export function AgentCanvas"));
    expect(form).toMatch(/form\.reset\(\);\s*setAccessChoice\("project"\);/u);
  });

  it("surfaces a host refusal inline with the workspace notice pattern", () => {
    expect(markup({ error: "Star topology: an orchestrator cannot be a worker." }))
      .toContain("Star topology: an orchestrator cannot be a worker.");
  });
});

describe("AgentCanvas populated graph", () => {
  const out = markup({ snapshot: populated });

  it("draws one node per agent with its provider mark, role, state and access", () => {
    expect(out).toContain('data-agent-id="agent-lead"');
    expect(out).toContain('data-agent-id="agent-w1"');
    expect(out).toContain('data-agent-id="agent-w2"');
    expect(out).toContain('data-provider-logo="claude"');
    expect(out).toContain('data-provider-logo="codex"');
    expect(out).toContain("Orchestrator");
    expect(out).toContain("Worker");
    expect(out).toContain("isolated");
    expect(out).toContain("canvas-status canvas-status-working");
    expect(out).toContain("canvas-status canvas-status-failed");
    // The node says the word the sidebar and the tile say, not the
    // host's own token: `working` read "working" here and "Working" everywhere
    // else, for the same agent in the same state.
    expect(out).toContain(">Working</span>");
    expect(out).not.toContain(">working</span>");
  });

  it("classes each lineage edge by the newest delegation's delivery state", () => {
    expect(out).toContain("canvas-edge canvas-edge-settled");
    expect(out).toContain("canvas-edge canvas-edge-failed");
    expect(out).not.toContain("canvas-edge canvas-edge-inflight");
    expect(markup({
      snapshot: snapshot(populated.agents as AgentNodeV1[],
        [message({ messageId: "m-one", recipientAgentId: "agent-w1",
          deliveryState: "queued" })], [policy]),
    })).toContain("canvas-edge canvas-edge-inflight");
  });

  it("states the orchestrator's live budget and offers the pause switch", () => {
    expect(out).toContain("workers 2/8 · running 1/4 · delegations 3/64");
    expect(out).toContain("Pause");
    expect(markup({
      snapshot: snapshot(populated.agents as AgentNodeV1[], populated.messages,
        [{ ...policy, autonomy: "paused" }]),
    })).toContain("Resume");
  });

  it("carries the canvas region's own a11y contract", () => {
    expect(out).toContain('role="application"');
    expect(out).toContain("aria-describedby=");
    expect(out).toContain('aria-live="polite"');
    expect(out).toContain('aria-label="Link Lead to a worker"');
    // Tab cycling moves a selection, not the focus ring, so the selected card
    // has to be named as the active descendant of the viewport.
    expect(out).toContain('id="canvas-node-agent-lead"');
    expect(out).toContain('id="canvas-node-agent-w1"');
    expect(source).toContain("aria-activedescendant={selectedAgentId === null");
    expect(source).toContain("? undefined : `canvas-node-${selectedAgentId}`}");
    // ARIA permits `aria-selected` only on gridcell/option/row/tab/treeitem, so
    // the role="group" cards say which one is selected by class and by being
    // named the viewport's active descendant, never with that attribute.
    expect(source).not.toContain("aria-selected");
    expect(source).toContain('? "canvas-node is-selected" : "canvas-node"');
  });

  it("says on each edge what the newest delegation is doing and when it was sent", () => {
    expect(out).toContain("Lead \u2192 Worker one: settled \u00b7 2026-08-25T12:00:00.000Z");
    expect(out).toContain("Lead \u2192 Worker two: failed \u00b7 2026-08-25T12:00:00.000Z");
    // No delegation yet is the one case with nothing to date.
    expect(markup({ snapshot: snapshot(populated.agents as AgentNodeV1[], [], [policy]) }))
      .toContain("Lead \u2192 Worker one: idle<");
  });

  it("names every key verb in the hint the region points at", () => {
    expect(out).toContain("M opens the actions menu for the selected node.");
    expect(out).toContain("Delete unlinks a selected edge.");
    expect(out).toContain("L links an orchestrator to a worker");
  });

  it("offers the strip actions the spec names", () => {
    expect(out).toContain('aria-label="Project: interface"');
    expect(out).toContain('aria-label="Add session: Choose a session\u2026"');
    expect(out).toContain('aria-label="New worker"');
    expect(out).toContain("Auto-arrange");
    expect(out).toContain('aria-label="Zoom out"');
    expect(out).toContain('aria-label="Zoom in"');
    expect(out).toContain('aria-label="Reset zoom"');
  });

  it("ships its stylesheet with tokens only and no hard-coded colour", () => {
    expect(out).toContain(".canvas-world");
    expect(out).toContain("var(--status-working)");
    expect(out).toContain("var(--danger-fg)");
    // The status pill takes the status tokens, except idle: it has no text
    // tier (see rowGraphicTokens in theme-accessibility.test.ts), so painting
    // pill TEXT with --status-idle would land under 4.5:1 in eight themes.
    expect(out).toContain(".canvas-status-failed { color:var(--status-failed); }");
    expect(out).toContain(".canvas-status-idle { color:var(--fg-dim); }");
    expect(out).not.toContain("var(--status-idle)");
    expect(/#[0-9a-fA-F]{3,8}\b/u.test(source.slice(source.indexOf("AGENT_CANVAS_CSS"))))
      .toBe(false);
  });
});

const keys = (over: Partial<CanvasKeyContext> = {}): CanvasKeyContext => ({
  order: ["agent-lead", "agent-w1", "agent-w2"],
  selectedAgentId: null,
  selectedEdgeId: null,
  orchestratorIds: new Set(["agent-lead"]),
  ...over,
});

describe("canvasKeyAction", () => {
  it("cycles the nodes forward and backward with Tab", () => {
    expect(canvasKeyAction({ key: "Tab", shiftKey: false }, keys()))
      .toEqual({ kind: "select", agentId: "agent-lead" });
    expect(canvasKeyAction({ key: "Tab", shiftKey: false },
      keys({ selectedAgentId: "agent-lead" })))
      .toEqual({ kind: "select", agentId: "agent-w1" });
    expect(canvasKeyAction({ key: "Tab", shiftKey: true },
      keys({ selectedAgentId: "agent-w2" })))
      .toEqual({ kind: "select", agentId: "agent-w1" });
  });

  it("lets Tab out of the canvas at both ends of the cycle (WCAG 2.1.2)", () => {
    expect(canvasKeyAction({ key: "Tab", shiftKey: false },
      keys({ selectedAgentId: "agent-w2" }))).toEqual({ kind: "leave" });
    expect(canvasKeyAction({ key: "Tab", shiftKey: true },
      keys({ selectedAgentId: "agent-lead" }))).toEqual({ kind: "leave" });
    expect(canvasKeyAction({ key: "Tab", shiftKey: true }, keys()))
      .toEqual({ kind: "leave" });
    expect(canvasKeyAction({ key: "Tab", shiftKey: false }, keys({ order: [] })))
      .toEqual({ kind: "leave" });
  });

  it("clears the selection on Escape", () => {
    expect(canvasKeyAction({ key: "Escape", shiftKey: false },
      keys({ selectedAgentId: "agent-w1" }))).toEqual({ kind: "clear" });
  });

  it("unlinks a selected EDGE on Delete and Backspace, and only an edge", () => {
    expect(canvasKeyAction({ key: "Delete", shiftKey: false },
      keys({ selectedAgentId: "agent-w1", selectedEdgeId: "agent-w2" })))
      .toEqual({ kind: "unlink", agentId: "agent-w2" });
    expect(canvasKeyAction({ key: "Backspace", shiftKey: false },
      keys({ selectedEdgeId: "agent-w1" })))
      .toEqual({ kind: "unlink", agentId: "agent-w1" });
    // A node is not its lineage: cutting a parent is the menu's Unlink, which
    // says so, not a Delete on whatever happens to be selected.
    expect(canvasKeyAction({ key: "Delete", shiftKey: false },
      keys({ selectedAgentId: "agent-w1" }))).toEqual({ kind: "ignore" });
    expect(canvasKeyAction({ key: "Delete", shiftKey: false }, keys()))
      .toEqual({ kind: "ignore" });
  });

  it("zooms on minus, plus and zero with no selection required", () => {
    expect(canvasKeyAction({ key: "-", shiftKey: false }, keys()))
      .toEqual({ kind: "zoom", delta: -.1 });
    expect(canvasKeyAction({ key: "+", shiftKey: false }, keys()))
      .toEqual({ kind: "zoom", delta: .1 });
    expect(canvasKeyAction({ key: "=", shiftKey: false }, keys()))
      .toEqual({ kind: "zoom", delta: .1 });
    expect(canvasKeyAction({ key: "0", shiftKey: false }, keys()))
      .toEqual({ kind: "zoom-reset" });
  });

  it("opens the selected node with Enter, in another pane with Shift", () => {
    expect(canvasKeyAction({ key: "Enter", shiftKey: false },
      keys({ selectedAgentId: "agent-w1" })))
      .toEqual({ kind: "open", agentId: "agent-w1", newPane: false });
    expect(canvasKeyAction({ key: "Enter", shiftKey: true },
      keys({ selectedAgentId: "agent-w1" })))
      .toEqual({ kind: "open", agentId: "agent-w1", newPane: true });
  });

  it("opens the link picker from an orchestrator and refuses it from a worker", () => {
    expect(canvasKeyAction({ key: "l", shiftKey: false },
      keys({ selectedAgentId: "agent-lead" })))
      .toEqual({ kind: "link", agentId: "agent-lead" });
    expect(canvasKeyAction({ key: "L", shiftKey: false },
      keys({ selectedAgentId: "agent-lead" })))
      .toEqual({ kind: "link", agentId: "agent-lead" });
    expect(canvasKeyAction({ key: "l", shiftKey: false },
      keys({ selectedAgentId: "agent-w1" })))
      .toEqual({ kind: "refuse", message: "Star topology: an orchestrator cannot be a worker." });
  });

  it("opens the selected node's menu with M, ContextMenu and Shift+F10", () => {
    const at = keys({ selectedAgentId: "agent-w1" });
    for (const key of ["m", "M", "ContextMenu"]) {
      expect(canvasKeyAction({ key, shiftKey: false }, at))
        .toEqual({ kind: "menu", agentId: "agent-w1" });
    }
    expect(canvasKeyAction({ key: "F10", shiftKey: true }, at))
      .toEqual({ kind: "menu", agentId: "agent-w1" });
    expect(canvasKeyAction({ key: "F10", shiftKey: false }, at)).toEqual({ kind: "ignore" });
    expect(canvasKeyAction({ key: "m", shiftKey: false }, keys())).toEqual({ kind: "ignore" });
  });

  it("nudges the selected node 8px by arrow and 32px with Shift", () => {
    const at = keys({ selectedAgentId: "agent-w1" });
    expect(canvasKeyAction({ key: "ArrowLeft", shiftKey: false }, at))
      .toEqual({ kind: "nudge", agentId: "agent-w1", dx: -8, dy: 0 });
    expect(canvasKeyAction({ key: "ArrowRight", shiftKey: true }, at))
      .toEqual({ kind: "nudge", agentId: "agent-w1", dx: 32, dy: 0 });
    expect(canvasKeyAction({ key: "ArrowUp", shiftKey: false }, at))
      .toEqual({ kind: "nudge", agentId: "agent-w1", dx: 0, dy: -8 });
    expect(canvasKeyAction({ key: "ArrowDown", shiftKey: true }, at))
      .toEqual({ kind: "nudge", agentId: "agent-w1", dx: 0, dy: 32 });
  });

  it("ignores node verbs with nothing selected and keys it does not bind", () => {
    expect(canvasKeyAction({ key: "ArrowUp", shiftKey: false }, keys()))
      .toEqual({ kind: "ignore" });
    expect(canvasKeyAction({ key: "Enter", shiftKey: false }, keys()))
      .toEqual({ kind: "ignore" });
    expect(canvasKeyAction({ key: "q", shiftKey: false },
      keys({ selectedAgentId: "agent-w1" }))).toEqual({ kind: "ignore" });
    expect(canvasKeyAction({ key: "ArrowUp", shiftKey: false },
      keys({ selectedAgentId: "gone" }))).toEqual({ kind: "ignore" });
  });
});

describe("canvasMenuItems", () => {
  const lead = node({ agentId: "agent-lead", role: "orchestrator" });
  const worker = node({ agentId: "agent-w1", role: "worker", parentAgentId: "agent-lead" });
  const loose = node({ agentId: "agent-solo" });
  const labels = (item: { label: string }): string => item.label;

  it("offers an orchestrator its policy, pause, demote and fan-out actions", () => {
    expect(canvasMenuItems(lead, { childCount: 0, blocked: false, autonomy: "active" })
      .map(labels)).toEqual([
      "Edit policy…", "Demote", "Pause", "Stop session", "Stop all workers", "Open",
      "Remove from canvas",
    ]);
  });

  it("gives the keyboard a Pause verb, and names it for the state it moves to", () => {
    expect(canvasMenuItems(lead, { childCount: 0, blocked: false, autonomy: "paused" })
      .find(item => item.kind === "pause")?.label).toBe("Resume");
    // An agent with no policy has no autonomy to flip.
    expect(canvasMenuItems(lead, { childCount: 0, blocked: false }).map(labels))
      .not.toContain("Pause");
  });

  it("names Stop for what the node actually is", () => {
    const stop = (item: { kind: string }): boolean => item.kind === "stop";
    expect(canvasMenuItems(worker, { childCount: 0, blocked: false }).find(stop)?.label)
      .toBe("Stop worker");
    expect(canvasMenuItems(lead, { childCount: 0, blocked: false }).find(stop)?.label)
      .toBe("Stop session");
    expect(canvasMenuItems(loose, { childCount: 0, blocked: false }).find(stop)?.label)
      .toBe("Stop session");
  });

  it("refuses Demote while the orchestrator still has workers", () => {
    const withKids = canvasMenuItems(lead, { childCount: 2, blocked: false });
    expect(withKids.find(item => item.kind === "demote")?.disabled).toBe(true);
    expect(withKids.find(item => item.kind === "policy")?.disabled).toBe(false);
    expect(canvasMenuItems(lead, { childCount: 0, blocked: false })
      .find(item => item.kind === "demote")?.disabled).toBe(false);
  });

  it("offers a linked worker Unlink but never Make orchestrator", () => {
    const items = canvasMenuItems(worker, { childCount: 0, blocked: false }).map(labels);
    expect(items).toEqual(["Unlink", "Stop worker", "Open", "Remove from canvas"]);
  });

  it("adds the five wcp task actions to a coordinated worker, in spec order", () => {
    expect(canvasMenuItems(worker, { childCount: 0, blocked: false, coordinated: true })
      .map(labels)).toEqual([
      "Unlink", "Submit", "Dry-run verify", "Integrate", "Abandon task",
      "Acknowledge obligation\u2026", "Stop worker", "Open", "Remove from canvas",
    ]);
    // Integrate is the human's act; policy.integration gates only the tool.
    expect(canvasMenuItems(worker, { childCount: 0, blocked: false, coordinated: true })
      .find(item => item.kind === "integrate")?.disabled).toBe(false);
    expect(canvasMenuItems(worker, { childCount: 0, blocked: true, coordinated: true })
      .filter(item => item.kind === "submit" || item.kind === "integrate")
      .every(item => item.disabled)).toBe(true);
  });

  it("offers Make orchestrator only to an unparented non-orchestrator", () => {
    expect(canvasMenuItems(loose, { childCount: 0, blocked: false }).map(labels))
      .toEqual(["Make orchestrator", "Stop session", "Open", "Remove from canvas"]);
  });

  it("disables every mutating action while blocked, and neither Open nor Remove", () => {
    const items = canvasMenuItems(lead, { childCount: 0, blocked: true, autonomy: "active" });
    for (const item of items) {
      expect(item.disabled).toBe(item.kind !== "open" && item.kind !== "remove");
    }
  });
});

describe("canvasLinkOutcome", () => {
  it("links an ordinary worker", () => {
    expect(canvasLinkOutcome("agent-lead", node({ agentId: "agent-w1", role: "worker" })))
      .toEqual({ kind: "link" });
  });

  it("refuses a second orchestrator with the star-topology message", () => {
    expect(canvasLinkOutcome("agent-lead", node({ agentId: "agent-two", role: "orchestrator" })))
      .toEqual({ kind: "refuse",
        message: "Star topology: an orchestrator cannot be a worker." });
  });

  it("ignores a self-link and an unknown target", () => {
    expect(canvasLinkOutcome("agent-lead", node({ agentId: "agent-lead" })))
      .toEqual({ kind: "ignore" });
    expect(canvasLinkOutcome("agent-lead", undefined)).toEqual({ kind: "ignore" });
  });
});

describe("canvasOpenTarget", () => {
  it("hands back the session key when the agent has one", () => {
    expect(canvasOpenTarget(node({ sessionKey: "claude#lead" })))
      .toEqual({ sessionKey: "claude#lead" });
  });

  it("refuses an agent with no recoverable provider session", () => {
    expect(canvasOpenTarget(node({ sessionKey: undefined })))
      .toEqual({ refusal: "That agent has no recoverable provider session yet." });
  });
});

describe("canvas view arithmetic", () => {
  it("clamps zoom to the half-to-double range in tenths", () => {
    expect(clampZoom(1, .1)).toBe(1.1);
    expect(clampZoom(1, -.1)).toBe(.9);
    expect(clampZoom(.5, -.1)).toBe(.5);
    expect(clampZoom(2, .1)).toBe(2);
    expect(clampZoom(1.2, .1)).toBe(1.3);
  });

  it("divides drag distance by the zoom and rounds to whole world pixels", () => {
    expect(draggedPosition({ x: 100, y: 40 }, 30, -20, 1)).toEqual({ x: 130, y: 20 });
    expect(draggedPosition({ x: 100, y: 40 }, 30, -20, 2)).toEqual({ x: 115, y: 30 });
    expect(draggedPosition({ x: 0, y: 0 }, 5, 5, 2)).toEqual({ x: 3, y: 3 });
  });

  it("maps a client point into world space through pan and zoom", () => {
    expect(canvasWorldPoint({ left: 10, top: 20 }, 110, 120, { x: 0, y: 0 }, 1))
      .toEqual({ x: 100, y: 100 });
    expect(canvasWorldPoint({ left: 10, top: 20 }, 110, 120, { x: 50, y: 50 }, 1))
      .toEqual({ x: 50, y: 50 });
    expect(canvasWorldPoint({ left: 0, top: 0 }, 100, 100, { x: 0, y: 0 }, 2))
      .toEqual({ x: 50, y: 50 });
  });
});

describe("refusalText", () => {
  it("prefers an Error's message and stringifies anything else", () => {
    expect(refusalText(new Error("policy_exhausted: maxWorkers")))
      .toBe("policy_exhausted: maxWorkers");
    expect(refusalText("capacity")).toBe("capacity");
    expect(refusalText(undefined)).toBe("undefined");
  });
});

describe("canvas coordination arithmetic", () => {
  it("finds the coordinator record on the worker's own wcp lease", () => {
    const withLease = snapshot(populated.agents as AgentNodeV1[], [], [policy], [lease()]);
    expect(canvasCoordinator(withLease, "agent-w1")?.taskId).toBe("task-7");
    expect(canvasCoordinator(withLease, "agent-w2")).toBeUndefined();
    expect(canvasCoordinator(null, "agent-w1")).toBeUndefined();
    const plain = snapshot(populated.agents as AgentNodeV1[], [], [policy],
      [{ ...lease(), coordinator: undefined }]);
    expect(canvasCoordinator(plain, "agent-w1")).toBeUndefined();
  });

  it("reads only an active lease, so a rebased worker loses its coordination block", () => {
    // rebaseWorker releases the retired worker's lease without erasing the
    // coordinator it carried. Reading a released one keeps a retired node
    // showing task/scope/fence and refusing "Remove from canvas" forever.
    for (const state of ["released", "escrow", "failed", "unknown"] as const) {
      const stale = snapshot(populated.agents as AgentNodeV1[], [], [policy],
        [lease({ state })]);
      expect(canvasCoordinator(stale, "agent-w1")).toBeUndefined();
    }
    expect(canvasCoordinator(snapshot(populated.agents as AgentNodeV1[], [], [policy],
      [lease()]), "agent-w1")?.taskId).toBe("task-7");
  });

  it("names task, fence and lease for the activedescendant announcement", () => {
    const at = Date.parse("2026-08-25T12:15:00.000Z");
    expect(coordinationLabel({ ...coordinator, fence: "unverified" }, at - 12 * 60_000))
      .toBe("task task-7, fence unverified, lease 12m left");
  });

  it("treats an unknown or unretired task state as still active", () => {
    expect(coordinatedTaskActive(coordinator)).toBe(true);
    expect(coordinatedTaskActive({ ...coordinator, taskState: undefined })).toBe(true);
    expect(coordinatedTaskActive({ ...coordinator, taskState: "submitted" })).toBe(true);
    for (const state of ["accepted", "abandoned", "cancelled", "retired"]) {
      expect(coordinatedTaskActive({ ...coordinator, taskState: state })).toBe(false);
    }
  });

  it("names the first write pattern and counts the rest", () => {
    expect(scopeSummary(["src/foo/**", "src/bar/**"])).toBe("src/foo/** +1");
    expect(scopeSummary(["src/foo/**"])).toBe("src/foo/**");
    expect(scopeSummary([])).toBe("no scope");
  });

  it("reads the lease countdown off the timestamp, with no timer", () => {
    const at = Date.parse("2026-08-25T12:15:00.000Z");
    expect(leaseCountdown("2026-08-25T12:15:00.000Z", at - 12 * 60_000)).toBe("lease 12m left");
    expect(leaseCountdown("2026-08-25T12:15:00.000Z", at - 20_000)).toBe("lease under 1m left");
    expect(leaseCountdown("2026-08-25T12:15:00.000Z", at)).toBe("lease expired");
    expect(leaseCountdown("not-a-date", at)).toBe("lease unknown");
  });

  it("says engine, model and effort as launched, and never invents them", () => {
    expect(engineLine(node({ engineId: "codex" }),
      { key: "codex#w1", title: "W", engine: "codex", cwd: "/work/interface",
        model: "gpt-5.6-luna", effort: "ultra" })).toBe("Codex \u00b7 gpt-5.6-luna \u00b7 ultra");
    expect(engineLine(node({ engineId: "codex" }),
      { key: "codex#w1", title: "W", engine: "codex", cwd: "/work/interface",
        model: "gpt-5.6-luna", effort: null })).toBe("Codex \u00b7 gpt-5.6-luna");
    expect(engineLine(node({ engineId: "claude" }), undefined)).toBe("Claude Code");
  });
});

describe("canvasVerifyingNotice", () => {
  const verifying = snapshot(populated.agents as AgentNodeV1[], [], [policy],
    [lease({ coordinator: { ...coordinator, taskState: "verifying" } })]);
  const settled = snapshot(populated.agents as AgentNodeV1[], [], [policy],
    [lease({ coordinator: { ...coordinator, taskState: "accepted" } })]);
  const claim = { agentId: "agent-w1", taskId: "task-7", at: settled };

  it("says nothing until a verify has been claimed", () => {
    expect(canvasVerifyingNotice(verifying, null)).toBeNull();
  });

  it("stands while the coordinator says verifying, and clears when it settles", () => {
    expect(canvasVerifyingNotice(verifying, { ...claim, at: null }))
      .toBe("Verifying task-7 \u2014 the result arrives on this canvas.");
    // The publish landed: the record no longer says verifying, so neither does
    // the strip. This is the case the old local flag never cleared.
    expect(canvasVerifyingNotice(settled, { ...claim, at: null })).toBeNull();
  });

  it("stands through a dry run until the next snapshot lands", () => {
    // wcp leaves a dry run in `submitted` — it stays heartbeatable — so there is
    // no record to read and the claim's own snapshot is what it stands on.
    expect(canvasVerifyingNotice(settled, claim))
      .toBe("Verifying task-7 \u2014 the result arrives on this canvas.");
    expect(canvasVerifyingNotice(snapshot([]), claim)).toBeNull();
  });

  it("clears when the worker's lease is gone entirely", () => {
    expect(canvasVerifyingNotice(snapshot([]), { ...claim, at: null })).toBeNull();
  });
});

describe("canvas policy validation", () => {
  it("accepts one allowlist entry per line and drops blank lines", () => {
    expect(parseModelAllowlist("codex:*\n\nacp:grok:grok-4.6\n"))
      .toEqual({ entries: ["codex:*", "acp:grok:grok-4.6"] });
    expect(parseModelAllowlist("   ")).toEqual({ entries: [] });
  });

  it("refuses an entry that is not engineId:modelKey, and more than 32 of them", () => {
    expect(parseModelAllowlist("claude"))
      .toEqual({ refusal: "Allowlist entries look like codex:gpt-5.6-luna or codex:*." });
    expect(parseModelAllowlist(":luna"))
      .toEqual({ refusal: "Allowlist entries look like codex:gpt-5.6-luna or codex:*." });
    expect(parseModelAllowlist("codex:"))
      .toEqual({ refusal: "Allowlist entries look like codex:gpt-5.6-luna or codex:*." });
    expect(parseModelAllowlist("codex:a b"))
      .toEqual({ refusal: "Allowlist entries look like codex:gpt-5.6-luna or codex:*." });
    const many = Array.from({ length: 33 }, (_, index) => `codex:m${index}`).join("\n");
    expect(parseModelAllowlist(many))
      .toEqual({ refusal: "At most 32 allowlist entries." });
  });

  it("bounds routing notes at 1024 bytes, counted as bytes", () => {
    expect(modelNotesRefusal("Haiku for verbatim work.")).toBeNull();
    expect(modelNotesRefusal("")).toBeNull();
    expect(modelNotesRefusal("\u00e9".repeat(513)))
      .toBe("Routing notes are limited to 1024 bytes.");
  });

  it("turns the live catalogue into the ids a human can paste", () => {
    expect(modelHint({
      engines: [{ engineId: "codex", ok: true,
        models: [{ id: "gpt-5.6-luna", name: "Luna", efforts: ["ultra"] }] }],
      synthetic: { ultracode: "note" }, catalogue: "live",
    })).toBe("Known ids: codex:gpt-5.6-luna");
    expect(modelHint({ engines: [], synthetic: { ultracode: "note" },
      catalogue: "unavailable" })).toBe("Model catalogue unavailable.");
  });
});

describe("AgentCanvas coordination surface", () => {
  const coordinated = markup({
    snapshot: snapshot(populated.agents as AgentNodeV1[], populated.messages, [policy],
      [lease({ coordinator: { ...coordinator, submissionId: "sub-9",
        taskState: "submitted", fence: "unverified" } })]),
    sessions: [{ key: "codex#w1", title: "Worker one", engine: "codex",
      cwd: "/work/interface", model: "gpt-5.6-luna", effort: "ultra" }],
    coordination: { registered: true, activeTasks: 2,
      publishedUncommittedPaths: ["src/a.ts", "src/b.ts"], requiredChecks: 1 },
  });

  it("puts task, scope, state, lease and fence on the coordinated worker's card", () => {
    expect(coordinated).toContain("task task-7 \u00b7 src/foo/** +1");
    expect(coordinated).toContain("submitted \u00b7 submission sub-9");
    expect(coordinated).toContain("lease expired");
    expect(coordinated).toContain('class="canvas-fence badge badge-quiet is-unverified"');
    expect(coordinated).toContain("fence unverified");
    // An uncoordinated worker carries none of it. (The class name itself is in
    // every render, because the stylesheet that declares it is.)
    expect(markup({ snapshot: populated })).not.toContain('class="canvas-fence');
  });

  it("appends the coordination summary to the node's accessible name", () => {
    // The canvas navigates by aria-activedescendant, so the group's name is the
    // whole announcement — including the fence, which the pill states in colour.
    expect(coordinated).toContain('aria-label="Worker one, Codex, Working, task task-7, '
      + 'fence unverified, lease expired"');
    // An uncoordinated node keeps the plain three-part name.
    expect(markup({ snapshot: populated }))
      .toContain('aria-label="Worker one, Codex, Working"');
  });

  it("says engine, model and effort as the worker was launched", () => {
    expect(coordinated).toContain("Codex \u00b7 gpt-5.6-luna \u00b7 ultra");
  });

  it("carries the two strip notices wcp's state implies", () => {
    expect(coordinated).toContain("2 published changes awaiting your commit");
    expect(coordinated).toContain("canonical is integrator-only while 2 tasks are active");
    // Notices are standing facts, not the one alert slot the refusal owns.
    expect(coordinated.match(/role="alert"/gu) ?? []).toEqual([]);
    expect(markup()).not.toContain("awaiting your commit");
  });

  it("pluralises both strip notices off their own counts", () => {
    const single = markup({
      snapshot: snapshot(populated.agents as AgentNodeV1[], populated.messages, [policy],
        [lease()]),
      coordination: { registered: true, activeTasks: 1,
        publishedUncommittedPaths: ["src/a.ts"], requiredChecks: 1 },
    });
    expect(single).toContain("1 published change awaiting your commit");
    expect(single).toContain("canonical is integrator-only while 1 task is active");
    expect(single).not.toContain("1 published changes");
    expect(single).not.toContain("1 tasks are");
  });

  it("offers coordinated as an access mode wherever a human picks one", () => {
    // AppSelect renders its rows into a portal only while the menu is open, so
    // the guarantee is checked where it is decided: both pickers that name an
    // access mode take the same list, and that list carries `coordinated`.
    expect(source).toContain('const ACCESS_MODES = ["read-only", "coordinated", '
      + '"isolated", "project"] as const;');
    expect(source.match(/options=\{ACCESS_ROWS\}/gu)?.length).toBe(2);
  });

  it("keeps picker option lists stable across pan and link-drag renders", () => {
    // AppSelect's per-option disambiguation is keyed on the list's identity
    // and scans every option per meta row; fresh lists re-ran it per pointermove.
    expect(source).toContain("options={projectOptions}");
    expect(source).toContain("options={sessionOptions}");
    expect(source).toMatch(/const projectOptions = useMemo\([\s\S]*?\[props\.projectRoots\]\);/u);
    expect(source).toMatch(/const sessionOptions = useMemo\([\s\S]*?\[addableSessions\]\);/u);
  });

  it("takes an orchestrator's workers with it when it is removed", () => {
    // The tedium this replaces: every worker refused removal on the link its
    // lead had put there, so the human unlinked each one by hand first.
    expect(source).toContain("const children = childrenOf(agent.agentId);");
    expect(source).toContain("layout = hideNode(layout, props.projectRoot, child.agentId);");
    // Severed, not just hidden: a worker whose lead is gone would otherwise
    // refuse every later removal on a link to a node nothing draws.
    expect(source).toContain("await props.onBindParent({ agentId: child.agentId, "
      + "parentAgentId: null,");
    expect(source).toContain("await props.onClearPolicy({ agentId: agent.agentId,");
  });

  it("still refuses when a worker holds a live coordinator task", () => {
    // Abandoning it would strand a wcp lease with nothing left to release it.
    const remove = source.slice(source.indexOf("const removeFromCanvas"),
      source.indexOf("const runMenuAction"));
    expect(remove).toContain("still holds its coordinator task.");
    expect(remove.indexOf("still holds its coordinator task."))
      .toBeLessThan(remove.indexOf("props.onLayoutChange"));
  });

  it("refuses Remove from canvas while the task still holds its scope", () => {
    expect(source).toContain("if (coordination !== undefined && coordinatedTaskActive("
      + "coordination)) {");
    expect(source)
      .toContain('const COORDINATION_REFUSAL = "Integrate, or abandon the task first.";');
    // The coordinated refusal is more specific than the lineage one, so it runs first.
    expect(source.indexOf("refuse(COORDINATION_REFUSAL)"))
      .toBeLessThan(source.indexOf("refuse(LINKED_REFUSAL)"));
  });

  it("keeps every card action in the menu the keyboard already reaches", () => {
    for (const kind of ["submit", "dry-run", "integrate", "abandon", "ack"]) {
      expect(source).toContain(`case "${kind}":`);
    }
    expect(source).toContain("props.onSubmitWorker({");
    expect(source).toContain("case \"dry-run\":\n        integrate(agent.agentId, true);");
    expect(source).toContain("props.onAbandonTask({");
    // No new key binding: M / ContextMenu / Shift+F10 is the keyboard path.
    expect(source.match(/if \(event\.key === "m" \|\| event\.key === "M"/gu)?.length).toBe(1);
  });
});

describe("AgentCanvas policy form", () => {
  const withPolicyOpen = (): string => markup({
    snapshot: snapshot(populated.agents as AgentNodeV1[], [], [policy]),
    models: { engines: [{ engineId: "codex", ok: true,
      models: [{ id: "gpt-5.6-luna", name: "Luna", efforts: ["ultra"] }] }],
      synthetic: { ultracode: "note" }, catalogue: "live" },
  });

  it("declares the six new policy fields with their bounds", () => {
    expect(source).toContain('<FormSelect fieldClassName="canvas-field" label="Integration"');
    expect(source).toContain('<input name="coordinationRequired" type="checkbox"');
    expect(source).toContain('<input name="allowUnverifiedFence" type="checkbox"');
    expect(source).toContain('<FormSelect fieldClassName="canvas-field" label="Model catalogue"');
    expect(source).toContain('<textarea name="modelAllowlist" rows={3}');
    expect(source).toContain('<textarea name="modelNotes" rows={3}');
    expect(source).toContain("const MODEL_ALLOWLIST_MAX = 32;");
    expect(source).toContain("const MODEL_NOTES_MAX_BYTES = 1_024;");
  });

  it("refuses a malformed allowlist or oversized notes before the host is touched", () => {
    expect(source).toContain('if ("refusal" in allowlist) { refuse(allowlist.refusal); return; }');
    expect(source).toContain("if (notesRefusal !== null) { refuse(notesRefusal); return; }");
    // Refusing must not close the form the human is still editing.
    const submit = source.match(
      /const allowlist = parseModelAllowlist[\s\S]*?setPolicyAgentId\(null\);/u)?.[0] ?? "";
    expect(submit).toContain("refuse(allowlist.refusal); return;");
  });

  it("sends an emptied Routing notes as the host's clear signal", () => {
    // `""` is what clears stored guidance; omitting the field carries it forward.
    expect(source).toContain("          modelNotes: notes,");
    expect(source).not.toContain('notes === "" ? {} : { modelNotes: notes }');
  });

  it("renders the live catalogue as the ids the allowlist accepts", () => {
    expect(withPolicyOpen()).toContain("canvas-policy-hint");
    expect(source).toContain('{props.models !== null && <p id={`${hintId}-catalogue`} '
      + 'className="canvas-policy-hint">');
    // The hint is the only statement of the id format, so the field it explains
    // names it rather than leaving it to sighted proximity.
    expect(source).toContain('aria-describedby={props.models === null ? undefined : '
      + '`${hintId}-catalogue`}');
  });

  it("prompts for the acknowledged path from the task's pending obligations", () => {
    expect(source).toContain('<form className="canvas-ack is-floating" aria-label="Acknowledge obligation"');
    // wcp reports obligations as paths; writeScope holds patterns, and one of
    // those typed back would match no obligation at all.
    expect(source).not.toContain("ackCoordinator.writeScope");
    expect(source).toContain("(ackCoordinator.pendingObligations ?? []).map(path =>");
    expect(source).toContain('<p className="canvas-ack-pending">No pending obligations '
      + 'on this task.</p>');
    expect(source).toContain("props.onAckObligation({");
  });

  it("lays coordinated nodes out against the taller card they draw", () => {
    expect(source).toContain("const coordinatedIds = useMemo(() => new Set(agents");
    expect(source).toContain("      coordinatedIds),");
  });
});

// The remaining contract lives in browser-only plumbing that the node test
// environment cannot mount (no DOM, and no new dependency may be added), so the
// spec's own testing note asks for a source contract here instead.
describe("AgentCanvas effect contract", () => {
  it("does no work while the workspace surface is hidden", () => {
    expect(source).toContain("const surfaceHidden = useCallback((): boolean =>");
    expect(source).toContain("rootRef.current === null || rootRef.current.offsetParent === null");
    expect(source.match(/if \(surfaceHidden\(\)/gu)?.length).toBeGreaterThanOrEqual(4);
    expect(source).not.toContain("requestAnimationFrame");
    expect(source).not.toContain("ResizeObserver");
    // And no clock of its own anywhere in the file: the lease countdown is the
    // pure `leaseCountdown` over the snapshot, redrawn when the snapshot lands.
    expect(source).not.toContain("setInterval");
    expect(source).not.toContain("setTimeout");
  });

  it("follows CenterPane's pointer-drag pattern and writes positions on drag end only", () => {
    expect(source).toContain('window.addEventListener("pointermove", move);');
    expect(source).toContain('window.addEventListener("pointerup", stop, { once: true });');
    expect(source).toContain('window.removeEventListener("pointermove", move);');
    // The layout and zoom a gesture writes are the CURRENT ones, not the ones
    // it closed over when the pointer went down.
    expect(source)
      .toContain("props.onLayoutChange(moveNode(layoutRef.current, props.projectRoot,");
    expect(source).toContain("zoomRef.current);");
    // The keyboard nudge is not a gesture: it runs inside the render that
    // handled the key, so props.layout is the right layout there.
    expect(source.match(/moveNode\(props\.layout, props\.projectRoot/gu)?.length).toBe(1);
  });

  it("ends all three gestures on pointercancel too, and never on a bare click", () => {
    expect(source.match(/window\.addEventListener\("pointercancel", cancel/gu)?.length).toBe(3);
    expect(source.match(/window\.removeEventListener\("pointercancel", cancel\);/gu)?.length)
      .toBe(3);
    expect(source).toContain("if (landed.x === origin.x && landed.y === origin.y) return;");
    // The port and the Pause button are pressed, not dragged.
    expect(source.match(/event\.stopPropagation\(\);/gu)?.length).toBe(2);
  });

  it("leaves Enter and Space to whatever card control has the focus", () => {
    expect(source).toContain("if (event.target !== event.currentTarget");
    expect(source).toContain('(event.key === "Enter" || event.key === " ")) return;');
    // And Tab to the menu the keyboard just opened, which follows the viewport.
    expect(source)
      .toContain('if (menuAgentId !== null && event.key === "Tab" && !event.shiftKey) return;');
  });

  it("removes a node from the canvas by hiding it, and refuses to cut lineage", () => {
    expect(source).toContain("props.onLayoutChange(hideNode(layout, props.projectRoot,");
    expect(source)
      .toContain("if (agent.parentAgentId !== undefined) { refuse(LINKED_REFUSAL); return; }");
    expect(source).toContain('const LINKED_REFUSAL = "Unlink before removing from the canvas.";');
    // Adding the session back is the undo.
    expect(source).toContain("props.onLayoutChange(unhideNode(props.layout, props.projectRoot,");
    expect(source).toContain("&& !hidden.has(agent.agentId))");
  });

  it("puts a live refusal in front of a standing load error, and announces the flip", () => {
    expect(source).toContain("const alertMessage = notice ?? props.error ?? null;");
    expect(source).toContain("pausing ? `Paused ${name}` : `Resumed ${name}`);");
  });
});

const app = readFileSync(new URL("./App.tsx", import.meta.url), "utf8");
const orchestrationHooks = readFileSync(new URL("./use-orchestration.ts", import.meta.url), "utf8");

describe("the canvas reaches the real App shell", () => {
  it("has one door, in the palette", () => {
    // Shelved 2026-08-27, unshelved as far as the palette: the destination had
    // been reachable by NOTHING — no button, no chord, no palette row — while
    // the component, the host and the iface_* tools all still stood. One row
    // is the whole door; the toolbar button and the composer switch stay
    // withdrawn, because the runs still cost more than a swarm of one model.
    expect(app).toContain(
      'type CenterView = "sessions" | "canvas" | "terminal";');
    expect(app).not.toContain('aria-label="Open the agent canvas"');
    expect(readFileSync(new URL("./palette-commands.ts", import.meta.url), "utf8"))
      .toContain('{ id: "workspace.canvas", label: "Coordination canvas"');
    expect(app).toContain('case "workspace.canvas": showCanvas(); break;');
    expect(app).toContain("const showCanvas = useCallback(");
    expect(app).toContain("<AgentCanvas");
    // The coordination console the Plans surface used to host now stacks under
    // the graph, so collaboration keeps a home on the surface that owns agents.
    expect(app).toContain('<div className="canvas-coordination">{coordinationPanel}</div>');
    expect(existsSync(new URL("./components/AgentFeed.tsx", import.meta.url))).toBe(true);
  });

  it("keeps canvas positions renderer-local and hands the canvas the controller's intents", () => {
    expect(app).toContain("parseCanvasLayout(readStored(CANVAS_LAYOUT_KEY))");
    expect(app).toContain("writeStored(CANVAS_LAYOUT_KEY, JSON.stringify(next))");
    // Each intent re-reading the snapshot after the host answers is driven in
    // orchestration-controller.test.ts; App passes the controller's members.
    expect(app).toContain("useOrchestration(window.iface)");
    expect(app).toContain("onRegisterSession={orchestration.registerSession}");
    expect(app).toContain("onCreateWorker={orchestration.createAgent}");
    expect(app).toContain("onBindParent={orchestration.bindParent}");
    expect(app).toContain("onClearPolicy={orchestration.clearPolicy}");
    expect(app).toContain("onStopAgent={orchestration.stopAgent}");
    // Make orchestrator records the policy and nothing else. No run has an
    // orchestrator host, so every orchestrator tool refuses; switching the
    // lead to its engine's no-prompt mode would loosen approvals for nothing.
    expect(app).toContain("onSetPolicy={orchestration.setPolicy}");
  });

  it("routes the four coordinated-worker intents through the controller", () => {
    expect(app).toContain("onSubmitWorker={orchestration.submitWorker}");
    expect(app).toContain("onIntegrateWorker={orchestration.integrateWorker}");
    expect(app).toContain("onAbandonTask={orchestration.abandonTask}");
    expect(app).toContain("onAckObligation={orchestration.ackObligation}");
  });

  it("reloads coordination status on snapshot change, debounced into one read", () => {
    const effect = orchestrationHooks.match(
      /\/\/ Coordination state is a fact about the project[\s\S]*?\}, \[api, projectRoot, reads, enabled\]\);/u)?.[0] ?? "";
    expect(effect).toContain(
      "getCoordinationStatus({ canonicalProjectRoot: projectRoot })");
    expect(effect).toContain("setCoordination(null);");
    // Still no clock of its own: the read is triggered by the snapshot and only
    // delayed, so a burst of agent state changes makes one read, not one per
    // event. The card's own lease countdown remains timer-free.
    expect(effect).not.toContain("setInterval");
    expect(effect).toContain("}, COORDINATION_STATUS_DEBOUNCE_MS);");
    expect(effect).toContain("return () => { clearTimeout(timer); };");
    expect(orchestrationHooks).toContain("export const COORDINATION_STATUS_DEBOUNCE_MS = 500;");
    // Read only while the canvas is shown, and once each time it is shown again.
    expect(app).toContain("useCanvasReads(window.iface, activeCanvasRoot, orchestrationReads, canvasShown)");
    expect(app).toContain("coordination={coordinationStatus}");
  });

  it("re-reads the model catalogue as the snapshot refines it, and passes it down", () => {
    expect(orchestrationHooks).toContain("api.getOrchestratorModels()");
    // A later probeEngines run or a live models.available refinement must reach
    // the policy form's "Known ids" hint; reading once at launch leaves it stale.
    const effect = orchestrationHooks.match(
      /\/\/ The catalogue is refined by a later probe[\s\S]*?\}, \[api, reads, enabled\]\);/u)?.[0] ?? "";
    // An unchanged catalogue keeps the held object, so App does not re-render.
    expect(effect).toContain("setModels(current => equalValue(current, next) ? current : next);");
    // Debounced like its sibling: the read is triggered by the snapshot and only
    // delayed, so a burst of agent state changes runs one control round trip.
    expect(effect).not.toContain("setInterval");
    expect(effect).toContain("}, COORDINATION_STATUS_DEBOUNCE_MS);");
    expect(effect).toContain("return () => { clearTimeout(timer); };");
    expect(app).toContain("models={orchestratorModels}");
  });

  it("answers an integrate with the claim and says the verify is still running", () => {
    // Ruling R-L: the outcome arrives later as a worker notice, so the canvas
    // states a standing fact and never takes the single alert slot.
    expect(readFileSync(new URL("./orchestration-controller.ts", import.meta.url), "utf8"))
      .toContain("return { taskId: claim.taskId, state: \"verifying\" };");
    expect(source).toContain("setVerifying({ agentId, taskId: claim.taskId, at });");
    // Derived from the coordinator, never a flag the NEXT integrate resets.
    expect(source)
      .toContain("const verifyingNotice = canvasVerifyingNotice(props.snapshot, verifying);");
    expect(source).toContain("{verifyingNotice !== null");
  });

  it("gives the canvas each session's launched model and effort", () => {
    const summary = (over: Partial<SessionSummary>): SessionSummary => ({
      engine: "claude", sessionId: "one", title: "One", cwd: "/work/one", status: "idle",
      cloud: false, lastActivity: now, firstSeen: now, archived: false, ...over,
    });
    const first = canvasSessionRows(undefined, [
      summary({ model: "opus", effort: "high", projectRoot: "/work" }),
      summary({ sessionId: "two", title: "Two" }),
    ]);
    expect(first).toEqual([
      { key: "claude#one", title: "One", engine: "claude", cwd: "/work", model: "opus", effort: "high" },
      { key: "claude#two", title: "Two", engine: "claude", cwd: "/work/one" },
    ]);
    // A push that changes nothing the canvas shows keeps the list and rows.
    const pushed = canvasSessionRows(first, [
      summary({ model: "opus", effort: "high", projectRoot: "/work", status: "working",
        lastActivity: "2026-08-25T12:05:00.000Z" }),
      summary({ sessionId: "two", title: "Two" }),
    ]);
    expect(pushed).toBe(first);
    const renamed = canvasSessionRows(first, [
      summary({ model: "opus", effort: "high", projectRoot: "/work" }),
      summary({ sessionId: "two", title: "Renamed" }),
    ]);
    expect(renamed).not.toBe(first);
    expect(renamed[0]).toBe(first[0]);
    expect(renamed[1]).toMatchObject({ title: "Renamed" });
  });

  it("opens a node's conversation on the surface that owns it", () => {
    const opener = app.match(
      /const openCanvasSession = useCallback[\s\S]*?\}, \[mayOpenWorkspaceKey, setWorkspaceFor, showSessionWorkspace, showSurface\]\);/u)?.[0] ?? "";
    // The canvas is in Code, which holds no conversations: the pane opens in
    // the session's own Chat or Agent layout.
    expect(opener).toContain(
      "const owner = conversationSurfaceFor(sessionByKeyRef.current.get(key), surfaceRef.current);");
    expect(opener).toContain("showSurface(owner);");
    expect(opener).toContain("setWorkspaceFor(owner, current => openWorkspaceKey(current, key));");
    expect(opener).toContain("setWorkspaceFor(owner, current => switchWorkspaceKey(current, key));");
    expect(opener).toContain("showSessionWorkspace();");
    expect(app).toContain("onOpenSession={openCanvasSession}");
    // A Shift+Enter that hits the four-pane ceiling must change nothing at all,
    // so the guard runs before the selection dispatch, not after it.
    expect(opener.indexOf("if (newPane && !mayOpenWorkspaceKey(key, owner)) return;"))
      .toBeGreaterThan(-1);
    expect(opener.indexOf("if (newPane && !mayOpenWorkspaceKey(key, owner)) return;"))
      .toBeLessThan(opener.indexOf("showSurface(owner);"));
    expect(opener.indexOf("showSurface(owner);"))
      .toBeLessThan(opener.indexOf('dispatch({ type: "select", key });'));
  });
});
