import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { RUST_HOST_PROTOCOL_VERSION } from "../main/rust-host-client";
import type { TerminalSummary, TerminalWorkspaceRecord } from "../shared/ipc";
import {
  placeHandedOffShell,
  SHELL_TERMINAL_PROFILE_ID,
  terminalRestartOffer,
} from "./terminal-shell-handoff";
import {
  EMPTY_TERMINAL_WORKSPACE_STATE,
  openTerminalTab,
  parseTerminalWorkspaceState,
  serializeTerminalWorkspaceState,
  terminalPaneForTerminal,
} from "./terminal-workspace";
import { reopenRequests } from "./terminal-workspaces";

const NOW = "2026-01-01T00:00:00.000Z";

function summary(overrides: Partial<TerminalSummary> = {}): TerminalSummary {
  return {
    id: "terminal-agent",
    generation: 1,
    title: "Claude Code",
    root: "/tmp/project",
    cwd: "/tmp/project/crates",
    state: "exited",
    size: { columns: 80, rows: 24 },
    createdAt: NOW,
    updatedAt: NOW,
    launch: { version: 1, profileId: "claude" },
    exit: { code: 0 },
    ...overrides,
  };
}

const shell = summary({
  id: "terminal-shell", title: "crates", state: "running", exit: undefined,
  launch: { version: 1, profileId: SHELL_TERMINAL_PROFILE_ID }, replaces: "terminal-agent",
});

function record(id: string, terminals: string[], layout = EMPTY_TERMINAL_WORKSPACE_STATE):
    TerminalWorkspaceRecord {
  return {
    id, name: id, createdAt: NOW, updatedAt: NOW,
    terminals: terminals.map(terminal => ({ id: terminal, title: terminal, cwd: "/tmp/project/crates" })),
    layout: serializeTerminalWorkspaceState(layout),
  };
}

describe("Rust's shell hand-off, reflected (protocol 50)", () => {
  it("puts the shell in the agent's own tab slot in the shown workspace and files it there", () => {
    let layout = openTerminalTab(EMPTY_TERMINAL_WORKSPACE_STATE, "pane-a", "terminal-agent");
    layout = openTerminalTab(layout, "pane-a", "terminal-other");
    const placed = placeHandedOffShell({
      shell, layout, records: [record("shown", ["terminal-agent", "terminal-other"])], now: () => NOW,
    });
    if (placed === null) throw new Error("the shell must take the agent's slot");
    expect(placed.layout.panes[0].tabs.map(tab => tab.terminalId))
      .toEqual(["terminal-shell", "terminal-other"]);
    expect(terminalPaneForTerminal(placed.layout, "terminal-agent")).toBeUndefined();
    expect(placed.records[0].terminals.map(entry => entry.id)).toEqual(["terminal-other", "terminal-shell"]);
    expect(placed.records[0].terminals[1].launch).toEqual({ version: 1, profileId: "shell" });
  });

  it("moves a parked workspace's saved layout without touching the shown one", () => {
    const shown = openTerminalTab(EMPTY_TERMINAL_WORKSPACE_STATE, "pane-shown", "terminal-elsewhere");
    const parked = openTerminalTab(EMPTY_TERMINAL_WORKSPACE_STATE, "pane-parked", "terminal-agent");
    const records = [record("shown", ["terminal-elsewhere"], shown), record("parked", ["terminal-agent"], parked)];
    const placed = placeHandedOffShell({ shell, layout: shown, records, now: () => NOW });
    if (placed === null) throw new Error("a parked agent's pane must be handed its shell too");
    expect(placed.layout).toBe(shown);
    expect(placed.records[0]).toBe(records[0]);
    const saved = parseTerminalWorkspaceState(placed.records[1].layout);
    expect(terminalPaneForTerminal(saved, "terminal-shell")?.id).toBe("pane-parked");
    expect(terminalPaneForTerminal(saved, "terminal-agent")).toBeUndefined();
    expect(placed.records[1].terminals.map(entry => entry.id)).toEqual(["terminal-shell"]);
  });

  it("reads the shell Rust sends as the shared v50 fixture spells it", () => {
    const fixture = JSON.parse(readFileSync(
      new URL("../fixtures/terminal-shell-handoff-v50.json", import.meta.url), "utf8",
    )) as { protocolVersion: number; shell: TerminalSummary };
    expect(RUST_HOST_PROTOCOL_VERSION).toBeGreaterThanOrEqual(fixture.protocolVersion);
    const agentId = fixture.shell.replaces!;
    const placed = placeHandedOffShell({
      shell: fixture.shell,
      layout: openTerminalTab(EMPTY_TERMINAL_WORKSPACE_STATE, "pane-a", agentId),
      records: [], now: () => NOW,
    });
    expect(placed?.layout.panes[0].activeTerminalId).toBe(fixture.shell.id);
  });

  it("never reopens a saved agent whose pane a live shell took", () => {
    const laidOut = openTerminalTab(
      openTerminalTab(EMPTY_TERMINAL_WORKSPACE_STATE, "pane-a", "terminal-agent"), "pane-a", "terminal-gone");
    const saved = record("shown", ["terminal-agent", "terminal-gone"], laidOut);
    expect(reopenRequests(saved, [shell], () => "minted").map(request => request.id))
      .toEqual(["terminal-gone"]);
  });

  it("does nothing once the move is made, for a terminal Rust did not hand off, or for a hidden agent", () => {
    const layout = openTerminalTab(EMPTY_TERMINAL_WORKSPACE_STATE, "pane-a", "terminal-shell");
    expect(placeHandedOffShell({
      shell, layout, records: [record("shown", ["terminal-shell"], layout)], now: () => NOW,
    })).toBeNull();
    expect(placeHandedOffShell({
      shell: { ...shell, replaces: undefined },
      layout: openTerminalTab(EMPTY_TERMINAL_WORKSPACE_STATE, "pane-a", "terminal-agent"),
      records: [], now: () => NOW,
    })).toBeNull();
    expect(placeHandedOffShell({
      shell, layout: EMPTY_TERMINAL_WORKSPACE_STATE, records: [record("other", ["terminal-x"])], now: () => NOW,
    })).toBeNull();
  });
});

/* Review A3 (2026-10-14): a crashed or failed agent pane was a dead end. The
   host already restarts a terminal under its restart proof; the pane offers it. */
describe("Start again on an ended agent pane", () => {
  it("offers a restart on every agent end the hand-off leaves alone", () => {
    expect(terminalRestartOffer(summary({ exit: { code: 1 } }), "claude")).toBe("Exited with code 1");
    expect(terminalRestartOffer(summary({ exit: { signal: 9 } }), "codex")).toBe("Ended by a signal");
    expect(terminalRestartOffer(summary({ state: "failed", exit: undefined }), "grok")).toBe("Did not start");
    // A clean exit Rust could not hand a shell (or one someone asked for) keeps its tab.
    expect(terminalRestartOffer(summary(), "claude")).toBe("Exited with code 0");
  });

  it("offers nothing to a live terminal or a plain shell", () => {
    expect(terminalRestartOffer(summary({ state: "running", exit: undefined }), "claude")).toBeNull();
    expect(terminalRestartOffer(summary({ exit: { code: 1 } }), SHELL_TERMINAL_PROFILE_ID)).toBeNull();
    expect(terminalRestartOffer(summary({ exit: { code: 1 } }), undefined)).toBeNull();
  });
});
