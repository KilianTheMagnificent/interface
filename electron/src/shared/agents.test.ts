import { describe, expect, it } from "vitest";
import fixture from "../fixtures/agent-workspace-v14.json";
import workFixture from "../fixtures/agent-work-v29.json";
import askFirstV71 from "../fixtures/ask-first-v71.json";
import fixtureV23 from "../fixtures/agent-workspace-v23.json";
import fixtureV32 from "../fixtures/agent-permission-modes-v32.json";
import fixtureV38 from "../fixtures/agent-task-start-v38.json";
import type { AgentCommand, AgentControlResponse, AgentWorkspace } from "./agents";
import type { AgentWorkSpec } from "./agent-work";
import { agentPermissionMode, emptyAgentControlResponse, permissionModeChange } from "./agents";
import { encodeRendererPayload } from "../main/ipc-payload";
import { CHANNELS } from "./ipc";
import { RUST_HOST_PROTOCOL_VERSION } from "../main/rust-host-client";
describe("durable agent contract", () => {
  it("passes complete versioned commands through the bridge without renderer-owned effects", () => {
    expect(RUST_HOST_PROTOCOL_VERSION).toBeGreaterThanOrEqual(fixture.protocolVersion);
    for (const command of fixture.commands) expect(encodeRendererPayload(CHANNELS.agentControl, [command as AgentCommand])).toEqual(command);
    const state: AgentWorkspace = { version: 1, revision: 0, agents: [], assignments: {}, routines: [], runs: [] };
    expect(Object.keys(state).sort()).toEqual(Object.keys(fixture.state).sort());
  });
});

// A key present for every member of the AgentCommand "kind" union. Missing a
// kind (or adding one that does not exist on the union) is a compile error,
// so this table double-checks the fixture against the TS contract itself.
const AGENT_COMMAND_KINDS = {
  calendar: true,
  work: true,
  read: true,
  importLegacy: true,
  saveAgent: true,
  removeAgent: true,
  assign: true,
  saveRoutine: true,
  removeRoutine: true,
  runNow: true,
  acknowledgeRun: true,
  startAgentThread: true,
  startAgentTask: true,
  newThread: true,
  markRead: true,
  readAgentFiles: true,
  saveAgentMemory: true,
  overwriteAgentInstructionsFile: true,
  saveAgentFolders: true,
  readAgentDecisions: true,
  recreateAgentHome: true,
  revealAgentHome: true,
  savePermissionMode: true,
} satisfies Record<AgentCommand["kind"], true>;

describe("agent teammates v23 contract", () => {
  it("carries every AgentCommand kind and RUST_HOST_PROTOCOL_VERSION covers it", () => {
    expect(RUST_HOST_PROTOCOL_VERSION).toBeGreaterThanOrEqual(fixtureV23.protocolVersion);
    const kinds = new Set([...fixtureV23.commands, ...fixtureV32.commands, ...workFixture.commands,
      ...fixtureV38.commands].map((command) => command.kind));
    for (const kind of Object.keys(AGENT_COMMAND_KINDS)) expect(kinds.has(kind)).toBe(true);
    for (const command of fixtureV23.commands) {
      expect(Object.keys(AGENT_COMMAND_KINDS)).toContain(command.kind);
      expect(encodeRendererPayload(CHANNELS.agentControl, [command as AgentCommand])).toEqual(command);
    }
  });

  it("returns an AgentControlResponse with statuses and pendingApprovals", () => {
    const response = fixtureV23.response as AgentControlResponse;
    expect(response.statuses).toBeTypeOf("object");
    expect(Array.isArray(response.pendingApprovals)).toBe(true);
    expect(response.version).toBe(2);
  });
});

it("transports every v27 work command unchanged", () => {
  for (const command of workFixture.commands) expect(encodeRendererPayload(CHANNELS.agentControl, [command as AgentCommand])).toEqual(command);
});

it("transports an agent's Search the web without asking switch unchanged (ask-first-v71.json)", () => {
  expect(RUST_HOST_PROTOCOL_VERSION).toBeGreaterThanOrEqual(askFirstV71.protocolVersion);
  for (const command of askFirstV71.agentCommands) {
    expect(encodeRendererPayload(CHANNELS.agentControl, [command as AgentCommand])).toEqual(command);
    const spec: AgentWorkSpec = (command as { command: { spec: AgentWorkSpec } }).command.spec;
    expect(spec.allowWebSearch).toBe(true);
  }
  // Absent in an older spec: searches ask.
  const older = workFixture.view.spec as AgentWorkSpec;
  expect(older.allowWebSearch).toBeUndefined();
});

describe("agent permission modes v32 contract", () => {
  it("transports savePermissionMode unchanged and the host version covers it", () => {
    expect(RUST_HOST_PROTOCOL_VERSION).toBeGreaterThanOrEqual(fixtureV32.protocolVersion);
    for (const command of fixtureV32.commands) {
      expect(encodeRendererPayload(CHANNELS.agentControl, [command as AgentCommand])).toEqual(command);
    }
  });

  it("reads permission modes from a control response", () => {
    const response = fixtureV32.response as AgentControlResponse;
    expect(agentPermissionMode(response, "research")).toBe("bypass");
    expect(agentPermissionMode(response, "someone-else")).toBe("gated");
    expect(agentPermissionMode(emptyAgentControlResponse(), "research")).toBe("gated");
  });
});

describe("agent task start v38 contract", () => {
  it("transports startAgentTask unchanged and the host version covers it", () => {
    expect(RUST_HOST_PROTOCOL_VERSION).toBeGreaterThanOrEqual(fixtureV38.protocolVersion);
    for (const command of fixtureV38.commands) {
      const typed: AgentCommand = command as Extract<AgentCommand, { kind: "startAgentTask" }>;
      expect(encodeRendererPayload(CHANNELS.agentControl, [typed])).toEqual(command);
    }
  });
});

it("sends a permission mode only when it changed", () => {
  expect(permissionModeChange("research", "gated", "gated")).toBeNull();
  expect(permissionModeChange("research", "auto", "auto")).toBeNull();
  expect(permissionModeChange("research", "gated", "bypass"))
    .toEqual({ kind: "savePermissionMode", agentId: "research", mode: "bypass" });
  expect(permissionModeChange("research", "bypass", "gated"))
    .toEqual({ kind: "savePermissionMode", agentId: "research", mode: "gated" });
});
