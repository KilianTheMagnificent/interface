import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { OrchestrationSnapshotDto } from "../../shared/ipc";
import {
  AgentFeed, agentFeedRows, elapsedLabel, type AgentFeedRow,
} from "./AgentFeed";

type FeedAgent = OrchestrationSnapshotDto["agents"][number];

function agent(over: Partial<FeedAgent> & { agentId: string }): FeedAgent {
  return {
    version: 1,
    engineId: "claude",
    runtimeEpoch: "runtime_test",
    depth: 0,
    state: "working",
    capabilities: [],
    scope: { canonicalCwd: "/work/interface", accessMode: "project",
      scopeDigest: "d".repeat(64) },
    createdAt: "2026-08-26T00:00:00.000Z",
    updatedAt: "2026-08-26T00:00:00.000Z",
    ...over,
  } as FeedAgent;
}

function snapshot(agents: FeedAgent[]): OrchestrationSnapshotDto {
  return { agents } as OrchestrationSnapshotDto;
}

const row = (over: Partial<AgentFeedRow> & { agentId: string }): AgentFeedRow => ({
  name: "Builder",
  engineId: "claude",
  state: "working",
  worker: true,
  createdAt: "2026-08-26T00:00:00.000Z",
  updatedAt: "2026-08-26T00:00:00.000Z",
  ...over,
});

const markup = (rows: readonly AgentFeedRow[], over: Partial<{ hidden: boolean }> = {}): string =>
  renderToStaticMarkup(createElement(AgentFeed, {
    rows, onOpen: () => undefined, ...over,
  }));

describe("elapsedLabel", () => {
  it("reads as a duration a human says out loud", () => {
    expect(elapsedLabel(0)).toBe("0s");
    expect(elapsedLabel(12_400)).toBe("12s");
    expect(elapsedLabel(134_000)).toBe("2m14s");
    expect(elapsedLabel(3_840_000)).toBe("1h04m");
    // A clock that ran backwards (a settle stamped before its create) is a
    // display problem, never a negative duration.
    expect(elapsedLabel(-5_000)).toBe("0s");
  });
});

describe("agentFeedRows", () => {
  it("drops everything that is not running right now", () => {
    // The durable record keeps every agent this host has ever run, including
    // ones an earlier app run quarantined as `unknown`. A feed showing those
    // was a list of yesterday's sessions that never emptied.
    const rows = agentFeedRows(snapshot([
      agent({ agentId: "agent_live", name: "Builder", state: "working" }),
      agent({ agentId: "agent_waiting", name: "Blocked", state: "waiting" }),
      agent({ agentId: "agent_done", name: "Finished", state: "idle" }),
      agent({ agentId: "agent_failed", name: "Broken", state: "failed" }),
      agent({ agentId: "agent_stale", name: "Yesterday", state: "unknown" }),
    ]));
    expect(rows.map(item => item.name)).toEqual(["Builder", "Blocked"]);
  });

  it("puts the newest spawn first and marks workers", () => {
    const rows = agentFeedRows(snapshot([
      agent({ agentId: "agent_lead", name: "Lead", role: "orchestrator",
        createdAt: "2026-08-26T00:00:00.000Z" }),
      agent({ agentId: "agent_w1", name: "Builder", role: "worker",
        parentAgentId: "agent_lead", createdAt: "2026-08-26T00:05:00.000Z" }),
    ]));
    expect(rows.map(item => item.agentId)).toEqual(["agent_w1", "agent_lead"]);
    expect(rows[0]).toMatchObject({ name: "Builder", worker: true });
    // An orchestrator is never a worker, even though the canvas nests it.
    expect(rows[1]).toMatchObject({ name: "Lead", worker: false });
  });

  it("empties, and so disappears, once the last agent settles", () => {
    const settled = agentFeedRows(snapshot([
      agent({ agentId: "agent_done", state: "idle" }),
      agent({ agentId: "agent_stale", state: "unknown" }),
    ]));
    expect(settled).toEqual([]);
    expect(markup(settled)).toBe("");
  });

  it("falls back to the agent id and answers an absent snapshot", () => {
    expect(agentFeedRows(null)).toEqual([]);
    expect(agentFeedRows(snapshot([agent({ agentId: "agent_anon" })]))[0]?.name)
      .toBe("agent_anon");
  });
});

describe("AgentFeed", () => {
  it("renders nothing at all when there is no agent to report", () => {
    expect(markup([])).toBe("");
  });

  it("stands aside for a modal surface rather than floating over it", () => {
    expect(markup([row({ agentId: "agent_w1" })], { hidden: true })).toBe("");
  });

  it("counts what is running, and names each row", () => {
    const out = markup([
      row({ agentId: "agent_w1", name: "Builder", state: "working",
        sessionKey: "claude#w1" }),
      row({ agentId: "agent_w2", name: "Scanner", state: "working", engineId: "codex" }),
    ]);
    expect(out).toContain('aria-label="Running agents"');
    // Every row is live, so one number is the whole story.
    expect(out).toContain(">2<");
    // The row says the app's word for the state, not the host's token.
    expect(out).toContain('aria-label="Builder, claude, Working"');
    // A row with no provider session has nothing to open.
    expect(out).toContain('aria-label="Scanner, codex, Working" disabled=""');
  });

  it("separates the one state that is a request from the ones that are not", () => {
    const out = markup([
      row({ agentId: "agent_w1", state: "working" }),
      row({ agentId: "agent_w2", state: "waiting" }),
      row({ agentId: "agent_w3", state: "starting" }),
    ]);
    expect(out).toContain("agent-feed-dot agent-feed-waiting");
    expect((out.match(/agent-feed-dot agent-feed-working/gu) ?? []).length).toBe(2);
  });

  it("paints from theme tokens only", () => {
    const out = markup([row({ agentId: "agent_w1" })]);
    expect(out).toContain("var(--status-working)");
    expect(out).toContain("var(--menu-bg)");
    expect(out).not.toMatch(/#[0-9a-f]{3,8}\b/iu);
  });
});
