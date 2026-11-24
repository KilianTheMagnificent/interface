import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { encodeRendererPayload } from "../main/ipc-payload";
import { CHANNELS, type EngineProbe, type TerminalLaunchProfile } from "../shared/ipc";
import { summarizeTerminalTools } from "./settings";

const fixture = JSON.parse(readFileSync(
  new URL("../fixtures/terminal-tools-v43.json", import.meta.url), "utf8",
)) as { requests: { refresh?: boolean; details?: boolean }[]; profiles: TerminalLaunchProfile[] };

const claudeProbe = (auth: "signedIn" | "signedOut"): EngineProbe => ({
  engine: "claude", ok: true, version: "2.1.300",
  info: {
    binaryPath: "/x", version: "2.1.300", loginIdentity: null, auth,
    capabilities: {
      streaming: true, approvals: true, resume: true, fork: true, cloudSessions: false,
      models: "dynamic", effortLevels: [], permissionModes: [],
    },
  },
});

describe("Settings' Terminal tools (C4, protocol 43)", () => {
  it("lists every launcher tool with its version, sign-in state and install hint", () => {
    const tools = summarizeTerminalTools(fixture.profiles, [claudeProbe("signedOut")]);
    expect(tools.map(tool => [tool.label, tool.healthLabel])).toEqual([
      ["Terminal", "Found"], ["Claude Code", "Signed out"], ["Codex", "Not installed"],
      ["OMP", "Not installed"],
    ]);
    expect(tools[0]).toMatchObject({ engine: null, detail: "Your login shell", missing: null });
    expect(tools[1]).toMatchObject({
      detail: "Version 2.1.300 (Claude Code)", auth: "Signed out", signIn: true,
    });
    expect(tools[2]).toMatchObject({
      detail: null, auth: null, signIn: false,
      missing: "Install it with `npm install -g @openai/codex`, then press Refresh.",
    });
    expect(summarizeTerminalTools(fixture.profiles, [claudeProbe("signedIn")])[1])
      .toMatchObject({ health: "ready", healthLabel: "Found", auth: "Signed in", signIn: false });
    // No probe answer: no sign-in claim either way.
    expect(summarizeTerminalTools(fixture.profiles, [])[1]).toMatchObject({ auth: null });
  });

  it("asks Rust for versions only when Settings lists the tools", () => {
    for (const request of fixture.requests) {
      expect(encodeRendererPayload(CHANNELS.listTerminalLaunchProfiles, [request])).toEqual(request);
    }
    expect(encodeRendererPayload(CHANNELS.listTerminalLaunchProfiles, [])).toEqual({});
    expect(encodeRendererPayload(CHANNELS.listTerminalLaunchProfiles, [{ details: false }])).toEqual({});
    expect(() => encodeRendererPayload(CHANNELS.listTerminalLaunchProfiles, [{ paths: true }])).toThrow();
    // probeEngines still takes only refresh.
    expect(() => encodeRendererPayload(CHANNELS.probeEngines, [{ details: true }])).toThrow();
  });
});

const paneToolsFixture = JSON.parse(readFileSync(
  new URL("../fixtures/terminal-pane-tools-v48.json", import.meta.url), "utf8",
)) as { requests: Record<string, unknown>[]; profiles: TerminalLaunchProfile[] };

describe("Interface's tools for Grok and Antigravity are opt-in (protocol 48)", () => {
  it("shows Settings' switch off until the owner turns it on, and locks a taken or unreadable one", () => {
    const tools = summarizeTerminalTools(paneToolsFixture.profiles, []);
    expect(tools.map(tool => tool.paneTools && [tool.paneTools.on, tool.paneTools.locked])).toEqual([
      [true, false], [false, false], [false, true], [false, true],
    ]);
    expect(tools[0]?.paneTools?.label).toBe("Let Grok use Interface's tools");
    expect(tools[1]?.paneTools?.note).toContain("Antigravity's own MCP config");
    expect(tools[2]?.paneTools?.note).toBe(
      "Grok already has its own MCP server named iface, so Interface leaves it alone.");
    // No switch on tools that keep no such entry, or on a missing CLI.
    expect(summarizeTerminalTools(fixture.profiles, []).every(tool => tool.paneTools === null)).toBe(true);
    expect(summarizeTerminalTools([{ ...paneToolsFixture.profiles[0]!, available: false }], [])[0]?.paneTools)
      .toBeNull();
  });

  it("sends the switch to Rust as one paneTools change", () => {
    for (const request of paneToolsFixture.requests) {
      expect(encodeRendererPayload(CHANNELS.listTerminalLaunchProfiles, [request])).toEqual(request);
    }
    for (const refused of [
      { paneTools: { profileId: "grok" } },
      { paneTools: { profileId: "grok", enabled: "yes" } },
      { paneTools: { profileId: "grok", enabled: true, path: "/x" } },
      { paneTools: "grok" },
    ]) {
      expect(() => encodeRendererPayload(CHANNELS.listTerminalLaunchProfiles, [refused])).toThrow();
    }
  });
});

const changeFixture = JSON.parse(readFileSync(
  new URL("../fixtures/terminal-tool-change-v51.json", import.meta.url), "utf8",
)) as { protocolVersion: number; profiles: TerminalLaunchProfile[] };

describe("Settings says what changed in a CLI since the last provider check (protocol 51)", () => {
  it("names the versions and what no longer matches, and says nothing without a change", () => {
    expect(changeFixture.protocolVersion).toBe(51);
    const tools = summarizeTerminalTools(changeFixture.profiles, []);
    expect(tools.map(tool => tool.changed)).toEqual([
      "Changed since last check: 2.1.300 → 2.1.301. What Interface depends on still matches.",
      "Changed since last check: 0.162.1 → 0.163.0. `codex exec --help` no longer lists "
        + "--ephemeral. Codex's app-server schema no longer declares turn/steer.",
      "Checked 1.0.31 against what Interface depends on. `grok --no-leader` is no longer "
        + "accepted: error: unexpected argument '--no-leader' found.",
      null,
    ]);
    // A CLI that is no longer installed has nothing to compare.
    expect(summarizeTerminalTools([{ ...changeFixture.profiles[0], available: false }], [])[0]
      .changed).toBeNull();
  });
});
