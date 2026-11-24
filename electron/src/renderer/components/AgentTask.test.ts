import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { EngineEvent, SessionSummary } from "../../shared/engine";
import { AgentTask } from "./AgentTask";

const session = (status: SessionSummary["status"]): SessionSummary => ({
  engine: "claude", sessionId: "s1", title: "Tidy the downloads folder",
  cwd: "/Users/me/Downloads", status, cloud: false, model: "claude-sonnet-5",
  lastActivity: "2026-08-29T10:00:00Z", firstSeen: "2026-08-29T09:59:00Z",
  archived: false, kind: "agent",
});
const events: EngineEvent[] = [
  { kind: "tool.call", callId: "1", tool: "Read", summary: "a.txt", input: {} },
  { kind: "tool.result", callId: "1", ok: true, summary: "" },
  { kind: "tool.call", callId: "2", tool: "Read", summary: "b.txt", input: {} },
  { kind: "tool.result", callId: "2", ok: true, summary: "" },
  { kind: "diff.produced", callId: "2", path: "/Users/me/Downloads/b.txt", before: "", after: "x" },
];
// The live log goes through the very same reducer the pane's transcript does
// (the session's shared projection), and history reaches it as ConvItems, so
// one list serves both. Opened, because a closed inspector draws neither list.
const render = (status: SessionSummary["status"], live: boolean, extra: EngineEvent[] = []): string =>
  renderToStaticMarkup(createElement(AgentTask, {
    session: session(status), events: [...events, ...extra], initialInspectorOpen: true,
    engineLabel: "Claude", live,
    turnStartedAt: live ? 1_000 : null, now: 66_000,
    onStop: () => undefined, onOpenDiff: () => undefined,
    // In the props object, not as a third argument: `children` is a required
    // prop, and createElement's rest children are not typed into it.
    children: createElement("div", { className: "transcript-stub" }),
  }));

describe("AgentTask", () => {
  it("shows the task, its folder, engine, status, and the folded steps", () => {
    const html = render("working", true);
    expect(html).toContain("Tidy the downloads folder");
    expect(html).toContain("Downloads");
    expect(html).toContain("Claude · claude-sonnet-5");
    expect(html).toContain("Read 2 files");
    expect(html).toMatch(/1m 5s/);
    expect(html).toContain('aria-label="Stop the task"');
  });
  it("prints the engine alone when the provider reports an empty model", () => {
    // Codex and Grok really do send `model: ""` and the registry passes it
    // through unnormalised, so an undefined-only check printed "Claude · "
    // with a dangling separator and nothing after it.
    const html = renderToStaticMarkup(createElement(AgentTask, {
      session: { ...session("idle"), model: "" }, events,
      engineLabel: "Claude", live: false, turnStartedAt: null, now: 66_000,
      onStop: () => undefined, onOpenDiff: () => undefined,
      children: createElement("div", { className: "transcript-stub" }),
    }));
    expect(html).toContain('<span class="agent-task-chip">Claude</span>');
    expect(html).not.toContain("Claude · ");
  });
  it("lists touched files as buttons and hides Stop once the task is idle", () => {
    const html = render("idle", false);
    expect(html).toContain("b.txt");
    expect(html).not.toContain('aria-label="Stop the task"');
    expect(html).toContain("transcript-stub");
  });

  it("prints how many lines each touched file gained and lost, and nothing when unknown", () => {
    const withStats = renderToStaticMarkup(createElement(AgentTask, {
      session: session("idle"), events, initialInspectorOpen: true, engineLabel: "Claude",
      live: false,
      turnStartedAt: null, now: 66_000, onStop: () => undefined, onOpenDiff: () => undefined,
      diffStats: { "/Users/me/Downloads/b.txt": { added: 12, removed: 4 } },
      children: createElement("div", { className: "transcript-stub" }),
    }));
    // THE ITEMS WIN WHERE BOTH ANSWER. The events above make b.txt +1 −0; the
    // caller says +12 −4 and loses, because `diffStats` is summed from the live
    // event log alone while the items span history and live — so on a session
    // this window did not start the caller's map is the one that undercounts.
    expect(withStats).toContain('<span class="agent-task-file-add">+1</span>');
    expect(withStats).toContain('<span class="agent-task-file-del">−0</span>');
    expect(withStats).not.toContain("+12");
    expect(render("idle", false)).toContain('<span class="agent-task-file-add">+1</span>');
    // …and the caller's count is still the fallback for a path the items left
    // unmeasured, so a live-only measurement reaches the row.
    const fallback = renderToStaticMarkup(createElement(AgentTask, {
      session: session("idle"), engineLabel: "Claude", live: false, events: [],
      history: { items: [{ kind: "diff", id: "d1", path: "/Users/me/Downloads/b.txt", callId: "2" }] },
      initialInspectorOpen: true,
      turnStartedAt: null, now: 66_000, onStop: () => undefined, onOpenDiff: () => undefined,
      diffStats: { "/Users/me/Downloads/b.txt": { added: 12, removed: 4 } },
      children: createElement("div", { className: "transcript-stub" }),
    }));
    expect(fallback).toContain('<span class="agent-task-file-add">+12</span>');
    expect(fallback).toContain('<span class="agent-task-file-del">−4</span>');
    // An unmeasured path says nothing rather than claiming "+0 −0" — the same
    // silence formatDiffStat keeps everywhere else. Matched on the SPAN, not the
    // bare class: the component ships the rule that styles it in its own <style>.
    const unmeasured = renderToStaticMarkup(createElement(AgentTask, {
      session: session("idle"), engineLabel: "Claude", live: false, events: [],
      history: { items: [{ kind: "diff", id: "d1", path: "/Users/me/Downloads/b.txt", callId: "2" }] },
      initialInspectorOpen: true,
      turnStartedAt: null, now: 66_000, onStop: () => undefined, onOpenDiff: () => undefined,
      children: createElement("div", { className: "transcript-stub" }),
    }));
    expect(unmeasured).toContain("b.txt");
    expect(unmeasured).not.toContain('<span class="agent-task-file-add"');
  });
  it("shows paths relative to the task's own folder, keeping the whole one on hover", () => {
    // Every row of this view is one session in one folder, so the folder's own
    // name repeated down the left of it is not information — the transcript
    // already strips it the same way through Conversation's `pathRoot`.
    const html = render("idle", false, [
      { kind: "tool.call", callId: "3", tool: "Bash", summary: "/Users/me/Downloads/run.sh",
        input: {} },
      { kind: "tool.result", callId: "3", ok: true, summary: "" },
    ]);
    expect(html).toContain('title="/Users/me/Downloads/b.txt">b.txt</button>');
    expect(html).toContain('title="/Users/me/Downloads/run.sh">run.sh</span>');
    // …and only under that folder. A path somewhere else keeps every segment.
    const outside = render("idle", false, [
      { kind: "diff.produced", callId: "9", path: "/etc/hosts", before: "", after: "x" },
    ]);
    expect(outside).toContain(">/etc/hosts</button>");
  });

  it("heads its two lists so the view reads as sections", () => {
    const html = render("idle", false);
    expect(html).toContain('<h2 class="agent-task-section">Progress</h2>');
    expect(html).toContain('<h2 class="agent-task-section">Files touched</h2>');
    // On the transcript summary's own type and weight, so the three agree.
    expect(html).toContain(".agent-task-section { margin:0; color:var(--fg-muted); "
      + "font-size:var(--text-meta);");
  });

  it("reads history before the live log, and a live row replaces its stored copy", () => {
    const html = renderToStaticMarkup(createElement(AgentTask, {
      session: session("idle"), engineLabel: "Claude", live: false, initialInspectorOpen: true,
      events: [{ kind: "tool.call", callId: "2", tool: "Bash", summary: "live run", input: {} },
        { kind: "tool.result", callId: "2", ok: true, summary: "" }],
      history: {
        items: [
          { kind: "tool", id: "1", tool: "Read", summary: "stored read", input: {} },
          { kind: "tool", id: "2", tool: "Bash", summary: "stored run", input: {} },
        ],
        plan: [{ content: "Stored plan step", priority: "medium", status: "pending" }],
      },
      turnStartedAt: null, now: 66_000, onStop: () => undefined, onOpenDiff: () => undefined,
      children: createElement("div", { className: "transcript-stub" }),
    }));
    expect(html).toContain("stored read");
    expect(html).toContain("live run");
    expect(html).not.toContain("stored run");
    expect(html.indexOf("stored read")).toBeLessThan(html.indexOf("live run"));
    expect(html).toContain("Stored plan step");
  });

  it("draws nothing in the closed inspector, and keeps its region for the toggle", () => {
    const html = renderToStaticMarkup(createElement(AgentTask, {
      session: session("working"), events, engineLabel: "Claude", live: true,
      turnStartedAt: 1_000, now: 66_000, onStop: () => undefined, onOpenDiff: () => undefined,
      children: createElement("div", { className: "transcript-stub" }),
    }));
    expect(html).toMatch(/<button type="button" class="agent-task-inspect" aria-expanded="false" aria-controls="([^"]+)"[\s\S]*<aside id="\1" class="agent-task-inspector" aria-label="Activity and tools" hidden="">\s*<\/aside>/u);
    expect(html).not.toContain("Read 2 files");
    expect(html).not.toContain("b.txt");
    expect(html).toContain("transcript-stub");
  });

  it("keeps the conversation visible while working or waiting for approval", () => {
    const waiting = render("waiting", true, [{ kind: "approval.requested", approvalId: "p",
      tool: "Bash", summary: "rm -rf build", input: {} }]);
    expect(waiting).toContain('<section class="agent-task-conversation" aria-label="Conversation">');
    expect(waiting).not.toContain("<details");
    expect(render("working", true)).toContain('<section class="agent-task-conversation" aria-label="Conversation">');
  });
});
