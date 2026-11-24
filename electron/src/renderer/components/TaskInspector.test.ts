import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { HarnessView } from "../../shared/harness";
import { branchSessionLabel, inventoryGroups } from "../task-inspector-view";
import { TaskInspector } from "./TaskInspector";

const noop = (): void => undefined;
const view: HarnessView = {
  version: 1,
  state: { version: 1, revision: 3, attention: [], tasks: [], drafts: [], branches: [
    { parent: "claude:parent-1", child: "claude:child-1", root: "/repo", boundary: "provider-current" },
    { parent: "claude:gone", child: "codex:child-2", root: "/repo", boundary: "provider-current" },
  ] },
  catalog: [{ id: "iface.review", name: "review", aliases: ["r"], description: "Review the checkout",
    arguments: [], example: "/review", target: "iface TUI", effect: "Runs a review", requiredCapability: null }],
  context: {
    root: "/repo",
    inventory: { root: "/repo", truncated: false, entries: [
      { kind: "mcp", name: "github", detail: "stdio", scope: "user", sourcePath: "/home/.claude.json" },
      { kind: "rules", name: "CLAUDE.md", detail: "", scope: "project", sourcePath: "/repo/CLAUDE.md" },
      { kind: "skill", name: "deploy", detail: "Ship the app", scope: "project",
        sourcePath: "/repo/.claude/skills/deploy/SKILL.md", error: "front matter unreadable" },
    ] },
    suggestions: [], usage: "", omissions: [],
  },
};
const titles: Record<string, string> = {
  "claude:parent-1": "Fix the login bug", "claude:child-1": "Fix the login bug (branch)",
  "codex:child-2": "Try the other approach",
};
const render = (over: Partial<Parameters<typeof TaskInspector>[0]> = {}): string =>
  renderToStaticMarkup(createElement(TaskInspector, {
    view, busy: false, error: null, servers: [], onClose: noop, onCapture: noop, onInspect: noop,
    onContext: noop, onAttach: noop, onPreset: noop, onRunChecks: noop, onFindServers: noop,
    onOpenServer: noop, onOpenSession: noop, sessionTitle: key => titles[key], ...over,
  }));

describe("Task details (review A7, 2026-10-14)", () => {
  it("prints the context inventory as named groups instead of raw JSON", () => {
    const html = render();
    expect(html).not.toContain("&quot;entries&quot;");
    expect(html).not.toContain("sourcePath");
    expect(inventoryGroups(view.context!.inventory).map(group => group.label))
      .toEqual(["Rules files", "Skills", "MCP servers"]);
    expect(html).toContain("Rules files · 1");
    expect(html).toContain("<strong>deploy</strong> — Ship the app");
    expect(html).toContain("front matter unreadable");
    expect(html).toContain("(User)");
    expect(html.indexOf("Rules files")).toBeLessThan(html.indexOf("MCP servers"));
  });

  it("names conversation branches by session title, never by session key", () => {
    const html = render();
    expect(html).toContain("Source: Fix the login bug</button>");
    expect(html).toContain("Branch: Try the other approach</button>");
    expect(html).toContain("Source: Session no longer listed</button>");
    expect(html).not.toMatch(/>(Source|Branch): (claude|codex):/u);
    expect(branchSessionLabel("  ")).toBe("Session no longer listed");
  });

  it("offers no iface TUI commands the desktop cannot run", () => {
    const html = render();
    expect(html).not.toContain("slash command");
    expect(html).not.toContain("/review");
    expect(html).not.toContain("iface TUI");
  });

  it("loads its context inventory and presets in place when it opened on results", () => {
    const results = render({ view: { ...view, context: undefined }, onShowContext: noop, onShowPresets: noop });
    expect(results).toContain(">Context inventory</button>");
    expect(results).toContain(">Workspace presets</button>");
    // Already loaded: no button for it.
    expect(render({ onShowContext: noop, onShowPresets: noop })).not.toContain(">Context inventory</button>");
  });
});
