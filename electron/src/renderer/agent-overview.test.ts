import { readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { AgentOverview } from "./components/AgentOverview";
import {
  agentOverviewCounts, agentOverviewElapsed, agentOverviewGroups, agentOverviewMatches,
  agentOverviewProjectGroups, agentOverviewRows, agentOverviewSince, agentOverviewStatus,
  agentOverviewSummary, agentOverviewWorkspace, isAgentProfile, savedAgentOverviewWorkspace,
  type AgentOverviewTerminal, type AgentOverviewWorkspace,
} from "./agent-overview";
import type { TerminalWorkspaceRecord } from "../shared/ipc";
import {
  EMPTY_TERMINAL_WORKSPACE_STATE, openTerminalTab, serializeTerminalWorkspaceState,
} from "./terminal-workspace";

function terminal(id: string, patch: Partial<AgentOverviewTerminal> = {}): AgentOverviewTerminal {
  return { id, title: `Title ${id}`, state: "running", profileId: "claude", ...patch };
}

function workspace(id: string, terminalIds: readonly string[],
    attached: readonly string[] = terminalIds, name = `Workspace ${id}`): AgentOverviewWorkspace {
  return { id, name, terminalIds, attachedIds: new Set(attached) };
}

function byId(...terminals: AgentOverviewTerminal[]): ReadonlyMap<string, AgentOverviewTerminal> {
  return new Map(terminals.map(entry => [entry.id, entry]));
}

describe("agentOverviewStatus", () => {
  it("folds host state and live activity into one of four groups", () => {
    expect(agentOverviewStatus("running", "waiting")).toBe("waiting");
    expect(agentOverviewStatus("running", "working")).toBe("working");
    expect(agentOverviewStatus("starting", undefined)).toBe("working");
    expect(agentOverviewStatus("running", "idle")).toBe("idle");
    expect(agentOverviewStatus("running", undefined)).toBe("idle");
    expect(agentOverviewStatus("exited", "waiting")).toBe("stopped");
    expect(agentOverviewStatus("failed", "working")).toBe("stopped");
  });

  it("calls a resting agent with an unseen finished turn done, and nothing busier", () => {
    expect(agentOverviewStatus("running", "idle", true)).toBe("done");
    expect(agentOverviewStatus("running", undefined, true)).toBe("done");
    expect(agentOverviewStatus("running", "working", true)).toBe("working");
    expect(agentOverviewStatus("running", "waiting", true)).toBe("waiting");
    expect(agentOverviewStatus("exited", "idle", true)).toBe("stopped");
  });
});

describe("isAgentProfile", () => {
  it("counts launch profiles and leaves plain shells out", () => {
    expect(isAgentProfile("claude")).toBe(true);
    expect(isAgentProfile("codex")).toBe(true);
    expect(isAgentProfile("shell")).toBe(false);
    expect(isAgentProfile(undefined)).toBe(false);
    expect(isAgentProfile("")).toBe(false);
  });
});

describe("agentOverviewRows", () => {
  it("lists agent panes across every workspace in saved order", () => {
    const rows = agentOverviewRows([
      workspace("w1", ["a", "b"], ["a", "b"], "api"),
      workspace("w2", ["c"], ["c"], "site"),
    ], byId(
      terminal("a", { activity: "working" }),
      terminal("b", { profileId: "codex" }),
      terminal("c", { activity: "waiting", sessionKey: "claude#s1" }),
    ));
    expect(rows.map(row => [row.terminalId, row.workspaceId, row.workspaceName, row.status]))
      .toEqual([["a", "w1", "api", "working"], ["b", "w1", "api", "idle"],
        ["c", "w2", "site", "waiting"]]);
    expect(rows[1]!.profileId).toBe("codex");
    expect(rows[2]!.sessionKey).toBe("claude#s1");
  });

  it("leaves out plain shells and terminals the host no longer lists", () => {
    const rows = agentOverviewRows([workspace("w1", ["a", "shell", "gone", "bare"])], byId(
      terminal("a"), terminal("shell", { profileId: "shell" }),
      terminal("bare", { profileId: undefined }),
    ));
    expect(rows.map(row => row.terminalId)).toEqual(["a"]);
  });

  it("keeps a running agent whose tab was closed, but not a finished one", () => {
    const rows = agentOverviewRows([workspace("w1", ["open", "parked", "done", "shown-done"],
      ["open", "shown-done"])], byId(
      terminal("open"), terminal("parked"),
      terminal("done", { state: "exited" }), terminal("shown-done", { state: "failed" }),
    ));
    expect(rows.map(row => [row.terminalId, row.status]))
      .toEqual([["open", "idle"], ["parked", "idle"], ["shown-done", "stopped"]]);
  });

  it("names a terminal once even if two workspaces claim it", () => {
    const rows = agentOverviewRows([workspace("w1", ["a"]), workspace("w2", ["a"])],
      byId(terminal("a")));
    expect(rows.map(row => row.workspaceId)).toEqual(["w1"]);
  });

  it("falls back to a readable title for an untitled terminal", () => {
    const [row] = agentOverviewRows([workspace("w1", ["a"])], byId(terminal("a", { title: "  " })));
    expect(row!.title).toBe("Terminal");
  });
});

describe("agentOverviewCounts and groups", () => {
  const rows = agentOverviewRows([workspace("w1", ["a", "b", "c", "d", "e", "f"])], byId(
    terminal("a", { activity: "idle" }), terminal("b", { activity: "working" }),
    terminal("c", { activity: "waiting" }), terminal("d", { activity: "idle" }),
    terminal("e", { state: "exited" }), terminal("f", { activity: "idle", done: true }),
  ));

  it("counts every status and the total", () => {
    expect(agentOverviewCounts(rows))
      .toEqual({ all: 6, waiting: 1, working: 1, done: 1, idle: 2, stopped: 1 });
  });

  it("orders groups by urgency and drops empty ones", () => {
    expect(agentOverviewGroups(rows, "all").map(group => [group.status, group.rows.length]))
      .toEqual([["waiting", 1], ["working", 1], ["done", 1], ["idle", 2], ["stopped", 1]]);
    const quiet = rows.filter(row => row.status === "idle");
    expect(agentOverviewGroups(quiet, "all").map(group => group.status)).toEqual(["idle"]);
  });

  it("shows only the chosen group under a filter", () => {
    expect(agentOverviewGroups(rows, "idle").map(group => [group.status, group.rows.length]))
      .toEqual([["idle", 2]]);
    expect(agentOverviewGroups(rows.filter(row => row.status !== "working"), "working"))
      .toEqual([]);
  });

  it("summarizes the parts that are not zero", () => {
    expect(agentOverviewSummary(agentOverviewCounts(rows)))
      .toBe("1 needs you · 1 working · 1 done · 2 idle · 1 stopped");
    expect(agentOverviewSummary({ all: 12, waiting: 2, working: 1, done: 0, idle: 9, stopped: 0 }))
      .toBe("2 need you · 1 working · 9 idle");
    expect(agentOverviewSummary({ all: 0, waiting: 0, working: 0, done: 0, idle: 0, stopped: 0 }))
      .toBe("No agent panes running");
  });
});

describe("AgentOverview markup", () => {
  const rows = agentOverviewRows([workspace("w1", ["a", "b", "c", "d"], ["a", "b", "c", "d"], "api")], byId(
    terminal("a", { activity: "waiting", title: "Security review" }),
    terminal("b", { activity: "working", profileId: "codex" }),
    terminal("c"), terminal("d", { state: "failed" }),
  ));
  const html = renderToStaticMarkup(createElement(AgentOverview, {
    rows, profileLabel: id => (id === "codex" ? "Codex" : "Claude Code"), onLocate: () => {}, onClose: () => {},
    now: 1_000_000,
    since: new Map([["a", { status: "waiting" as const, since: 1_000_000 - 4 * 60_000 }],
      ["b", { status: "working" as const, since: 1_000_000 - 4 * 60_000 }]]),
    tails: new Map([
      ["a", ["older", "Allow command?", "npm test -- upload", "1. Yes  2. Always  3. No"]],
      ["b", ["older", "● Bash npm test", "✻ Wiring…"]],
      ["c", ["idle lines are never drawn"]],
    ]),
    onCloseIdle: () => {},
  }));

  it("groups by urgency with the app's status words", () => {
    const labels = [...html.matchAll(
      /agent-overview-group-label">(?:<span class="dot [^"]+" aria-hidden="true"><\/span>)?<span>([^<]+)</gu,
    )].map(match => match[1]);
    expect(labels).toEqual(["Needs attention", "Working", "Idle", "Stopped"]);
    expect(html).toContain("1 needs you · 1 working · 1 idle · 1 stopped");
    expect(html).toContain('aria-label="Security review, Claude Code, api, Needs attention, 4m"');
    expect(html).toContain(">Failed</span>");
  });

  it("shows a live card's last lines, its clock, and how to open a waiting pane", () => {
    expect(html).toContain('<span class="agent-overview-row-tail" aria-hidden="true"><span>Allow command?</span>'
      + "<span>npm test -- upload</span><span>1. Yes  2. Always  3. No</span></span>");
    // Working cards show two lines, and a clock only once a change was watched.
    expect(html).toContain("<span>● Bash npm test</span><span>✻ Wiring…</span>");
    // Every card says its state in a pill, with the clock once one was watched.
    expect(html).toContain('<span class="dot dot-working" aria-hidden="true"></span>Working 4m</span>');
    expect(html).toContain("Open pane <kbd>⏎</kbd>");
    // Idle agents are one compact line, without a tail.
    expect(html).toMatch(/class="agent-overview-row is-idle is-compact"/u);
  });

  it("offers Close idle only with idle agents and a handler", () => {
    expect(html).toContain("Close idle (1)");
    const quiet = renderToStaticMarkup(createElement(AgentOverview, {
      rows, profileLabel: () => "Claude Code", onLocate: () => {}, onClose: () => {},
    }));
    expect(quiet).not.toContain("Close idle");
  });

  it("offers a Browser tab that shows its body in place of the agents", () => {
    const base = { rows, profileLabel: () => "Claude Code", onLocate: () => {}, onClose: () => {},
      browser: createElement("p", { className: "fixture-browser" }, "page") };
    const agents = renderToStaticMarkup(createElement(AgentOverview, base));
    expect(agents).toContain('aria-pressed="false"><span class="codicon codicon-globe" aria-hidden="true"></span>Browser');
    expect(agents).toContain('class="agent-overview-tab is-active" aria-pressed="true"');
    // Nothing loads until Browser is first chosen.
    expect(agents).not.toContain("fixture-browser");
    const browsing = renderToStaticMarkup(createElement(AgentOverview, { ...base, tab: "browser" }));
    expect(browsing).toContain('<div class="agent-overview-browser"><p class="fixture-browser">page</p></div>');
    expect(browsing).toContain('class="agent-overview-tab is-active" aria-pressed="true"><span class="codicon codicon-globe"');
    expect(browsing).not.toContain("agent-overview-list");
    expect(browsing).not.toContain("Search agents");
    // No body, no tab, and the agents show whatever tab was remembered.
    const plain = renderToStaticMarkup(createElement(AgentOverview, { ...base, browser: undefined, tab: "browser" }));
    expect(plain).not.toContain("codicon-globe");
    expect(plain).toContain("agent-overview-list");
  });

  it("draws the shared status dots, and none for idle", () => {
    expect(html).toContain("dot dot-waiting");
    expect(html).toContain("dot dot-working");
    expect(html).toContain("dot dot-failed");
    expect(html).not.toContain("dot-idle");
  });
});

describe("savedAgentOverviewWorkspace", () => {
  it("reads each saved record once, and a replaced record afresh", () => {
    const layout = openTerminalTab(EMPTY_TERMINAL_WORKSPACE_STATE, "pane-a", "t-open");
    const record: TerminalWorkspaceRecord = {
      id: "w-1", name: "Saved", createdAt: "2026-09-27T00:00:00Z", updatedAt: "2026-09-27T00:00:00Z",
      terminals: [{ id: "t-saved", title: "Claude", cwd: "/tmp/project" }],
      layout: serializeTerminalWorkspaceState(layout),
    };
    const first = savedAgentOverviewWorkspace(record);
    expect(first).toEqual(agentOverviewWorkspace(record, layout));
    expect(first.terminalIds).toEqual(["t-saved", "t-open"]);
    // The roster keeps records it does not change, so this is every splitter
    // frame and focus click while the overview is open: no second parse.
    expect(savedAgentOverviewWorkspace(record)).toBe(first);
    const replaced = { ...record, layout: serializeTerminalWorkspaceState(EMPTY_TERMINAL_WORKSPACE_STATE) };
    expect(savedAgentOverviewWorkspace(replaced).terminalIds).toEqual(["t-saved"]);
  });
});

describe("search, project grouping and clocks", () => {
  const rows = agentOverviewRows([
    workspace("w1", ["a", "b"], ["a", "b"], "api"),
    workspace("w2", ["c"], ["c"], "site"),
  ], byId(
    terminal("a", { title: "Auth middleware", activity: "working" }),
    terminal("b", { title: "Upload limit", profileId: "codex", activity: "waiting" }),
    terminal("c", { title: "Landing copy" }),
  ));

  it("matches the title, workspace or provider, ignoring case", () => {
    expect(agentOverviewMatches(rows[0], "AUTH", "Claude Code")).toBe(true);
    expect(agentOverviewMatches(rows[2], "site", "Claude Code")).toBe(true);
    expect(agentOverviewMatches(rows[1], "codex", "Codex")).toBe(true);
    expect(agentOverviewMatches(rows[1], "landing", "Codex")).toBe(false);
    expect(agentOverviewMatches(rows[1], "  ", "Codex")).toBe(true);
  });

  it("groups by workspace in roster order", () => {
    expect(agentOverviewProjectGroups(rows).map(group => [group.name, group.rows.map(row => row.terminalId)]))
      .toEqual([["api", ["a", "b"]], ["site", ["c"]]]);
  });

  it("dates only the changes it watched, and keeps the map when nothing moved", () => {
    const first = agentOverviewSince(new Map(), [{ id: "a", status: "working" }], 100);
    expect(first.get("a")).toEqual({ status: "working", since: null });
    const same = agentOverviewSince(first, [{ id: "a", status: "working" }], 200);
    expect(same).toBe(first);
    const moved = agentOverviewSince(same, [{ id: "a", status: "done" }], 300);
    expect(moved.get("a")).toEqual({ status: "done", since: 300 });
    expect(agentOverviewSince(moved, [], 400).size).toBe(0);
  });

  it("prints whole minutes, hours and days, and nothing for an unknown start", () => {
    expect(agentOverviewElapsed(null, 0)).toBeNull();
    expect(agentOverviewElapsed(0, 30_000)).toBe("<1m");
    expect(agentOverviewElapsed(0, 4 * 60_000 + 5_000)).toBe("4m");
    expect(agentOverviewElapsed(0, 65 * 60_000)).toBe("1h 5m");
    expect(agentOverviewElapsed(0, 120 * 60_000)).toBe("2h");
    expect(agentOverviewElapsed(0, 3 * 24 * 60 * 60_000)).toBe("3d");
  });
});

describe("agent overview column width", () => {
  const terminal = readFileSync(new URL("./terminal.css", import.meta.url), "utf8");

  it("takes 360px at a 1440px window and stops at 560px on a large display", () => {
    expect(terminal).toContain("--agent-overview-width: clamp(360px, 22vw, 560px);");
    expect(terminal).toMatch(/\.terminal-center\.has-agent-overview > \.agent-overview \{[^}]*width: min\(360px, 92%\);/u);
  });
});
