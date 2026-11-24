import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { AGENT_AUTO_MODEL_NOTE, AGENT_BYPASS_WARNING, AGENT_PERMISSION_HELP, AGENT_PERMISSION_TIMING, agentModelEfforts, AgentHeader, AgentMenu,
  AgentProfileDialog, claudeAutoUnsupported } from "./AgentMenu";
import { agentEngineRefusal, newAgentEngine, type AgentProfile } from "../agent-profiles";

const noop = (): void => undefined;

const agent = (over: Partial<AgentProfile> = {}): AgentProfile => ({
  id: "a1",
  name: "Release Manager",
  engine: "codex",
  model: "",
  effort: "",
  instructions: "",
  color: "teal",
  ...over,
});

describe("AgentMenu", () => {
  it("renders shaped characters in a vertical roster and marks the selected one", () => {
    const html = renderToStaticMarkup(createElement(AgentMenu, {
      agents: [agent(), agent({ id: "b2", name: "Docs", color: "sky" })],
      selectedId: "b2",
      onSelect: noop,
      onCreate: noop,
    }));

    expect(html).toContain('role="listbox"');
    expect(html).toContain('aria-label="Agents"');
    expect(html).toContain("Release Manager");
    expect(html).toContain('class="agent-menu-roster"');
    expect(html).toContain('data-character="teal"');
    expect(html).toContain('data-character="sky"');
    // Search opens from the header's filter glyph, like Tasks' (owner, 2026-09-28).
    expect(html).toContain('aria-label="Filter agents"');
    expect(html).not.toContain('aria-label="Search agents"');
    expect(html).toContain('aria-label="New agent"');
    // The selected card, and only it, is the highlighted option. Matched with
    // the title beside it: the stylesheet is part of this markup and carries
    // its own `[aria-selected="true"]` selectors.
    expect(html.match(/aria-selected="true" title=/gu)).toHaveLength(1);
    expect(html).toContain('aria-selected="true" title="Docs"');
    expect(html.match(/type="button"/gu)?.length).toBeGreaterThanOrEqual(3);
  });

  it("says so plainly when no agent has been made yet", () => {
    const html = renderToStaticMarkup(createElement(AgentMenu, {
      agents: [], selectedId: null, onSelect: noop, onCreate: noop,
    }));

    expect(html).toContain("No agents yet.");
    expect(html).not.toContain('role="listbox"');
  });

  it("marks each row with its status and a spoken label, and idle with nothing", () => {
    const agents = [
      agent({ id: "w", name: "Worker" }),
      agent({ id: "n", name: "Needs", color: "coral" }),
      agent({ id: "u", name: "Unread", color: "sky" }),
      agent({ id: "i", name: "Idle", color: "sand" }),
    ];
    const html = renderToStaticMarkup(createElement(AgentMenu, {
      agents, selectedId: null, onSelect: noop, onCreate: noop,
      statuses: { w: "working", n: "needsAttention", u: "unread", i: "idle" },
    }));
    const marks = [...html.matchAll(/<span class="agent-menu-status([^"]*)"[^>]*data-status="([^"]+)"[^>]*>/gu)];

    expect(marks.map(mark => mark[2])).toEqual(["working", "needsAttention", "unread"]);
    // Working is the sidebar's own working indicator; needs attention is the
    // attention amber glyph; unread is the tray's quiet dot.
    expect(marks[0]![1]).toContain("dot dot-working dot-pulse");
    expect(marks[1]![1]).toContain("session-wait");
    expect(marks[2]![1]).toContain("attention-tray-dot");
    expect(marks[0]![0]).toContain('aria-label="Working"');
    expect(marks[1]![0]).toContain('aria-label="Needs attention"');
    expect(marks[2]![0]).toContain('aria-label="Unread"');
    expect(html).not.toContain('data-status="idle"');
    // Every mark lives inside its own row, and the roster keeps saved order.
    expect([...html.matchAll(/<button [^>]*class="agent-menu-row"[^>]*title="([^"]+)"/gu)]
      .map(row => row[1])).toEqual(["Worker", "Needs", "Unread", "Idle"]);
    const worker = html.slice(html.indexOf('title="Worker"'), html.indexOf('title="Needs"'));
    expect(worker).toContain('data-status="working"');
  });

  it("previews the current thread's latest message before the latest task", () => {
    const threadKey = "claude#thread-1";
    const html = renderToStaticMarkup(createElement(AgentMenu, {
      agents: [agent({ id: "t", name: "Threaded" }), agent({ id: "p", name: "Plain", instructions: "Plain job" })],
      selectedId: null, onSelect: noop, onCreate: noop,
      latestTasks: {
        t: { engine: "codex", sessionId: "old", title: "Older task title", cwd: "/f", status: "idle",
          cloud: false, archived: false, lastActivity: "2026-09-07T08:00:00Z", firstSeen: "2026-09-07T08:00:00Z" },
      },
      threads: { t: { current: { sessionKey: threadKey, startedAtMs: 1, status: "idle", interrupted: false },
        past: [], lastReadAtMs: 0, lastActivityAtMs: 1 } },
      threadPreviews: { [threadKey]: "Drafted the launch brief" },
    }));

    expect(html).toContain('title="Drafted the launch brief">Drafted the launch brief</span>');
    expect(html).not.toContain(">Older task title</span>");
    expect(html).toContain('title="Plain job">Plain job</span>');
  });

  it("disables New agent until the agent store can take one", () => {
    const html = renderToStaticMarkup(createElement(AgentMenu, {
      agents: [], selectedId: null, onSelect: noop, onCreate: noop, createDisabled: true,
    }));

    expect(html).toContain('aria-label="New agent" title="New agent" disabled=""');
  });
});

describe("AgentHeader", () => {
  it("names the provider and how many of this agent's tasks are working", () => {
    const html = renderToStaticMarkup(createElement(AgentHeader, {
      profile: agent(),
      engineLabel: "Codex",
      workingCount: 2,
      onEdit: noop,
      onDelete: noop,
    }));

    expect(html).toContain("New tasks: Codex");
    expect(html).toContain("2 working");
    expect(html).toContain('aria-label="Edit agent"');
    expect(html).toContain('aria-label="Delete agent"');
  });
});

describe("AgentProfileDialog", () => {
  const engines = [
    { id: "codex", label: "Codex", available: true },
    { id: "claude", label: "Claude", available: false },
  ];
  const props = {
    open: true,
    initial: agent({ name: "" }),
    engines,
    effortLevelsFor: () => ["low", "high"] as const,
    onSubmit: noop,
    onCancel: noop,
  };

  it("titles a blank draft New agent and refuses to submit without a name", () => {
    const html = renderToStaticMarkup(createElement(AgentProfileDialog, props));

    expect(html).toContain("New agent");
    expect(html).toContain('role="dialog"');
    expect(html).toContain("Choose at launch");
    expect(html).toContain("Researches competitors and writes a weekly brief");
    // The provider and effort pickers are the app's own control now, so a
    // closed one states its choice on the trigger rather than listing options.
    expect(html).toContain('aria-label="Provider: Codex"');
    expect(html).toContain('aria-label="Effort: Default"');
    expect(html).not.toContain("<select");
    expect(html).toContain('type="submit"');
    expect(html).toContain("disabled=\"\">Create</button>");
  });

  it("asks for the name, what it does and the icon before the collapsed provider settings", () => {
    const html = renderToStaticMarkup(createElement(AgentProfileDialog, {
      ...props, initial: agent({ name: "", color: "coral", shape: "star" }),
    }));
    const at = (needle: string): number => {
      const index = html.indexOf(needle);
      expect(index, needle).toBeGreaterThan(-1);
      return index;
    };

    expect(at(">Name</label>")).toBeLessThan(at(">What it does</label>"));
    expect(at(">What it does</label>")).toBeLessThan(at(">Icon</span>"));
    expect(at(">Icon</span>")).toBeLessThan(at('aria-label="Shape"'));
    expect(at('aria-label="Shape"')).toBeLessThan(at('aria-label="Color"'));
    expect(at('aria-label="Color"')).toBeLessThan(at('<details class="agent-dialog-advanced">'));
    expect(at('<details class="agent-dialog-advanced">')).toBeLessThan(at('aria-label="Provider: Codex"'));
    // The live preview wears the chosen shape and colour.
    expect(html).toMatch(/class="agent-dialog-preview"><span class="agent-avatar" aria-hidden="true" data-character="coral"/u);
    expect(html).toContain("Codex · Model at launch");
  });

  it("titles a saved agent Edit agent and enables its submit", () => {
    const html = renderToStaticMarkup(createElement(AgentProfileDialog, {
      ...props, initial: agent(),
    }));

    expect(html).toContain("Edit agent");
    expect(html).toContain(">Save</button>");
    expect(html).not.toContain("disabled=\"\">Save</button>");
  });

  it("never creates an agent on a provider that cannot be a teammate (review A6)", () => {
    const acp = [...engines, { id: "acp:grok", label: "Grok", available: false,
      unavailableReason: agentEngineRefusal("acp:grok")! }];
    const named = { ...props, engines: acp, initial: agent({ name: "", engine: "acp:grok" }) };
    const html = renderToStaticMarkup(createElement(AgentProfileDialog, named)).replaceAll("&#x27;", "'");
    // Create stays off and the reason is on screen.
    expect(html).toContain("disabled=\"\">Create</button>");
    expect(html).toContain("Can't be a teammate yet: Interface can't enforce its approval gate.");
    // An agent saved on one before the rule can still be renamed.
    const saved = renderToStaticMarkup(createElement(AgentProfileDialog, {
      ...named, initial: agent({ engine: "acp:grok" }) }));
    expect(saved).not.toContain("disabled=\"\">Save</button>");
    expect(agentEngineRefusal("acp:qwen")).not.toBeNull();
    expect(agentEngineRefusal("acp:my-profile")).not.toBeNull();
    expect(agentEngineRefusal("claude")).toBeNull();
    expect(agentEngineRefusal("codex")).toBeNull();
    expect(newAgentEngine("acp:grok", "claude")).toBe("claude");
    expect(newAgentEngine("codex", "claude")).toBe("codex");
    expect(newAgentEngine(null, "claude")).toBe("claude");
  });

  it("renders nothing while closed", () => {
    expect(renderToStaticMarkup(createElement(AgentProfileDialog, { ...props, open: false })))
      .toBe("");
  });

  it("sets permissions after effort, and the summary names a provider mode", () => {
    const html = renderToStaticMarkup(createElement(AgentProfileDialog, {
      ...props, initial: agent(), initialPermission: "bypass",
    })).replaceAll("&#x27;", "'");
    const at = (needle: string): number => {
      const index = html.indexOf(needle);
      expect(index, needle).toBeGreaterThan(-1);
      return index;
    };
    expect(at('aria-label="Effort: Default"')).toBeLessThan(at('aria-label="Permissions: Bypass"'));
    expect(html).toContain("Codex · Model at launch · Bypass");
    expect(html).toContain(AGENT_PERMISSION_HELP.bypass);
    expect(html).toContain(AGENT_BYPASS_WARNING);
  });

  it("says a saved agent's new mode applies from its next thread", () => {
    const edit = renderToStaticMarkup(createElement(AgentProfileDialog, { ...props, initial: agent() }))
      .replaceAll("&#x27;", "'");
    expect(edit).toContain(AGENT_PERMISSION_TIMING);
    // A new agent has no running thread to keep an older mode.
    const create = renderToStaticMarkup(createElement(AgentProfileDialog, props)).replaceAll("&#x27;", "'");
    expect(create).not.toContain(AGENT_PERMISSION_TIMING);
  });

  it("keeps an agent that asks exactly as quiet as before", () => {
    const html = renderToStaticMarkup(createElement(AgentProfileDialog, { ...props, initial: agent() }))
      .replaceAll("&#x27;", "'");
    expect(html).toContain('aria-label="Permissions: Ask"');
    expect(html).toContain("Codex · Model at launch</span>");
    expect(html).toContain(AGENT_PERMISSION_HELP.gated);
    expect(html).not.toContain(AGENT_BYPASS_WARNING);
  });

  it("notes when Claude's auto mode cannot run on the chosen model", () => {
    for (const model of ["claude-haiku-4-5", "claude-sonnet-4-5", "claude-opus-4-1", "haiku"]) {
      expect(claudeAutoUnsupported(model), model).toBe(true);
    }
    for (const model of ["", "sonnet", "opus", "claude-sonnet-4-6", "claude-sonnet-5", "claude-opus-5-5", "claude-fable-5-1"]) {
      expect(claudeAutoUnsupported(model), model).toBe(false);
    }
    const claude = { ...props, engines: [{ id: "claude", label: "Claude", available: true }] };
    const html = (model: string): string => renderToStaticMarkup(createElement(AgentProfileDialog, {
      ...claude, initial: agent({ engine: "claude", model }), initialPermission: "auto",
    })).replaceAll("&#x27;", "'");
    expect(html("claude-haiku-4-5")).toContain(AGENT_AUTO_MODEL_NOTE);
    expect(html("claude-sonnet-5")).not.toContain(AGENT_AUTO_MODEL_NOTE);
  });
});


describe("agent model effort choices", () => {
  const models = [
    { value: "reasoner", displayName: "Reasoner", supportsEffort: true, effortLevels: ["low", "high"] },
    { value: "fast", displayName: "Fast", supportsEffort: false, effortLevels: [] },
  ];
  it("uses the selected model capability rather than the provider-wide ladder", () => {
    expect(agentModelEfforts("acp:local", "reasoner", models, ["medium"])).toEqual(["low", "high"]);
    expect(agentModelEfforts("acp:local", "fast", models, ["medium"])).toEqual([]);
    expect(agentModelEfforts("acp:local", "custom", models, ["medium"])).toEqual(["medium"]);
  });
});
