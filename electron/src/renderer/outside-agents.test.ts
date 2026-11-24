import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { startedFromOutsideWords, ThreadPaneHead } from "./components/ThreadPaneHead";
import { OUTSIDE_AGENT_OPTIONS, OUTSIDE_AGENT_SETUP, outsideAgentsLevel } from "./outside-agents";

describe("outside agents setting", () => {
  it("names Off, Read only and Can start and message threads, off when Rust says nothing", () => {
    expect(OUTSIDE_AGENT_OPTIONS.map(option => [option.value, option.label])).toEqual([
      ["off", "Off"], ["read", "Read only"], ["full", "Can start and message threads"],
    ]);
    const base = { autoSettleDays: 3, settleOnMerge: true, generatedTitles: false, trackPullRequests: true };
    expect(outsideAgentsLevel(base)).toBe("off");
    expect(outsideAgentsLevel(null)).toBe("off");
    expect(outsideAgentsLevel({ ...base, outsideAgents: "read" })).toBe("read");
    expect(outsideAgentsLevel({ ...base, outsideAgents: "full" })).toBe("full");
  });

  it("gives the one-line setup for Claude Code and Codex", () => {
    expect(OUTSIDE_AGENT_SETUP).toEqual([
      { tool: "Claude Code", command: "claude mcp add interface -- iface mcp" },
      { tool: "Codex", command: "codex mcp add interface -- iface mcp" },
    ]);
  });

  it("marks a thread an outside agent started in the pane head, with its client's name", () => {
    expect(startedFromOutsideWords("claude-code")).toBe("Started from outside (claude-code)");
    const head = renderToStaticMarkup(createElement(ThreadPaneHead, { startedFromOutside: "claude-code" }));
    expect(head).toContain("data-thread-outside=\"\"");
    expect(head).toContain(">Started from outside (claude-code)</span>");
    expect(head).toContain("asks you before it acts");
    expect(renderToStaticMarkup(createElement(ThreadPaneHead, {}))).toBe("");
  });
});
