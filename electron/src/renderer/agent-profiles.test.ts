import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  AGENT_COLORS,
  AGENT_PROFILES_STORAGE_KEY,
  AGENT_PROFILES_VERSION,
  MAX_AGENT_ASSIGNMENTS,
  MAX_AGENT_PROFILES,
  agentForSession,
  agentTaskCommand,
  newAgentId,
  nextAgentColor,
  normalizeAgentProfileStore,
  parseAgentProfiles,
  readAgentProfiles,
  serializeAgentProfiles,
  type AgentProfile,
  type AgentProfileStore,
} from "./agent-profiles";

const agent = (over: Partial<AgentProfile> = {}): AgentProfile => ({
  id: "a1",
  name: "Refactorer",
  engine: "codex",
  model: "gpt-5.6-sol",
  effort: "high",
  instructions: "Prefer the smallest diff.",
  color: "teal",
  ...over,
});

const store = (over: Partial<AgentProfileStore> = {}): AgentProfileStore => ({
  agents: [agent()],
  assignments: { "codex#one": "a1" },
  ...over,
});

describe("agent profiles", () => {
  it("round trips through the versioned envelope", () => {
    expect(parseAgentProfiles(serializeAgentProfiles(store()))).toEqual(store());
  });

  it("fails closed on corrupt, foreign and wrong-version storage", () => {
    const empty = { agents: [], assignments: {} };
    expect(parseAgentProfiles(null)).toEqual(empty);
    expect(parseAgentProfiles("")).toEqual(empty);
    expect(parseAgentProfiles("{not json")).toEqual(empty);
    expect(parseAgentProfiles(JSON.stringify({ agents: [agent()] }))).toEqual(empty);
    expect(parseAgentProfiles(JSON.stringify({
      version: AGENT_PROFILES_VERSION + 1, agents: [agent()], assignments: {},
    }))).toEqual(empty);
  });

  it("normalizes fields, drops unusable agents and stays bounded", () => {
    const normalized = normalizeAgentProfileStore({
      agents: [
        agent({ name: "  Refactorer  ", engine: " codex " }),
        agent({ id: "dup", name: "x".repeat(61) }),
        agent({ id: "", name: "No id" }),
        agent({ id: "b2", name: "No engine", engine: "" }),
        agent({ id: "a1", name: "Duplicate id" }),
        agent({ id: "c3", name: "Repairable", model: "m".repeat(513),
          instructions: "i".repeat(8001), color: "chartreuse" as AgentProfile["color"] }),
        ...Array.from({ length: MAX_AGENT_PROFILES + 5 }, (_, index) =>
          agent({ id: `bulk-${index}`, name: `Bulk ${index}` })),
      ],
      assignments: { "codex#one": "a1", "codex#two": "gone", "": "a1" },
    });

    expect(normalized.agents[0]).toEqual(agent());
    expect(normalized.agents[1]).toEqual(agent({
      id: "c3", name: "Repairable", model: "", instructions: "", color: AGENT_COLORS[0],
    }));
    expect(normalized.agents).toHaveLength(MAX_AGENT_PROFILES);
    expect(normalized.assignments).toEqual({ "codex#one": "a1" });
  });

  it("keeps only the last assignments in object order", () => {
    const assignments: Record<string, string> = {};
    for (let index = 0; index < MAX_AGENT_ASSIGNMENTS + 3; index += 1) {
      assignments[`codex#${index}`] = "a1";
    }
    const current = normalizeAgentProfileStore({ agents: [agent()], assignments });

    const keys = Object.keys(current.assignments);
    expect(keys).toHaveLength(MAX_AGENT_ASSIGNMENTS);
    expect(keys[0]).toBe("codex#3");
    expect(keys.at(-1)).toBe(`codex#${MAX_AGENT_ASSIGNMENTS + 2}`);
  });

  it("reads a session's agent back", () => {
    const two = store({
      agents: [agent({ name: "Reviewer" }), agent({ id: "b2", name: "Docs" })],
      assignments: { "codex#one": "a1", "codex#two": "b2" },
    });
    expect(agentForSession(two, "codex#one")?.name).toBe("Reviewer");
    expect(agentForSession(two, "codex#two")?.name).toBe("Docs");
    expect(agentForSession(two, "codex#nothing")).toBeUndefined();
    // An assignment whose agent is gone resolves to nobody.
    expect(agentForSession(store({ agents: [] }), "codex#one")).toBeUndefined();
  });

  it("prefers unused characters after edits or deletions and mints unique ids", () => {
    expect(nextAgentColor({ agents: [], assignments: {} })).toBe(AGENT_COLORS[0]);
    expect(nextAgentColor(store())).toBe(AGENT_COLORS[1]);
    expect(nextAgentColor(store({ agents: [agent({ color: "coral" }), agent({ id: "a2", color: "sky" })] })))
      .toBe("teal");
    expect(newAgentId()).not.toBe(newAgentId());
    expect(newAgentId().length).toBeLessThanOrEqual(64);
  });

  it("reads the legacy store through a boundary that never throws", () => {
    const stored: Record<string, string> = {
      [AGENT_PROFILES_STORAGE_KEY]: serializeAgentProfiles(store()),
    };
    expect(readAgentProfiles({ getItem: key => stored[key] ?? null })).toEqual(store());
    expect(readAgentProfiles({ getItem: () => null })).toEqual({ agents: [], assignments: {} });
    expect(readAgentProfiles({ getItem: () => { throw new Error("blocked"); } }))
      .toEqual({ agents: [], assignments: {} });
    expect(AGENT_PROFILES_STORAGE_KEY).toBe("interface.agentProfiles.v1");
  });
});

describe("a teammate's New task goes through its gated start (review A6)", () => {
  it("sends only the owner's words, folder and launch choices for Rust to gate", () => {
    const agent = { id: "research", engine: "claude" };
    expect(agentTaskCommand(agent, { engine: "claude", cwd: "/work/app", model: "opus", effort: "high" },
      "Fix the flake\nand explain it", "Fix the flake\nand explain it\n\n<context/>", "req-1")).toEqual({
      kind: "startAgentTask", agentId: "research", text: "Fix the flake\nand explain it\n\n<context/>",
      cwd: "/work/app", requestId: "req-1", title: "Fix the flake", model: "opus", effort: "high",
    });
    // A model picked for another provider, or the provider's own default, is not sent.
    expect(agentTaskCommand(agent, { engine: "codex", cwd: "/w", model: "gpt-6", effort: "high" },
      "Go", "Go", "r2")).toEqual({
      kind: "startAgentTask", agentId: "research", text: "Go", cwd: "/w", requestId: "r2", title: "Go",
    });
    expect(agentTaskCommand(agent, { engine: "claude", cwd: "/w", model: "default", effort: "default" },
      "  ", "Go", "r3")).toEqual({ kind: "startAgentTask", agentId: "research", text: "Go", cwd: "/w", requestId: "r3" });
  });

  it("no longer prefixes the job or files the task itself in App", () => {
    const app = readFileSync(new URL("./App.tsx", import.meta.url), "utf8");
    const launch = app.slice(app.indexOf("const launch = useCallback"), app.indexOf("/* ---- turn-bound checkpoints"));
    expect(launch).toContain("startAgentTask({ ...agentTaskCommand(agent, opts, opts.prompt, request.prompt,");
    expect(launch).toContain("recovery: request.recovery");
    expect(launch).not.toContain("agentLaunchPrompt(");
    expect(launch).not.toContain('kind: "assign"');
  });
});
