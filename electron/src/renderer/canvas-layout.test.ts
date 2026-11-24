import { describe, expect, it } from "vitest";
import type { AgentNodeV1 } from "../shared/orchestration";
import type { OrchestrationSnapshotDto } from "../shared/ipc";
import {
  CANVAS_COORDINATED_NODE_H,
  CANVAS_LAYOUT_KEY, CANVAS_NODE_SIZE, EMPTY_CANVAS_LAYOUT, canvasProjectRoot, clearNodes,
  deliveryEdgeState, edgePath, forgetNode, hiddenNodes, hideNode, moveNode, newestDelegation,
  orchestrationStatusWord, parseCanvasLayout, placeCanvasNodes, unhideNode,
} from "./canvas-layout";

const now = "2026-08-25T12:00:00.000Z";
const digest = "a".repeat(64);
const scope = { canonicalCwd: "/work/interface/worktrees/one",
  canonicalProjectRoot: "/work/interface", accessMode: "isolated", scopeDigest: digest };

function node(over: Partial<AgentNodeV1> = {}): AgentNodeV1 {
  return {
    version: 1, agentId: "agent-lead", engineId: "claude", sessionKey: "claude#lead",
    runtimeEpoch: "epoch-one", depth: 0, state: "idle",
    capabilities: [{ facet: "message", state: "available" }], scope,
    createdAt: now, updatedAt: now, ...over,
  };
}

function message(over: Record<string, unknown>): OrchestrationSnapshotDto["messages"][number] {
  return {
    version: 1, messageId: "message-one", threadId: "thread-one", origin: "agent",
    senderAgentId: "agent-lead", recipientAgentId: "agent-w1", expectReply: true,
    deliveryState: "settled", createdAt: now, updatedAt: now, ...over,
  } as OrchestrationSnapshotDto["messages"][number];
}

function snapshot(agents: AgentNodeV1[],
    messages: OrchestrationSnapshotDto["messages"] = []): OrchestrationSnapshotDto {
  return {
    version: 1, hostId: "host", runtimeEpoch: "epoch-one", revision: 1,
    createdAt: now, updatedAt: now, agents, messages, orchestratorPolicies: [],
    roleClaims: [], worktreeLeases: [],
    projection: { version: 1, truncated: false, eventFloorSequence: 0,
      omitted: { agents: 0, messages: 0, roleClaims: 0,
        worktreeLeases: 0, orchestratorPolicies: 0 } },
  } as OrchestrationSnapshotDto;
}

describe("parseCanvasLayout", () => {
  it("names one versioned renderer-local key", () => {
    expect(CANVAS_LAYOUT_KEY).toBe("interface.canvas.v1");
    expect(CANVAS_NODE_SIZE).toEqual({ w: 220, h: 96 });
  });

  it("turns every shape of garbage into the empty layout", () => {
    for (const raw of [null, undefined, "", "{", "[]", "3", [1, 2], 7, true,
      { version: 2, byProject: {} }, { byProject: {} }, { version: 1 },
      { version: 1, byProject: [] }]) {
      expect(parseCanvasLayout(raw)).toEqual(EMPTY_CANVAS_LAYOUT);
    }
  });

  it("keeps only finite integer coordinates and drops the rest of the record", () => {
    const parsed = parseCanvasLayout(JSON.stringify({
      version: 1,
      byProject: {
        "/work/interface": {
          "agent-lead": { x: 40.6, y: 40 },
          "agent-bad": { x: "40", y: 40 },
          "agent-nan": { x: Number.NaN, y: 0 },
          "agent-extra": { x: 1, y: 2, z: 3 },
          "": { x: 1, y: 1 },
        },
        "": { "agent-lead": { x: 1, y: 1 } },
      },
    }));
    expect(parsed).toEqual({
      version: 1,
      byProject: { "/work/interface": { nodes: {
        "agent-lead": { x: 41, y: 40 }, "agent-extra": { x: 1, y: 2 } }, hidden: [] } },
    });
  });

  it("never lets a stored key reach Object.prototype", () => {
    // Written as JSON text on purpose: a `__proto__` key in an object literal
    // sets the prototype instead of becoming the own property JSON.parse makes.
    const parsed = parseCanvasLayout('{"version":1,"byProject":{'
      + '"__proto__":{"agent-lead":{"x":1,"y":1}},'
      + '"/work/interface":{"nodes":{"__proto__":{"x":9,"y":9},'
      + '"constructor":{"x":8,"y":8},"prototype":{"x":7,"y":7},'
      + '"agent-lead":{"x":1,"y":2}},'
      + '"hidden":["__proto__","constructor","prototype","agent-w1"]}}}');
    expect(parsed).toEqual({ version: 1, byProject: { "/work/interface": {
      nodes: { "agent-lead": { x: 1, y: 2 } }, hidden: ["agent-w1"] } } });
    expect(Object.hasOwn(parsed.byProject, "__proto__")).toBe(false);
    expect(Object.getPrototypeOf(parsed.byProject)).toBe(Object.prototype);
    expect(Object.getPrototypeOf(parsed.byProject["/work/interface"].nodes))
      .toBe(Object.prototype);
    expect(moveNode(parsed, "__proto__", "agent-x", { x: 1, y: 1 })).toBe(parsed);
    expect(moveNode(parsed, "/work/interface", "__proto__", { x: 1, y: 1 })).toBe(parsed);
    expect(hideNode(parsed, "/work/interface", "constructor")).toBe(parsed);
  });

  it("accepts an already-parsed object as readily as its JSON text", () => {
    const value = { version: 1, byProject: { "/w": { nodes: { a: { x: 0, y: 0 } },
      hidden: ["b"] } } };
    expect(parseCanvasLayout(value)).toEqual(value);
    expect(parseCanvasLayout(JSON.stringify(value))).toEqual(value);
  });

  it("migrates the first stored shape, whose positions sat under the root", () => {
    expect(parseCanvasLayout({ version: 1, byProject: { "/w": { a: { x: 3, y: 4 } } } }))
      .toEqual({ version: 1, byProject: { "/w": { nodes: { a: { x: 3, y: 4 } },
        hidden: [] } } });
    // Hidden alone is a project worth keeping; neither is not.
    expect(parseCanvasLayout({ version: 1, byProject: { "/w": { nodes: {}, hidden: ["a"] } } }))
      .toEqual({ version: 1, byProject: { "/w": { nodes: {}, hidden: ["a"] } } });
    expect(parseCanvasLayout({ version: 1, byProject: { "/w": { nodes: {}, hidden: [] } } }))
      .toEqual(EMPTY_CANVAS_LAYOUT);
    expect(parseCanvasLayout({ version: 1, byProject: { "/w": { nodes: {},
      hidden: ["a", "a", 7, ""] } } }))
      .toEqual({ version: 1, byProject: { "/w": { nodes: {}, hidden: ["a"] } } });
  });

  it("clamps a coordinate far outside any reachable canvas", () => {
    expect(parseCanvasLayout({ version: 1, byProject: { "/w": {
      a: { x: 1e12, y: -1e12 } } } })).toEqual({
      version: 1, byProject: { "/w": { nodes: { a: { x: 100_000, y: -100_000 } },
        hidden: [] } } });
  });
});

describe("canvasProjectRoot", () => {
  it("prefers the canonical project root over an isolated worktree cwd", () => {
    expect(canvasProjectRoot(node())).toBe("/work/interface");
    expect(canvasProjectRoot(node({ scope: { canonicalCwd: "/solo", accessMode: "project",
      scopeDigest: digest } }))).toBe("/solo");
  });
});

describe("placeCanvasNodes", () => {
  const lead = node({ agentId: "agent-lead", role: "orchestrator" });
  const second = node({ agentId: "agent-second", role: "orchestrator" });
  const w1 = node({ agentId: "agent-w1", role: "worker", parentAgentId: "agent-lead", depth: 1 });
  const w2 = node({ agentId: "agent-w2", role: "worker", parentAgentId: "agent-lead", depth: 1 });
  const loose = node({ agentId: "agent-loose" });

  it("puts orchestrators on the top row and their children beneath, in agentId order", () => {
    expect(placeCanvasNodes([w2, lead, w1], {})).toEqual({
      "agent-lead": { x: 40, y: 40 },
      "agent-w1": { x: 40, y: 200 },
      "agent-w2": { x: 320, y: 200 },
    });
  });

  it("is independent of input order and puts unlinked agents to the right", () => {
    const first = placeCanvasNodes([loose, second, lead], {});
    const again = placeCanvasNodes([lead, loose, second], {});
    expect(first).toEqual(again);
    expect(first).toEqual({
      "agent-lead": { x: 40, y: 40 },
      "agent-second": { x: 320, y: 40 },
      "agent-loose": { x: 600, y: 40 },
    });
  });

  it("lets a stored position win and ignores positions for absent agents", () => {
    expect(placeCanvasNodes([lead, w1], {
      "agent-w1": { x: 900, y: 512 }, "agent-gone": { x: 1, y: 1 },
    })).toEqual({ "agent-lead": { x: 40, y: 40 }, "agent-w1": { x: 900, y: 512 } });
  });

  it("drops the worker row when the row above it carries a coordinated card", () => {
    // The coordinated card grows past CANVAS_NODE_SIZE.h (minHeight, not height),
    // so laying the row below out on the fixed height lets the two overlap.
    expect(CANVAS_COORDINATED_NODE_H).toBeGreaterThan(CANVAS_NODE_SIZE.h);
    expect(placeCanvasNodes([w1, lead], {}, new Set(["agent-lead"]))).toEqual({
      "agent-lead": { x: 40, y: 40 },
      "agent-w1": { x: 40, y: 40 + CANVAS_COORDINATED_NODE_H + 64 },
    });
    // A coordinated card on the bottom row has nothing beneath it to push down.
    expect(placeCanvasNodes([w1, lead], {}, new Set(["agent-w1"])))
      .toEqual(placeCanvasNodes([w1, lead], {}));
  });

  it("treats a child whose parent is not on this canvas as unlinked", () => {
    expect(placeCanvasNodes([node({ agentId: "agent-orphan", parentAgentId: "gone", depth: 1 })],
      {})).toEqual({ "agent-orphan": { x: 40, y: 40 } });
  });
});

describe("edgePath", () => {
  it("runs a cubic Bezier from the bottom-centre port to the top-centre port", () => {
    expect(edgePath({ x: 0, y: 0 }, { x: 280, y: 200 }))
      .toBe("M 110 96 C 110 148 390 148 390 200");
  });

  it("clamps the bend to the gap so near nodes draw a link, not an S-loop", () => {
    // The old minimum bend put the first control point 60px BELOW the second.
    expect(edgePath({ x: 0, y: 0 }, { x: 0, y: 100 }))
      .toBe("M 110 96 C 110 98 110 98 110 100");
    expect(edgePath({ x: 0, y: 0 }, { x: 280, y: 96 }))
      .toBe("M 110 96 C 110 96 390 96 390 96");
    // A worker dragged above its orchestrator gets a straight line, not a loop.
    expect(edgePath({ x: 0, y: 200 }, { x: 0, y: 0 }))
      .toBe("M 110 296 C 110 296 110 0 110 0");
  });
});

describe("deliveryEdgeState", () => {
  // The canvas draws each edge from its worker's newest delegation.
  const edgeVisualState = (value: Parameters<typeof newestDelegation>[0], workerAgentId: string) =>
    deliveryEdgeState(newestDelegation(value, workerAgentId));
  const lead = node({ agentId: "agent-lead", role: "orchestrator" });
  const w1 = node({ agentId: "agent-w1", parentAgentId: "agent-lead", depth: 1 });

  it("is idle with no message, no parent, or no such worker", () => {
    expect(edgeVisualState(snapshot([lead, w1]), "agent-w1")).toBe("idle");
    expect(edgeVisualState(snapshot([lead]), "agent-lead")).toBe("idle");
    expect(edgeVisualState(snapshot([lead, w1]), "agent-missing")).toBe("idle");
    expect(edgeVisualState(null, "agent-w1")).toBe("idle");
  });

  it("maps every receipt state the projection can carry", () => {
    for (const [deliveryState, expected] of [
      ["requested", "inflight"], ["queued", "inflight"], ["provider_accepted", "inflight"],
      ["settled", "settled"], ["processed", "settled"],
      ["failed", "failed"], ["unknown", "unknown"],
    ] as const) {
      expect(edgeVisualState(snapshot([lead, w1], [message({ deliveryState })]), "agent-w1"))
        .toBe(expected);
    }
  });

  it("reads the newest message from the worker's own parent and ignores the others", () => {
    const messages = [
      message({ messageId: "m-old", deliveryState: "settled",
        createdAt: "2026-08-25T10:00:00.000Z" }),
      message({ messageId: "m-other-sender", senderAgentId: "agent-else",
        deliveryState: "failed", createdAt: "2026-08-25T18:00:00.000Z" }),
      message({ messageId: "m-other-worker", recipientAgentId: "agent-w9",
        deliveryState: "failed", createdAt: "2026-08-25T19:00:00.000Z" }),
      message({ messageId: "m-new", deliveryState: "queued",
        createdAt: "2026-08-25T14:00:00.000Z" }),
    ];
    expect(edgeVisualState(snapshot([lead, w1], messages), "agent-w1")).toBe("inflight");
    expect(edgeVisualState(snapshot([lead, w1], [...messages].reverse()), "agent-w1"))
      .toBe("inflight");
  });
});

describe("newestDelegation", () => {
  const lead = node({ agentId: "agent-lead", role: "orchestrator" });
  const w1 = node({ agentId: "agent-w1", parentAgentId: "agent-lead", depth: 1 });

  it("hands back the newest message the worker's own parent sent it", () => {
    const messages = [
      message({ messageId: "m-old", createdAt: "2026-08-25T10:00:00.000Z" }),
      message({ messageId: "m-new", deliveryState: "queued",
        createdAt: "2026-08-25T14:00:00.000Z" }),
      message({ messageId: "m-other", senderAgentId: "agent-else",
        createdAt: "2026-08-25T19:00:00.000Z" }),
    ];
    expect(newestDelegation(snapshot([lead, w1], messages), "agent-w1")?.messageId)
      .toBe("m-new");
  });

  it("is null with no snapshot, no parent or no message", () => {
    expect(newestDelegation(null, "agent-w1")).toBeNull();
    expect(newestDelegation(snapshot([lead]), "agent-lead")).toBeNull();
    expect(newestDelegation(snapshot([lead, w1]), "agent-w1")).toBeNull();
  });
});

describe("moveNode, forgetNode, hideNode and unhideNode", () => {
  const base = { version: 1 as const,
    byProject: { "/work/interface": { nodes: { "agent-lead": { x: 40, y: 40 } },
      hidden: [] } } };

  it("returns a new state and never mutates the one it was given", () => {
    const next = moveNode(base, "/work/interface", "agent-w1", { x: 320.4, y: 200 });
    expect(next).not.toBe(base);
    expect(next.byProject).not.toBe(base.byProject);
    expect(next.byProject["/work/interface"]).not.toBe(base.byProject["/work/interface"]);
    expect(base.byProject["/work/interface"].nodes)
      .toEqual({ "agent-lead": { x: 40, y: 40 } });
    expect(next.byProject["/work/interface"].nodes).toEqual({
      "agent-lead": { x: 40, y: 40 }, "agent-w1": { x: 320, y: 200 } });
  });

  it("opens a project it has never seen and refuses an empty identity or coordinate", () => {
    expect(moveNode(base, "/other", "agent-x", { x: 1, y: 2 }).byProject["/other"])
      .toEqual({ nodes: { "agent-x": { x: 1, y: 2 } }, hidden: [] });
    expect(moveNode(base, "", "agent-x", { x: 1, y: 2 })).toBe(base);
    expect(moveNode(base, "/work/interface", "", { x: 1, y: 2 })).toBe(base);
    expect(moveNode(base, "/work/interface", "agent-x", { x: Number.NaN, y: 2 })).toBe(base);
  });

  it("forgets one node without disturbing the rest or the input", () => {
    const two = moveNode(base, "/work/interface", "agent-w1", { x: 1, y: 1 });
    const gone = forgetNode(two, "/work/interface", "agent-w1");
    expect(gone.byProject["/work/interface"].nodes)
      .toEqual({ "agent-lead": { x: 40, y: 40 } });
    expect(two.byProject["/work/interface"].nodes).toHaveProperty("agent-w1");
    expect(forgetNode(base, "/work/interface", "agent-absent")).toBe(base);
    expect(forgetNode(base, "/nowhere", "agent-lead")).toBe(base);
  });

  it("hides a node and drops the position it was hidden from", () => {
    const hiddenState = hideNode(base, "/work/interface", "agent-lead");
    expect(hiddenNodes(hiddenState, "/work/interface")).toEqual(["agent-lead"]);
    expect(hiddenState.byProject["/work/interface"].nodes).toEqual({});
    // The input is untouched, and hiding twice changes nothing.
    expect(hiddenNodes(base, "/work/interface")).toEqual([]);
    expect(hideNode(hiddenState, "/work/interface", "agent-lead")).toBe(hiddenState);
  });

  it("hides a node this canvas has never placed", () => {
    expect(hiddenNodes(hideNode(base, "/other", "agent-x"), "/other")).toEqual(["agent-x"]);
  });

  it("un-hides only what is hidden and keeps the other positions", () => {
    const hiddenState = hideNode(
      moveNode(base, "/work/interface", "agent-w1", { x: 9, y: 9 }),
      "/work/interface", "agent-lead");
    const back = unhideNode(hiddenState, "/work/interface", "agent-lead");
    expect(hiddenNodes(back, "/work/interface")).toEqual([]);
    expect(back.byProject["/work/interface"].nodes).toEqual({ "agent-w1": { x: 9, y: 9 } });
    expect(unhideNode(base, "/work/interface", "agent-lead")).toBe(base);
    expect(unhideNode(base, "/nowhere", "agent-lead")).toBe(base);
  });

  it("auto-arrange drops every position and keeps what the human removed", () => {
    const hiddenState = hideNode(base, "/work/interface", "agent-gone");
    const arranged = clearNodes(hiddenState, "/work/interface");
    expect(arranged.byProject["/work/interface"])
      .toEqual({ nodes: {}, hidden: ["agent-gone"] });
    expect(clearNodes(base, "")).toBe(base);
  });
});

describe("orchestration status words", () => {
  it("asks status-words.ts for the four states a conversation also has", () => {
    expect(orchestrationStatusWord("waiting")).toBe("Needs attention");
    expect(orchestrationStatusWord("working")).toBe("Working");
    expect(orchestrationStatusWord("idle")).toBe("Idle");
    expect(orchestrationStatusWord("failed")).toBe("Failed");
  });

  it("names the six an orchestrated agent has and a conversation does not", () => {
    expect(orchestrationStatusWord("planned")).toBe("Planned");
    expect(orchestrationStatusWord("starting")).toBe("Starting");
    expect(orchestrationStatusWord("ready")).toBe("Ready");
    expect(orchestrationStatusWord("completed")).toBe("Completed");
    expect(orchestrationStatusWord("cancelled")).toBe("Cancelled");
    expect(orchestrationStatusWord("unknown")).toBe("Unknown");
  });

  it("shows an unrecognised token rather than collapsing it onto Unknown", () => {
    // "Unknown" is the host's own state (above). Answering it for a word this
    // build has never heard of throws away the only thing that word carried,
    // and makes a newer host's real state indistinguishable from ignorance.
    expect(orchestrationStatusWord("quarantined")).toBe("Quarantined");
    expect(orchestrationStatusWord("needs-review")).toBe("Needs-review");
    expect(orchestrationStatusWord("")).toBe("Unknown");
  });
});
