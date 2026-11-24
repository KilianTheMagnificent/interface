import { describe, it, expect } from "vitest";
import {
  collectRunningSubagents, compactSeamLabel, contextMeterDetails, contextMeterLabel,
  contextUsageReports, conversationItems, latestPlanEntries, settleDetachedSubagents,
} from "./conversation-reducer";
import type { ContextUsage, EngineEvent } from "../shared/engine";

describe("conversationItems", () => {
  it("coalesces consecutive message.delta into one streaming text item", () => {
    const evts: EngineEvent[] = [
      { kind: "message.delta", text: "a" },
      { kind: "message.delta", text: "b" },
      { kind: "message.delta", text: "c" },
    ];
    const items = conversationItems(evts);
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({ kind: "text", text: "abc", streaming: true });
  });

  it("keeps an ephemeral command refresh transparent to streaming prose", () => {
    const items = conversationItems([
      { kind: "message.delta", text: "one " },
      { kind: "commands.available", commands: [
        { name: "review", description: "Review changes" },
      ] },
      { kind: "message.delta", text: "sentence" },
    ]);
    expect(items).toEqual([{
      kind: "text", id: "text-0", text: "one sentence", streaming: true,
    }]);
  });

  it("stops streaming once a non-delta event follows the run", () => {
    const evts: EngineEvent[] = [
      { kind: "message.delta", text: "a" },
      { kind: "message.delta", text: "b" },
      { kind: "status.changed", status: "idle" },
    ];
    const items = conversationItems(evts);
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({ kind: "text", text: "ab", streaming: false });
  });

  it("starts a new text item after an interrupting event", () => {
    const evts: EngineEvent[] = [
      { kind: "message.delta", text: "one" },
      { kind: "tool.call", callId: "t1", tool: "Bash", summary: "ls", input: {} },
      { kind: "message.delta", text: "two" },
    ];
    const items = conversationItems(evts);
    expect(items.map((i) => i.kind)).toEqual(["text", "tool", "text"]);
    expect(items[0]).toMatchObject({ text: "one", streaming: false });
    expect(items[2]).toMatchObject({ text: "two", streaming: true });
  });

  it("pairs tool.result with its tool.call by callId, even out of order", () => {
    const evts: EngineEvent[] = [
      { kind: "tool.call", callId: "t1", tool: "Bash", summary: "ls", input: { command: "ls" } },
      { kind: "tool.call", callId: "t2", tool: "Read", summary: "read a.ts", input: { file: "a.ts" } },
      { kind: "tool.result", callId: "t2", ok: true, summary: "12 lines" },
      { kind: "tool.result", callId: "t1", ok: false, summary: "exit 1" },
    ];
    const items = conversationItems(evts);
    expect(items).toHaveLength(2);
    expect(items[0]).toMatchObject({
      kind: "tool", tool: "Bash", summary: "ls",
      input: { command: "ls" }, result: { ok: false, summary: "exit 1" },
    });
    expect(items[1]).toMatchObject({
      kind: "tool", tool: "Read", result: { ok: true, summary: "12 lines" },
    });
  });

  it("updates an existing tool card when the same callId is announced again", () => {
    const items = conversationItems([
      { kind: "tool.call", callId: "t1", tool: "other", summary: "read_file",
        input: { target_file: "/repo/a.ts" } },
      { kind: "tool.call", callId: "t1", tool: "Read", summary: "Read `/repo/a.ts`",
        input: { target_file: "/repo/a.ts" } },
    ]);
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({
      kind: "tool", id: "t1", tool: "Read", summary: "Read `/repo/a.ts`",
    });
  });

  it("leaves result undefined on a tool.call that has not returned", () => {
    const items = conversationItems([
      { kind: "tool.call", callId: "t1", tool: "Bash", summary: "ls", input: {} },
    ]);
    expect(items).toHaveLength(1);
    expect(items[0].kind).toBe("tool");
    expect(items[0]).not.toHaveProperty("result.ok");
    expect((items[0] as { result?: unknown }).result).toBeUndefined();
  });

  it("renders an approval item carrying tool, summary and input, unresolved", () => {
    const items = conversationItems([
      { kind: "approval.requested", approvalId: "ap1", tool: "Write",
        summary: "Write hello.txt", input: { file_path: "/tmp/hello.txt", content: "hi" } },
    ]);
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({
      kind: "approval", tool: "Write", summary: "Write hello.txt",
      input: { file_path: "/tmp/hello.txt", content: "hi" },
    });
    expect((items[0] as { resolved?: unknown }).resolved).toBeUndefined();
  });

  it("attaches parsed AskUserQuestion options to the approval item", () => {
    const items = conversationItems([
      { kind: "approval.requested", approvalId: "q1", tool: "AskUserQuestion",
        summary: "AskUserQuestion", input: {
          questions: [{
            question: "Which library?", header: "Library", multiSelect: false,
            options: [
              { label: "date-fns", description: "small" },
              { label: "luxon", description: "zones" },
            ],
          }],
        } },
    ]);
    expect(items[0]).toMatchObject({
      kind: "approval",
      questions: [{ question: "Which library?", header: "Library" }],
    });
  });

  it("yields a diff item carrying the path and the call that produced it", () => {
    // callId is what lets the pane draw a call and its diffs as one row. It has
    // to survive the reduction, or the merge silently stops pairing anything
    // and every engine goes back to drawing each edit twice.
    const items = conversationItems([
      { kind: "diff.produced", callId: "c1", path: "/tmp/p/a.ts", before: "a", after: "b" },
    ]);
    expect(items).toHaveLength(1);
    expect(items[0]).toMatchObject({
      kind: "diff", path: "/tmp/p/a.ts", callId: "c1", added: 1, removed: 1,
    });
  });

  it("keeps each diff attributed to its own call when two are open at once", () => {
    const items = conversationItems([
      { kind: "tool.call", callId: "a", tool: "edit", summary: "alpha", input: {} },
      { kind: "tool.call", callId: "b", tool: "edit", summary: "beta", input: {} },
      { kind: "diff.produced", callId: "a", path: "/tmp/alpha.ts", before: "1", after: "2" },
      { kind: "diff.produced", callId: "b", path: "/tmp/beta.ts", before: "3", after: "4" },
    ]);
    expect(items.filter(i => i.kind === "diff")
      .map(i => [(i as { path: string }).path, (i as { callId: string }).callId]))
      .toEqual([["/tmp/alpha.ts", "a"], ["/tmp/beta.ts", "b"]]);
  });

  it("yields a notice for error events", () => {
    const items = conversationItems([
      { kind: "error", message: "boom", retryable: false },
    ]);
    expect(items).toEqual([{ kind: "notice", id: expect.any(String), text: "boom",
      tone: "error" }]);
  });

  it("produces no item for thinking.signal, session.meta, status or usage events", () => {
    const items = conversationItems([
      { kind: "session.meta", sessionId: "s1", model: "m", cwd: "/tmp" },
      { kind: "thinking.signal", active: true },
      { kind: "status.changed", status: "working" },
      { kind: "usage.updated", inputTokens: 1, outputTokens: 2 },
      { kind: "thinking.signal", active: false },
    ]);
    expect(items).toEqual([]);
  });

  it("gives every item a distinct id", () => {
    const items = conversationItems([
      { kind: "message.delta", text: "hi" },
      { kind: "tool.call", callId: "t1", tool: "Bash", summary: "ls", input: {} },
      { kind: "message.delta", text: "there" },
      { kind: "diff.produced", callId: "c1", path: "/tmp/a.ts", before: "", after: "x" },
      { kind: "diff.produced", callId: "c1", path: "/tmp/a.ts", before: "x", after: "y" },
      { kind: "error", message: "boom", retryable: true },
      { kind: "approval.requested", approvalId: "ap1", tool: "Bash", summary: "rm", input: {} },
    ]);
    const ids = items.map((i) => i.id);
    expect(new Set(ids).size).toBe(ids.length);
    expect(ids.every((id) => id.length > 0)).toBe(true);
  });

  it("ignores a tool.result with no matching call", () => {
    const items = conversationItems([
      { kind: "tool.result", callId: "ghost", ok: true, summary: "orphan" },
    ]);
    expect(items).toEqual([]);
  });

  it("returns an empty list for no events", () => {
    expect(conversationItems([])).toEqual([]);
  });
});

describe("settleDetachedSubagents", () => {
  it("ends only unconfirmed running children when no live transport exists", () => {
    const items = conversationItems([
      { kind: "tool.call", callId: "running", tool: "spawnAgent",
        summary: "Scout", input: {} },
      { kind: "subagent.started", agentKey: "running", subagentType: "scout",
        description: "Scout" },
      { kind: "tool.call", callId: "done", tool: "spawnAgent",
        summary: "Reviewer", input: {} },
      { kind: "subagent.started", agentKey: "done", subagentType: "reviewer",
        description: "Reviewer" },
      { kind: "subagent.finished", agentKey: "done", ok: true, summary: "Reviewed" },
    ]);
    expect(settleDetachedSubagents(items).filter(item => item.kind === "subagent")
      .map(item => item.kind === "subagent" ? [item.id, item.status] : []))
      .toEqual([["running", "ended"], ["done", "done"]]);
    // The live projection remains untouched; the caller applies this helper
    // only when its `live` binding is false.
    expect(items.find(item => item.kind === "subagent" && item.id === "running"))
      .toMatchObject({ status: "running" });
  });
});

describe("collectRunningSubagents", () => {
  const agent = (id: string, status: "running" | "done" | "failed",
    children: ConvItem[] = []): ConvItem => ({
    kind: "subagent", id, subagentType: id, description: id, status,
    children, toolCount: 0, subagentCount: children.length,
  });

  it("walks nested running children and skips finished ones", () => {
    const items = [
      agent("parent", "running", [
        agent("child-run", "running"),
        agent("child-done", "done"),
      ]),
      agent("done", "done"),
      { kind: "text" as const, id: "t", text: "hi", streaming: false },
    ];
    expect(collectRunningSubagents(items).map(item => item.id))
      .toEqual(["parent", "child-run"]);
  });

  it("is empty when nobody is running", () => {
    expect(collectRunningSubagents([agent("a", "done"), agent("b", "failed")])).toEqual([]);
  });
});

describe("contextUsageReports", () => {
  it("keeps two complete reports while merging compacting-only state", () => {
    expect(contextUsageReports([
      { kind: "context.usage", usage: { used: 80, size: 200, percent: 40 } },
      { kind: "context.usage", usage: { percent: null, compacting: true } },
      { kind: "context.compact", trigger: "auto", ok: true },
      { kind: "context.usage", usage: { used: 45, size: 200, percent: 23,
        autoCompact: true, autoCompactThreshold: 180,
        categories: [{ name: "Tools", tokens: 20 }] } },
    ])).toEqual({
      previous: { used: 80, size: 200, percent: 40, compacting: false },
      current: { used: 45, size: 200, percent: 23,
        autoCompact: true, autoCompactThreshold: 180,
        categories: [{ name: "Tools", tokens: 20 }] },
    });
  });
});

describe("contextMeterDetails", () => {
  it("formats only facts available from the two provider reports", () => {
    expect(contextMeterDetails({ used: 125, size: 200, percent: 63,
      autoCompact: true, autoCompactThreshold: 180 },
    { used: 80, size: 200, percent: 40 })).toEqual({
      windowRemaining: "75",
      reportChange: "+45",
      autoCompact: "on",
      autoCompactThreshold: "180 · 55 remaining",
    });
    expect(contextMeterDetails({ percent: null, compacting: true })).toEqual({});
  });
});

describe("context and compact labels", () => {
  it("does not print 0% when a tiny occupancy rounded down", () => {
    expect(contextMeterLabel({ used: 100, size: 258400, percent: 0 })).toBe("<1%");
    expect(contextMeterLabel({ used: 0, size: 200, percent: 0 })).toBe("0%");
    expect(contextMeterLabel({ percent: null, compacting: true })).toBe("Compacting");
  });

  it("names a compact seam without inventing counts", () => {
    expect(compactSeamLabel({ kind: "compact", id: "c", trigger: "auto",
      preTokens: 180_000, postTokens: 42_000, ok: true }))
      .toBe("Context compacted automatically · 180k → 42k");
    expect(compactSeamLabel({ kind: "compact", id: "c", trigger: "unknown", ok: false }))
      .toBe("Context compaction failed");
  });
});

describe("conversationItems — context health", () => {
  it("draws compact as a seam and retry as an info notice, not an error", () => {
    expect(conversationItems([
      { kind: "message.delta", text: "before" },
      { kind: "context.compact", trigger: "auto", preTokens: 180_000, postTokens: 42_000,
        ok: true },
      { kind: "turn.retry", attempt: 1, maxAttempts: 10, delayMs: 1500,
        message: "Retrying (1/10) in 2s" },
      { kind: "context.usage", usage: { used: 42, size: 200, percent: 21 } },
    ])).toEqual([
      { kind: "text", id: "text-0", text: "before", streaming: false },
      { kind: "compact", id: "compact-1", trigger: "auto", preTokens: 180000,
        postTokens: 42000, ok: true },
      { kind: "notice", id: "notice-2",
        text: "Retry 1/10 — Retrying (1/10) in 2s", tone: "info" },
    ]);
  });

  // The counter is the renderer's own, not the provider's sentence: a vendor
  // that says nothing about which attempt this is left the GUI unable to tell a
  // first retry from a last one, while the CLI beside it drew "retry 3/5" the
  // whole time.
  it("prints the attempt and the ceiling even when the message omits both", () => {
    const items = conversationItems([
      { kind: "turn.retry", attempt: 3, maxAttempts: 5, message: "Overloaded" },
    ]);
    expect(items).toHaveLength(1);
    const notice = items[0];
    expect(notice.kind).toBe("notice");
    if (notice.kind !== "notice") throw new Error("expected a notice");
    expect(notice.text).toContain("3");
    expect(notice.text).toContain("5");
    expect(notice.text).toContain("Overloaded");
  });
});

// A consulted peer's answer is transcript — it is the reply to a question this
// session asked — and the renderer used to drop it entirely on a bare
// `default: break` while the CLI rendered it. The untrusted flag travels with
// it because a surface that loses the flag has no way left to say these words
// came from somewhere else.
describe("conversationItems — peer results", () => {
  const peer: EngineEvent = {
    kind: "peer.result", operationId: "op-7", sourceEngine: "codex",
    sourceSessionId: "s-3", objective: "Review the retry backoff",
    text: "The ceiling is never read.", untrusted: true,
  };

  it("keeps a peer reply in the transcript, marked untrusted", () => {
    expect(conversationItems([peer])).toEqual([{
      kind: "peer", id: "peer-0", operationId: "op-7", sourceEngine: "codex",
      objective: "Review the retry backoff", text: "The ceiling is never read.",
      untrusted: true,
    }]);
  });

  it("closes the text run it lands in, like any other non-delta event", () => {
    const items = conversationItems([
      { kind: "message.delta", text: "asking" },
      peer,
      { kind: "message.delta", text: "answered" },
    ]);
    expect(items.map(item => item.kind)).toEqual(["text", "peer", "text"]);
    expect(items[0]).toMatchObject({ text: "asking", streaming: false });
  });
});

describe("latestPlanEntries", () => {
  it("uses ACP's complete-replacement semantics", () => {
    const first = [{ content: "Old", priority: "low" as const, status: "pending" as const }];
    const latest = [{ content: "Run checks", priority: "high" as const,
      status: "in_progress" as const }];
    expect(latestPlanEntries([
      { kind: "plan.updated", entries: first },
      { kind: "message.delta", text: "working" },
      { kind: "plan.updated", entries: latest },
    ])).toBe(latest);
  });
});

// The two model/effort events are configuration, not transcript. The reducer's
// `default: break` already drops them, so this pins that behaviour rather than
// adding it: a config pill changing must not push a card into the message list,
// and — the part a default branch can still get wrong — it must not interrupt a
// streaming text block either, because any non-delta event closes the open run.
describe("conversationItems — model/effort events", () => {
  const models: EngineEvent = { kind: "models.available", models: [
    { value: "claude-opus-5", displayName: "Opus 5", supportsEffort: true,
      effortLevels: ["low", "high"] },
  ] };
  const changed: EngineEvent = { kind: "config.changed", model: "claude-opus-5",
    effort: "high" };

  it("renders nothing for either event", () => {
    expect(conversationItems([models, changed])).toEqual([]);
  });

  it("still splits the text run they land in, like any other non-delta event", () => {
    const items = conversationItems([
      { kind: "message.delta", text: "before" },
      changed,
      { kind: "message.delta", text: "after" },
    ]);
    expect(items).toEqual([
      { kind: "text", id: "text-0", text: "before", streaming: false },
      { kind: "text", id: "text-2", text: "after", streaming: true },
    ]);
  });
});

/* ---- subagents (Phase 1.6) ------------------------------------------------ */

import type { ConvItem } from "./conversation-reducer";

// The engine's real spawn shape: the Agent/Task tool.call lands first and
// `subagent.started` follows it, keyed by that call's own tool_use id.
function spawn(key: string, type = "reviewer", desc = "Review the diff",
    model?: string): EngineEvent[] {
  const started: EngineEvent = model === undefined
    ? { kind: "subagent.started", agentKey: key, subagentType: type, description: desc }
    : { kind: "subagent.started", agentKey: key, subagentType: type, description: desc,
        model };
  return [
    { kind: "tool.call", callId: key, tool: "Task", summary: `Task — ${desc}`,
      input: { subagent_type: type, description: desc } },
    started,
  ];
}

// Narrowing accessor: a wrong kind must fail the test loudly rather than read
// `undefined.children` three lines later.
function sub(item: ConvItem | undefined): Extract<ConvItem, { kind: "subagent" }> {
  if (item === undefined || item.kind !== "subagent") {
    throw new Error(`expected a subagent item, got ${String(item?.kind)}`);
  }
  return item;
}

describe("conversationItems — subagent grouping", () => {
  it("draws the card where the spawn was, and drops the spawn's own tool card", () => {
    const items = conversationItems([
      { kind: "message.delta", text: "delegating" },
      ...spawn("a1"),
      { kind: "message.delta", text: "back" },
    ]);
    expect(items.map(i => i.kind)).toEqual(["text", "subagent", "text"]);
    expect(items[1]).toMatchObject({
      kind: "subagent", id: "a1", subagentType: "reviewer",
      description: "Review the diff", status: "running",
    });
    // The card IS the spawn — the Task call must not also appear as a tool row.
    expect(items.some(i => i.kind === "tool")).toBe(false);
  });

  it("suppresses the spawn's tool.result as well as its tool.call", () => {
    const items = conversationItems([
      ...spawn("a1"),
      { kind: "tool.result", callId: "a1", ok: true, summary: "subagent said things" },
      { kind: "subagent.finished", agentKey: "a1", ok: true, summary: "subagent said things" },
    ]);
    expect(items.map(i => i.kind)).toEqual(["subagent"]);
    expect(sub(items[0]).status).toBe("done");
  });

  it("nests the subagent's own events as children, by the same item rules", () => {
    const items = conversationItems([
      ...spawn("a1"),
      { kind: "message.delta", text: "look", agentKey: "a1" },
      { kind: "message.delta", text: "ing", agentKey: "a1" },
      { kind: "tool.call", callId: "t1", tool: "Bash", summary: "ls",
        input: { command: "ls" }, agentKey: "a1" },
      { kind: "tool.result", callId: "t1", ok: true, summary: "3 files", agentKey: "a1" },
      { kind: "diff.produced", callId: "c1", path: "/tmp/a.ts", before: "a", after: "b", agentKey: "a1" },
      { kind: "error", message: "inner boom", retryable: true, },
    ]);
    // The error carries no agentKey, so it belongs to the main thread.
    expect(items.map(i => i.kind)).toEqual(["subagent", "notice"]);
    const card = sub(items[0]);
    expect(card.children.map(c => c.kind)).toEqual(["text", "tool", "diff"]);
    expect(card.children[0]).toMatchObject({ text: "looking", streaming: false });
    expect(card.children[1]).toMatchObject({
      kind: "tool", tool: "Bash", input: { command: "ls" },
      result: { ok: true, summary: "3 files" },
    });
    expect(card.children[2]).toMatchObject({ kind: "diff", path: "/tmp/a.ts" });
    expect(card.toolCount).toBe(1);
  });

  it("keeps subagent prose out of the main thread's text runs", () => {
    const items = conversationItems([
      { kind: "message.delta", text: "main-a" },
      ...spawn("a1"),
      { kind: "message.delta", text: "inner", agentKey: "a1" },
      { kind: "message.delta", text: "main-b" },
    ]);
    expect(items.map(i => i.kind)).toEqual(["text", "subagent", "text"]);
    expect(items[0]).toMatchObject({ text: "main-a", streaming: false });
    expect(items[2]).toMatchObject({ text: "main-b", streaming: true });
    expect(sub(items[1]).children).toMatchObject([{ kind: "text", text: "inner" }]);
  });

  it("moves running -> done and keeps the reported summary as the activity line", () => {
    const evts: EngineEvent[] = [
      ...spawn("a1"),
      { kind: "tool.call", callId: "t1", tool: "Bash", summary: "npm test",
        input: {}, agentKey: "a1" },
    ];
    expect(sub(conversationItems(evts)[0])).toMatchObject({
      status: "running", lastActivity: "npm test",
    });
    const done = conversationItems([...evts,
      { kind: "subagent.finished", agentKey: "a1", ok: true, summary: "12 tests passed" }]);
    expect(sub(done[0])).toMatchObject({ status: "done", lastActivity: "12 tests passed" });
  });

  it("marks a subagent that finished badly as failed", () => {
    const items = conversationItems([
      ...spawn("a1"),
      { kind: "subagent.finished", agentKey: "a1", ok: false, summary: "gave up" },
    ]);
    expect(sub(items[0])).toMatchObject({ status: "failed", lastActivity: "gave up" });
  });

  it("stops the trailing text child streaming once the subagent finishes", () => {
    const trailing: EngineEvent[] = [
      ...spawn("a1"),
      { kind: "message.delta", text: "I looked at it", agentKey: "a1" },
    ];
    // Still open while it runs: the caret belongs on a live subagent.
    expect(sub(conversationItems(trailing)[0]).children).toEqual([
      { kind: "text", id: "text-2", text: "I looked at it", streaming: true },
    ]);
    // Closed the moment it reports back — a blinking caret under a header that
    // says "done" is the bug this pins.
    const done = conversationItems([...trailing,
      { kind: "subagent.finished", agentKey: "a1", ok: true, summary: "ok" }]);
    expect(sub(done[0]).children).toEqual([
      { kind: "text", id: "text-2", text: "I looked at it", streaming: false },
    ]);
    const failed = conversationItems([...trailing,
      { kind: "subagent.finished", agentKey: "a1", ok: false, summary: "gave up" }]);
    expect(sub(failed[0])).toMatchObject({ status: "failed" });
    expect(sub(failed[0]).children).toEqual([
      { kind: "text", id: "text-2", text: "I looked at it", streaming: false },
    ]);
  });

  it("renders a subagent that produced nothing at all", () => {
    const items = conversationItems([
      ...spawn("a1"),
      { kind: "subagent.finished", agentKey: "a1", ok: true, summary: "" },
    ]);
    expect(items).toHaveLength(1);
    const card = sub(items[0]);
    expect(card.children).toEqual([]);
    expect(card.toolCount).toBe(0);
    expect(card.lastActivity).toBeUndefined();
    expect(card.status).toBe("done");
  });

  it("separates two subagents whose streams interleave", () => {
    const items = conversationItems([
      ...spawn("a1", "reviewer", "review"),
      ...spawn("a2", "tester", "test"),
      { kind: "message.delta", text: "r1", agentKey: "a1" },
      { kind: "message.delta", text: "t1", agentKey: "a2" },
      { kind: "message.delta", text: "r2", agentKey: "a1" },
      { kind: "tool.call", callId: "x1", tool: "Bash", summary: "vitest",
        input: {}, agentKey: "a2" },
      { kind: "message.delta", text: "t2", agentKey: "a2" },
      { kind: "subagent.finished", agentKey: "a2", ok: true, summary: "green" },
    ]);
    expect(items.map(i => i.kind)).toEqual(["subagent", "subagent"]);
    const [one, two] = [sub(items[0]), sub(items[1])];
    expect(one.id).toBe("a1");
    expect(two.id).toBe("a2");
    // Each thread coalesces its OWN deltas: a1 says "r1r2", not "r1t1r2".
    expect(one.children).toMatchObject([{ kind: "text", text: "r1r2" }]);
    expect(two.children.map(c => c.kind)).toEqual(["text", "tool", "text"]);
    expect(two.children[0]).toMatchObject({ text: "t1" });
    expect(two.children[2]).toMatchObject({ text: "t2" });
    expect(one.status).toBe("running");
    expect(two.status).toBe("done");
    expect(two.toolCount).toBe(1);
  });

  it("nests a subagent spawned by a subagent under its parent", () => {
    const items = conversationItems([
      ...spawn("a1", "planner", "plan it"),
      // The inner spawn is announced on a1's thread: its tool.call carries a1's key.
      { kind: "tool.call", callId: "a2", tool: "Task", summary: "Task — dig",
        input: { subagent_type: "digger", description: "dig" }, agentKey: "a1" },
      { kind: "subagent.started", agentKey: "a2", subagentType: "digger",
        description: "dig" },
      { kind: "message.delta", text: "digging", agentKey: "a2" },
    ]);
    expect(items.map(i => i.kind)).toEqual(["subagent"]);
    const outer = sub(items[0]);
    expect(outer.children.map(c => c.kind)).toEqual(["subagent"]);
    // A nested spawn is its own card, never one of the parent's tool calls.
    expect(outer.toolCount).toBe(0);
    const inner = sub(outer.children[0]);
    expect(inner.id).toBe("a2");
    expect(inner.children).toMatchObject([{ kind: "text", text: "digging" }]);
  });

  it("leaves events with an unannounced agentKey on the main thread", () => {
    const items = conversationItems([
      { kind: "message.delta", text: "hi", agentKey: "ghost" },
      { kind: "tool.call", callId: "t1", tool: "Bash", summary: "ls",
        input: {}, agentKey: "ghost" },
      { kind: "subagent.finished", agentKey: "ghost", ok: true, summary: "?" },
    ]);
    // Nothing to nest under, so it degrades to the flat stream this app showed
    // before subagents existed — dropped events would be worse than plain ones.
    expect(items.map(i => i.kind)).toEqual(["text", "tool"]);
    expect(items[0]).toMatchObject({ text: "hi" });
  });

  it("draws the card at its announcement when no spawn call was seen", () => {
    const items = conversationItems([
      { kind: "message.delta", text: "before" },
      { kind: "subagent.started", agentKey: "a1", subagentType: "reviewer",
        description: "review" },
      { kind: "message.delta", text: "inner", agentKey: "a1" },
    ]);
    expect(items.map(i => i.kind)).toEqual(["text", "subagent"]);
    expect(sub(items[1]).children).toMatchObject([{ kind: "text", text: "inner" }]);
  });

  it("keeps only the first announcement of a repeated agentKey", () => {
    const items = conversationItems([
      ...spawn("a1", "reviewer", "first"),
      { kind: "subagent.started", agentKey: "a1", subagentType: "other",
        description: "second" },
    ]);
    expect(items).toHaveLength(1);
    expect(sub(items[0])).toMatchObject({ subagentType: "reviewer", description: "first" });
  });

  it("carries the model only when the engine named one", () => {
    const named = conversationItems(spawn("a1", "reviewer", "r", "claude-opus-5"));
    expect(sub(named[0]).model).toBe("claude-opus-5");
    const plain = conversationItems(spawn("a2"));
    expect(sub(plain[0])).not.toHaveProperty("model");
  });

  it("prefers the newest tool summary for the activity line, prose as fallback", () => {
    const prose = conversationItems([
      ...spawn("a1"),
      { kind: "message.delta", text: "  reading\n the  file  ", agentKey: "a1" },
    ]);
    // Collapsed to one line: the card renders this on a single ellipsised row.
    expect(sub(prose[0]).lastActivity).toBe("reading the file");

    const tools = conversationItems([
      ...spawn("a1"),
      { kind: "message.delta", text: "thinking", agentKey: "a1" },
      { kind: "tool.call", callId: "t1", tool: "Read", summary: "read a.ts",
        input: {}, agentKey: "a1" },
      { kind: "tool.result", callId: "t1", ok: true, summary: "40 lines", agentKey: "a1" },
    ]);
    expect(sub(tools[0]).lastActivity).toBe("40 lines");

    const long = conversationItems([
      ...spawn("a1"),
      { kind: "tool.result", callId: "t1", ok: true, summary: "x".repeat(200),
        agentKey: "a1" },
    ]);
    const line = sub(long[0]).lastActivity ?? "";
    expect(line.length).toBe(80);
    expect(line.endsWith("…")).toBe(true);
  });

  it("gives every item a distinct id across the main thread and its subagents", () => {
    const items = conversationItems([
      { kind: "message.delta", text: "main" },
      ...spawn("a1"),
      { kind: "message.delta", text: "inner", agentKey: "a1" },
      { kind: "diff.produced", callId: "c1", path: "/tmp/a.ts", before: "", after: "x", agentKey: "a1" },
      { kind: "diff.produced", callId: "c1", path: "/tmp/a.ts", before: "x", after: "y" },
    ]);
    const ids: string[] = [];
    const walk = (list: ConvItem[]): void => {
      for (const item of list) {
        ids.push(item.id);
        if (item.kind === "subagent") walk(item.children);
      }
    };
    walk(items);
    expect(ids).toHaveLength(5);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("does not let the lifecycle events split a main-thread text run", () => {
    const items = conversationItems([
      { kind: "message.delta", text: "a" },
      ...spawn("a1"),
      { kind: "message.delta", text: "b" },
      { kind: "subagent.finished", agentKey: "a1", ok: true, summary: "done" },
      { kind: "message.delta", text: "c" },
    ]);
    // The card already split the run where the spawn happened; a second split
    // would leave an empty block with nothing rendered between the two halves.
    expect(items.map(i => i.kind)).toEqual(["text", "subagent", "text"]);
    expect(items[2]).toMatchObject({ text: "bc" });
  });
});

/* ---- a subagent whose stream ends without a result ------------------------ */

// `subagent.finished` is not guaranteed: an interrupted turn, a dead CLI or a
// session that ends mid-spawn leaves a card with no closing event at all. It
// must not keep claiming to run, and it must not claim a result nobody
// reported — hence "ended", which is neither "done" nor "failed".
describe("conversationItems — subagent still running when the stream ends", () => {
  it("ends a running subagent on session.closed", () => {
    const items = conversationItems([
      ...spawn("a1"),
      { kind: "message.delta", text: "working", agentKey: "a1" },
      { kind: "session.closed" },
    ]);
    expect(sub(items[0]).status).toBe("ended");
  });

  it("ends a running subagent on a non-retryable error", () => {
    const items = conversationItems([
      ...spawn("a1"),
      { kind: "error", message: "cli exited", retryable: false },
    ]);
    // The error is still the main thread's notice; the card ends alongside it.
    expect(items.map(i => i.kind)).toEqual(["subagent", "notice"]);
    expect(sub(items[0]).status).toBe("ended");
  });

  it("ends a running subagent when the session status goes failed", () => {
    const items = conversationItems([
      ...spawn("a1"),
      { kind: "status.changed", status: "failed" },
    ]);
    expect(sub(items[0]).status).toBe("ended");
  });

  it("leaves a running subagent running on a RETRYABLE error", () => {
    // A retry is the turn continuing, not the stream ending.
    const items = conversationItems([
      ...spawn("a1"),
      { kind: "error", message: "overloaded, retrying", retryable: true },
    ]);
    expect(sub(items[0]).status).toBe("running");
  });

  it("does not downgrade a subagent that already reported done", () => {
    const items = conversationItems([
      ...spawn("a1"),
      { kind: "subagent.finished", agentKey: "a1", ok: true, summary: "all green" },
      { kind: "session.closed" },
    ]);
    expect(sub(items[0])).toMatchObject({ status: "done", lastActivity: "all green" });
  });

  it("leaves a subagent that reported failure as failed", () => {
    const items = conversationItems([
      ...spawn("a1"),
      { kind: "subagent.finished", agentKey: "a1", ok: false, summary: "gave up" },
      { kind: "error", message: "cli exited", retryable: false },
      { kind: "session.closed" },
    ]);
    expect(sub(items[0])).toMatchObject({ status: "failed", lastActivity: "gave up" });
  });

  it("leaves a running subagent running while the stream is still open", () => {
    const items = conversationItems([
      ...spawn("a1"),
      { kind: "message.delta", text: "still going", agentKey: "a1" },
      { kind: "status.changed", status: "working" },
      { kind: "usage.updated", inputTokens: 1, outputTokens: 2 },
    ]);
    expect(sub(items[0]).status).toBe("running");
  });

  it("does not end a subagent spawned after the last terminal event", () => {
    // The event log accumulates across a resume, so a close belonging to an
    // earlier turn must not end a subagent that only started afterwards.
    const items = conversationItems([
      { kind: "error", message: "turn one died", retryable: false },
      { kind: "session.closed" },
      ...spawn("a1"),
      { kind: "message.delta", text: "turn two", agentKey: "a1" },
    ]);
    expect(items.map(i => i.kind)).toEqual(["notice", "subagent"]);
    expect(sub(items[1]).status).toBe("running");
  });

  it("stops the trailing text child streaming once the stream ends", () => {
    // The same rule the finished case pins: a live caret under a card that is
    // no longer running is exactly the false state this status exists to avoid.
    const items = conversationItems([
      ...spawn("a1"),
      { kind: "message.delta", text: "half a sen", agentKey: "a1" },
      { kind: "session.closed" },
    ]);
    const card = sub(items[0]);
    expect(card.status).toBe("ended");
    expect(card.children).toEqual([
      { kind: "text", id: "text-2", text: "half a sen", streaming: false },
    ]);
  });

  it("ends a nested subagent along with the parent that spawned it", () => {
    const items = conversationItems([
      ...spawn("a1", "planner", "plan it"),
      { kind: "tool.call", callId: "a2", tool: "Task", summary: "Task — dig",
        input: { subagent_type: "digger", description: "dig" }, agentKey: "a1" },
      { kind: "subagent.started", agentKey: "a2", subagentType: "digger",
        description: "dig" },
      { kind: "session.closed" },
    ]);
    const outer = sub(items[0]);
    expect(outer.status).toBe("ended");
    expect(sub(outer.children[0]).status).toBe("ended");
  });
});

/* ---- the user's own turns ------------------------------------------------- */

import {
  APPROVAL_ALLOW_KEYS, APPROVAL_ALLOW_KEYSHORTCUTS, APPROVAL_DENY_KEYS, APPROVAL_DENY_KEYSHORTCUTS,
  APPROVAL_KEY_ATTR, approvalKeyTyping, approvalKeyVerdict, approvalChordStep, keyedApprovalId,
  approvalVerdictKey,
} from "./approval-keys";
import {
  TRANSCRIPT_SEARCH_MAX_QUERY_CHARS, TRANSCRIPT_SEARCH_MAX_RESULTS, buildTranscriptSearchCorpus,
  moveTranscriptSearchIndex, searchTranscriptCorpus, transcriptSearchStatus,
} from "./transcript-search";
import {
  countUserTurns, echoSeen, launchPendingEcho, mergeTranscript, settledEchoRowKey,
} from "./transcript-merge";
import {
  editedFilesLabel, toolInputBlocks, turnChangeSummaries, turnChangesLabel,
} from "./tool-activity";
import { expandImportedRows, IMPORTED_SESSION_LABEL } from "./transcript-import";
import {
  jumpLatestBacklog, jumpLatestLabel, jumpScrollBehavior, paceArrival, paceFrame,
  prefersReducedMotion, promptAnchorKey, promptAnchorView, REVEAL_BURST_MS, REVEAL_FLOOD_CHARS,
  REVEAL_SNAP_CHARS, type RevealPace, mountedPace, settledPace, transcriptPrompts, truncationNotice,
} from "./transcript-navigation";
import type { PendingEcho, TranscriptRow } from "./transcript-merge";

/** A work group's plain-text line, as `WorkGroupText` draws it. */
const workGroupLabel = (input: Parameters<typeof workGroupParts>[0]): string =>
  workGroupPartsLabel(workGroupParts(input));
/** One-shot transcript search; Conversation runs the same two stages. */
const transcriptSearchMatches = (rows: readonly TranscriptRow[], query: string) =>
  [...searchTranscriptCorpus(buildTranscriptSearchCorpus(rows), query).matches];

// Narrowing accessor, same reason as sub(): a missing user item must fail here
// rather than three assertions later.
function user(item: ConvItem | undefined): Extract<ConvItem, { kind: "user" }> {
  if (item === undefined || item.kind !== "user") {
    throw new Error(`expected a user item, got ${String(item?.kind)}`);
  }
  return item;
}

describe("conversationItems — user turns", () => {
  it("renders a replayed user.message as a user item", () => {
    const items = conversationItems([
      { kind: "user.message", text: "what broke?" },
      { kind: "message.delta", text: "the build did" },
    ]);
    expect(items.map(i => i.kind)).toEqual(["user", "text"]);
    expect(user(items[0]).text).toBe("what broke?");
  });

  it("reduces a session that is ENTIRELY user turns to those turns", () => {
    // 14 of the 87 sessions on the author's machine look exactly like this.
    // Before user.message had a case they reduced to [] and the pane said
    // "No messages yet." over a real conversation.
    const items = conversationItems([
      { kind: "session.meta", sessionId: "s1", model: "opus", cwd: "/tmp" },
      { kind: "user.message", text: "first thing" },
      { kind: "user.message", text: "second thing" },
      { kind: "status.changed", status: "idle" },
    ]);
    expect(items.map(i => i.kind)).toEqual(["user", "user"]);
    expect(items.map(i => user(i).text)).toEqual(["first thing", "second thing"]);
  });

  it("closes an open text run, so the answer above it stops streaming", () => {
    const items = conversationItems([
      { kind: "message.delta", text: "done." },
      { kind: "user.message", text: "now the tests" },
      { kind: "message.delta", text: "running them" },
    ]);
    expect(items.map(i => i.kind)).toEqual(["text", "user", "text"]);
    expect(items[0]).toMatchObject({ text: "done.", streaming: false });
    expect(items[2]).toMatchObject({ text: "running them", streaming: true });
  });

  it("ids user turns by their position in the ORIGINAL event array", () => {
    // Same rule as text-/diff-/notice-: unique across every thread, and stable
    // for as long as the same array is reduced.
    const items = conversationItems([
      { kind: "user.message", text: "a" },
      { kind: "message.delta", text: "…" },
      { kind: "user.message", text: "b" },
    ]);
    expect(items.map(i => i.id)).toEqual(["user-0", "text-1", "user-2"]);
  });

  it("nests a subagent's prompt under its card rather than the main thread", () => {
    const items = conversationItems([
      ...spawn("a1"),
      { kind: "user.message", text: "inner prompt", agentKey: "a1" },
      { kind: "user.message", text: "outer prompt" },
    ]);
    expect(items.map(i => i.kind)).toEqual(["subagent", "user"]);
    expect(user(sub(items[0]).children[0]).text).toBe("inner prompt");
    expect(user(items[1]).text).toBe("outer prompt");
  });

  it("keeps a user turn whose agentKey nobody announced on the main thread", () => {
    // The doc comment's rule: unresolvable attribution is never dropped.
    const items = conversationItems([
      { kind: "user.message", text: "orphan", agentKey: "ghost" },
    ]);
    expect(user(items[0]).text).toBe("orphan");
  });

  const SHOT = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUg==";

  it("carries a replayed turn's images onto the item that draws them", () => {
    const items = conversationItems([
      { kind: "user.message", text: "what is this?", images: [{ src: SHOT, label: "1" }] },
    ]);
    expect(user(items[0])).toEqual({ kind: "user", id: "user-0", text: "what is this?",
      images: [{ src: SHOT, label: "1" }] });
  });

  it("carries them into a subagent's nested transcript too", () => {
    const items = conversationItems([
      ...spawn("a1"),
      { kind: "user.message", text: "look", agentKey: "a1",
        images: [{ src: SHOT, label: "1" }] },
    ]);
    expect(user(sub(items[0]).children[0]).images).toEqual([{ src: SHOT, label: "1" }]);
  });

  it("leaves a wordless picture turn as a real turn", () => {
    const items = conversationItems([
      { kind: "user.message", text: "", images: [{ src: SHOT, label: "1" }] },
    ]);
    expect(items.map(i => i.kind)).toEqual(["user"]);
    expect(user(items[0]).images).toHaveLength(1);
  });

  it("adds no empty `images` to a plain turn", () => {
    // A message with no pictures stays the item it has always been, so nothing
    // downstream has to distinguish "none" from "[]".
    expect(user(conversationItems([{ kind: "user.message", text: "hi" }])[0]))
      .toEqual({ kind: "user", id: "user-0", text: "hi" });
    expect(user(conversationItems([{ kind: "user.message", text: "hi", images: [] }])[0]))
      .not.toHaveProperty("images");
  });
});

describe("countUserTurns", () => {
  const say = (text: string): ConvItem => ({ kind: "user", id: text, text });

  it("counts only top-level user items with exactly this text", () => {
    const items: ConvItem[] = [
      say("run the tests"), { kind: "text", id: "t", text: "run the tests", streaming: false },
      say("run the tests"), say("something else"),
    ];
    expect(countUserTurns(items, "run the tests")).toBe(2);
    expect(countUserTurns(items, "never said")).toBe(0);
  });

  it("compares trimmed, because an engine may store the prompt with a newline", () => {
    expect(countUserTurns([say("ship it\n")], "ship it")).toBe(1);
  });

  it("does not count a subagent's own prompt as something the human typed", () => {
    const card: ConvItem = { kind: "subagent", id: "a1", subagentType: "reviewer",
      description: "review", status: "done", children: [say("dig here")], toolCount: 0,
      subagentCount: 0 };
    expect(countUserTurns([card], "dig here")).toBe(0);
  });
});

describe("settledEchoRowKey", () => {
  // The other half of mergeTranscript's drop rule, for the caller that has to
  // FOLLOW a turn across it. An ACP engine streams the user's own message back
  // live, so a row that was `e:0` when it was sent is `l:user-3` a second
  // later; anything the shell hung on the old key has to move with it.
  const say = (id: string, text: string): ConvItem => ({ kind: "user", id, text });
  const echo = (over: Partial<PendingEcho> = {}): PendingEcho =>
    ({ id: "0", text: "delete the parser", seen: 0, at: 0, historyKnown: true, ...over });

  it("is null while nothing has supplied the echo", () => {
    expect(settledEchoRowKey([], [], echo())).toBeNull();
    expect(settledEchoRowKey([], [say("user-1", "something else")], echo())).toBeNull();
  });

  it("names the live row that supplied it — the same one mergeTranscript drops for", () => {
    const live = [say("user-3", "delete the parser")];
    expect(settledEchoRowKey([], live, echo())).toBe("l:user-3");
    // And the pane agrees: with that live turn present the echo is gone.
    expect(mergeTranscript([], live, [echo()]).map(r => r.key)).toEqual(["l:user-3"]);
  });

  it("names a history row when the read is what supplied it", () => {
    expect(settledEchoRowKey([say("user-7", "delete the parser")], [], echo()))
      .toBe("h:user-7");
  });

  it("counts past the copies the echo was frozen against", () => {
    // seen: 1 means one identical turn already stood there when this was sent,
    // so it is the SECOND match that settles this echo, never the first.
    const live = [say("user-1", "delete the parser"), say("user-9", "delete the parser")];
    expect(settledEchoRowKey([], live, echo({ seen: 1 }))).toBe("l:user-9");
    expect(settledEchoRowKey([], [live[0]], echo({ seen: 1 }))).toBeNull();
  });

  it("leaves history out when the echo was frozen against an uncounted one", () => {
    const past = [say("user-7", "delete the parser")];
    expect(settledEchoRowKey(past, [], echo({ historyKnown: false }))).toBeNull();
    expect(settledEchoRowKey(past, [say("user-2", "delete the parser")],
      echo({ historyKnown: false }))).toBe("l:user-2");
  });

  it("never settles a refused send", () => {
    // Rule 3: it was never delivered, so nothing can legitimately supply it —
    // and the row it keeps is its own.
    expect(settledEchoRowKey([], [say("user-3", "delete the parser")],
      echo({ error: "the engine did not accept it" }))).toBeNull();
  });
});

describe("mergeTranscript", () => {
  const text = (id: string, body: string): ConvItem =>
    ({ kind: "text", id, text: body, streaming: false });
  const tool = (id: string): ConvItem =>
    ({ kind: "tool", id, tool: "Bash", summary: "ls", input: {} });

  it("keeps history first, then the seam, then live, in the order given", () => {
    const rows = mergeTranscript([text("text-0", "old"), text("text-1", "older")],
      [text("text-0", "new")]);
    expect(rows.map(r => r.kind)).toEqual(["item", "item", "mark", "item"]);
    expect(rows.map(r => r.key))
      .toEqual(["h:text-0", "h:text-1", "mark:live", "l:text-0"]);
  });

  it("namespaces keys so a history and a live block cannot share a React key", () => {
    // Generated ids restart at the same values in each list; one key for both
    // would show one block with the other's expansion state.
    const rows = mergeTranscript([text("text-0", "old")], [text("text-0", "new")]);
    expect(new Set(rows.map(r => r.key)).size).toBe(rows.length);
  });

  it("hands an unchanged item the same row object on the next frame", () => {
    // The display pass and the row memos below it compare by identity, so a
    // streamed token must leave every row it did not touch exactly as it was.
    const settled = [text("text-0", "old"), tool("call-1")];
    const echo: PendingEcho = { id: "e1", text: "go", seen: 0, at: 1, historyKnown: true };
    const first = mergeTranscript(settled, [text("text-0", "stream")], [echo]);
    const tail = text("text-0", "streaming on");
    const second = mergeTranscript(settled, [tail], [echo]);
    expect(second).toHaveLength(first.length);
    second.forEach((row, index) => {
      if (row.kind === "item" && row.item === tail) expect(row).not.toBe(first[index]);
      else expect(row).toBe(first[index]);
    });
    expect(second).toEqual(mergeTranscript([...settled], [{ ...tail }],
      [{ ...echo }]));
  });

  it("draws no seam when either half is empty", () => {
    expect(mergeTranscript([text("text-0", "old")], []).map(r => r.kind))
      .toEqual(["item"]);
    expect(mergeTranscript([], [text("text-0", "new")]).map(r => r.kind))
      .toEqual(["item"]);
    expect(mergeTranscript([], [])).toEqual([]);
  });

  it("drops a history copy of a turn the live list already holds, by engine id", () => {
    const rows = mergeTranscript([tool("call-1"), text("text-9", "prose")],
      [tool("call-1")]);
    expect(rows.map(r => r.key)).toEqual(["h:text-9", "mark:live", "l:call-1"]);
  });

  it("never removes a live row, even when history holds the same id twice", () => {
    const rows = mergeTranscript([tool("call-1"), tool("call-1")], [tool("call-1")]);
    expect(rows.filter(r => r.kind === "item" && r.origin === "live")).toHaveLength(1);
  });

  it("dedupes approvals and subagent cards, which are engine-minted too", () => {
    const approval: ConvItem = { kind: "approval", id: "ap-1", tool: "Write",
      summary: "write it", input: {} };
    const card: ConvItem = { kind: "subagent", id: "a1", subagentType: "reviewer",
      description: "review", status: "done", children: [], toolCount: 0,
      subagentCount: 0 };
    const rows = mergeTranscript([approval, card], [approval, card]);
    // And no seam: the dedupe emptied the history half, so there is nothing on
    // the other side of it. "Content on both sides" is decided after the drops.
    expect(rows.map(r => r.key)).toEqual(["l:ap-1", "l:a1"]);
  });

  it("never dedupes prose, notices or diffs — those ids are positions, not identities", () => {
    // history's `text-0` and live's `text-0` are different blocks; dropping one
    // would delete a real turn the user said or the agent wrote.
    const notice: ConvItem = { kind: "notice", id: "notice-0", text: "boom" };
    const diff: ConvItem = { kind: "diff", id: "diff-0", callId: "c-diff-0", path: "/tmp/a.ts" };
    const said: ConvItem = { kind: "user", id: "user-0", text: "again" };
    const rows = mergeTranscript([text("text-0", "a"), notice, diff, said],
      [text("text-0", "a"), notice, diff, said]);
    expect(rows.filter(r => r.kind === "item")).toHaveLength(8);
  });
});

describe("mergeTranscript — this window's own sends", () => {
  // `at` defaults past the end of every live list these tests build, which is
  // "queued after everything currently on screen" — the ordinary case.
  const AFTER_ALL = 1000;
  const echo = (id: string, text: string, seen = 0, at = AFTER_ALL): PendingEcho =>
    ({ id, text, seen, at, historyKnown: true });
  const said = (id: string, text: string): ConvItem => ({ kind: "user", id, text });
  const text = (id: string, body: string): ConvItem =>
    ({ kind: "text", id, text: body, streaming: false });

  it("renders a sent message as a user item, after everything live", () => {
    // The engines never send this back, so without it the composer empties and
    // the transcript does not change until the agent starts replying.
    const rows = mergeTranscript([], [text("text-0", "working")],
      [echo("0", "ship it", 0, 1)]);
    expect(rows.map(r => r.key)).toEqual(["l:text-0", "e:0"]);
    const last = rows[1];
    expect(last.kind === "item" && last.item).toMatchObject(
      { kind: "user", text: "ship it" });
    expect(last.kind === "item" && last.origin).toBe("echo");
  });

  it("counts as new content for the seam, even with an empty live list", () => {
    const rows = mergeTranscript([text("text-0", "past")], [], [echo("0", "hello")]);
    expect(rows.map(r => r.kind)).toEqual(["item", "mark", "item"]);
  });

  it("drops the echo once a history read supplies the same message", () => {
    // The one case the two lists really can overlap: the read lands after the
    // send and includes it. Without this the message renders twice.
    const rows = mergeTranscript([said("user-0", "ship it")], [], [echo("0", "ship it", 0)]);
    expect(rows.map(r => r.key)).toEqual(["h:user-0"]);
  });

  it("keeps a message the user deliberately sent twice", () => {
    // `seen` was 1 when the second one was sent, so the transcript holding one
    // copy is not evidence that this echo has been supplied.
    const rows = mergeTranscript([said("user-0", "again")], [], [echo("1", "again", 1)]);
    expect(rows.map(r => r.key)).toEqual(["h:user-0", "mark:live", "e:1"]);
  });

  it("drops only the echoes the transcript has caught up with", () => {
    const rows = mergeTranscript([], [said("user-0", "one")],
      [echo("0", "one", 0), echo("1", "two", 0)]);
    expect(rows.map(r => r.key)).toEqual(["l:user-0", "e:1"]);
  });

  it("matches trimmed, so a stored trailing newline still cancels the echo", () => {
    const rows = mergeTranscript([said("user-0", "ship it\n")], [], [echo("0", "ship it")]);
    expect(rows.map(r => r.key)).toEqual(["h:user-0"]);
  });

  it("keys echoes in their own namespace", () => {
    const rows = mergeTranscript([], [said("l0", "x")], [echo("l0", "y")]);
    expect(rows.map(r => r.key)).toEqual(["l:l0", "e:l0"]);
  });

  it("is unchanged by an empty echo list", () => {
    expect(mergeTranscript([text("text-0", "a")], [text("text-1", "b")], []))
      .toEqual(mergeTranscript([text("text-0", "a")], [text("text-1", "b")]));
  });
});

describe("launchPendingEcho", () => {
  it("puts the launch prompt on the empty live transcript", () => {
    // createSession never emits user.message, and a session started here does
    // not load history, so without this echo the first prompt exists only as
    // the sidebar title.
    const echo = launchPendingEcho("Build me a harness");
    const rows = mergeTranscript([], [], [echo]);
    // Before every live item, the compaction notice included: the launch
    // prompt is the oldest thing in the session.
    expect(echo).toMatchObject({
      id: "launch", text: "Build me a harness", seen: 0, at: Number.NEGATIVE_INFINITY,
      historyKnown: true,
    });
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({
      origin: "echo",
      item: { kind: "user", text: "Build me a harness" },
    });
  });

  it("carries attached pictures the same way a later send does", () => {
    const images = [{ src: "data:image/png;base64,abc", label: "1" }];
    expect(launchPendingEcho("look at this", images).images).toEqual(images);
    expect("images" in launchPendingEcho("plain")).toBe(false);
  });
});

describe("mergeTranscript — a send stands ABOVE the answer to it", () => {
  const echo = (id: string, text: string, at: number): PendingEcho =>
    ({ id, text, seen: 0, at, historyKnown: true });
  const text = (id: string, body: string): ConvItem =>
    ({ kind: "text", id, text: body, streaming: false });

  it("keeps the question above the reply it prompted", () => {
    // The whole defect in one assertion. The user asked "what broke?" when the
    // live list held one item; the agent then answered. Appending echoes after
    // the live list — which is what this replaced — puts the question BELOW
    // the answer, and leaves it there for the life of the app run.
    const rows = mergeTranscript([], [text("t0", "hi there"), text("t1", "the build did")],
      [echo("0", "what broke?", 1)]);
    expect(rows.map(r => r.key)).toEqual(["l:t0", "e:0", "l:t1"]);
  });

  it("holds the order across several exchanges", () => {
    // Four rows of a real conversation, alternating, from two sends whose
    // positions were frozen one live item apart.
    const rows = mergeTranscript([],
      [text("t0", "hi"), text("t1", "the build did"), text("t2", "fixed, tests green")],
      [echo("0", "what broke?", 1), echo("1", "fix it", 2)]);
    expect(rows.map(r => r.key)).toEqual(["l:t0", "e:0", "l:t1", "e:1", "l:t2"]);
  });

  it("puts a send made before any live output at the very top of the live half", () => {
    const rows = mergeTranscript([text("h0", "yesterday")], [text("t0", "on it")],
      [echo("0", "start", 0)]);
    expect(rows.map(r => r.key)).toEqual(["h:h0", "mark:live", "e:0", "l:t0"]);
  });

  it("orders by position, and keeps send order within one position", () => {
    // Two messages typed without waiting: same `at`, and a stable sort has to
    // leave them in the order they were typed.
    const rows = mergeTranscript([], [text("t0", "a")],
      [echo("1", "second", 1), echo("0", "first", 0)]);
    expect(rows.map(r => r.key)).toEqual(["e:0", "l:t0", "e:1"]);
    const same = mergeTranscript([], [], [echo("0", "first", 0), echo("1", "second", 0)]);
    expect(same.map(r => r.key)).toEqual(["e:0", "e:1"]);
  });

  it("falls back to the end for a position the live list never reached", () => {
    const rows = mergeTranscript([], [text("t0", "a")], [echo("0", "q", 99)]);
    expect(rows.map(r => r.key)).toEqual(["l:t0", "e:0"]);
  });
});

describe("mergeTranscript — an echo the history was never counted against", () => {
  const said = (id: string, text: string): ConvItem => ({ kind: "user", id, text });

  it("survives a history read that merely contains the same words", () => {
    // Sent while the read was still in flight: `seen` was frozen at 0 against
    // an empty stand-in for a transcript that already held one "continue".
    // Comparing that number to the loaded history deletes the message the user
    // just sent, and the pane then shows last week's copy and nothing else.
    const echo: PendingEcho =
      { id: "0", text: "continue", seen: 0, at: 0, historyKnown: false };
    const rows = mergeTranscript([said("user-0", "continue")], [], [echo]);
    expect(rows.map(r => r.key)).toEqual(["h:user-0", "mark:live", "e:0"]);
  });

  it("still yields to a copy the LIVE list supplies", () => {
    // The live half was counted, so it can still cancel the echo — this is not
    // a licence to render every echo forever.
    const echo: PendingEcho =
      { id: "0", text: "continue", seen: 0, at: 0, historyKnown: false };
    const rows = mergeTranscript([], [said("user-0", "continue")], [echo]);
    expect(rows.map(r => r.key)).toEqual(["l:user-0"]);
  });

  it("yields to history once the count was taken against it", () => {
    const echo: PendingEcho =
      { id: "0", text: "continue", seen: 0, at: 0, historyKnown: true };
    expect(mergeTranscript([said("user-0", "continue")], [], [echo]).map(r => r.key))
      .toEqual(["h:user-0"]);
  });
});

describe("mergeTranscript — a send the engine refused", () => {
  const said = (id: string, text: string): ConvItem => ({ kind: "user", id, text });
  const refused: PendingEcho = {
    id: "0", text: "ship it", seen: 0, at: 0, historyKnown: true,
    error: "Session is not running — resume it first",
  };

  it("says so directly under the message that never left", () => {
    const rows = mergeTranscript([], [], [refused]);
    expect(rows.map(r => r.key)).toEqual(["e:0", "e:0:err"]);
    const notice = rows[1];
    expect(notice.kind === "item" && notice.item).toEqual({
      kind: "notice", id: "echo-err-0",
      text: "Not delivered — Session is not running — resume it first",
    });
  });

  it("is never dropped by an unrelated copy of the same words", () => {
    // Nothing can legitimately supply a message that was never sent, so the
    // drop rule must not fire here — it would take the failure notice with it
    // and leave the user believing the message went out.
    const rows = mergeTranscript([said("user-0", "ship it")], [], [refused]);
    expect(rows.map(r => r.key)).toEqual(["h:user-0", "mark:live", "e:0", "e:0:err"]);
  });

  it("carries no notice while the send is still in flight", () => {
    const pending: PendingEcho =
      { id: "0", text: "ship it", seen: 0, at: 0, historyKnown: true };
    expect(mergeTranscript([], [], [pending]).map(r => r.key)).toEqual(["e:0"]);
  });
});

describe("expandImportedRows", () => {
  // The exact grammar an imported session arrives in — one marker of each of
  // the four kinds, copied from a Codex rollout the app actually replayed.
  const IMPORTED_FIXTURE: readonly ConvItem[] = [
    { kind: "text", id: "text-0", text: "<EXTERNAL SESSION IMPORTED>", streaming: false },
    { kind: "user", id: "user-1",
      text: "[external unsupported block: image]\n\nHere is what it looks like now" },
    { kind: "text", id: "text-2", streaming: false, text: [
      "[external_agent_tool_call: Bash]",
      "description: List Python files in the project",
      "command: find . -name \"*.py\"",
      "[/external_agent_tool_call]",
    ].join("\n") },
    { kind: "text", id: "text-3", streaming: false, text: [
      "[external_agent_tool_result]",
      "a.py",
      "b.py",
      "[/external_agent_tool_result]",
    ].join("\n") },
  ];

  const expand = (items: readonly ConvItem[]): readonly TranscriptRow[] =>
    expandImportedRows(mergeTranscript(items, []));

  it("draws the four markers as rows the transcript already knows how to paint", () => {
    const rows = expand(IMPORTED_FIXTURE);
    expect(rows.map(row => row.kind === "item" ? row.item.kind : "mark"))
      .toEqual(["mark", "notice", "user", "tool"]);
    expect(rows[0]).toMatchObject({ kind: "mark", label: IMPORTED_SESSION_LABEL });
    expect(rows[1]).toMatchObject({ item: { kind: "notice", text: "image not imported" } });
    expect(rows[2]).toMatchObject({ item: { text: "Here is what it looks like now" } });
    expect(rows[3]).toMatchObject({ item: {
      kind: "tool", tool: "Bash", summary: "List Python files in the project",
      input: { description: "List Python files in the project",
        command: "find . -name \"*.py\"" },
      result: { ok: true, summary: "a.py\nb.py" },
    } });
  });

  it("keeps the marker text out of the prompt strip", () => {
    expect(transcriptPrompts(expand(IMPORTED_FIXTURE)))
      .toEqual([{ key: "h:user-1", text: "Here is what it looks like now" }]);
  });

  it("keeps the user turn's own row key so anchors and restore points still land", () => {
    const rows = expand(IMPORTED_FIXTURE);
    expect(rows.map(row => row.key))
      .toEqual(["h:text-0", "h:user-1~x0", "h:user-1", "h:text-2"]);
    expect(new Set(rows.map(row => row.key)).size).toBe(rows.length);
  });

  it("reads a failed result as a failed call", () => {
    const rows = expand([
      IMPORTED_FIXTURE[2],
      { kind: "text", id: "text-3", streaming: false, text:
        "[external_agent_tool_result: error]\nExit code 1\n[/external_agent_tool_result]" },
    ]);
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({ item: { result: { ok: false, summary: "Exit code 1" } } });
  });

  it("keeps prose that shares a message with a call block", () => {
    const rows = expand([{ kind: "text", id: "text-0", streaming: false, text:
      `Let me look.\n\n${String(IMPORTED_FIXTURE[2].kind === "text"
        ? IMPORTED_FIXTURE[2].text : "")}` }]);
    expect(rows.map(row => row.kind === "item" ? row.item.kind : "mark"))
      .toEqual(["text", "tool"]);
    expect(rows[0]).toMatchObject({ key: "h:text-0", item: { text: "Let me look." } });
    expect(rows[1]).toMatchObject({ key: "h:text-0~x1" });
  });

  it("keeps an orphaned result rather than dropping its output", () => {
    const rows = expand([IMPORTED_FIXTURE[3]]);
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({ item: { kind: "notice", text: "a.py\nb.py" } });
  });

  it("parses an imported message once and keeps its rows across frames", () => {
    const first = expand(IMPORTED_FIXTURE);
    const second = expand(IMPORTED_FIXTURE);
    expect(second).toHaveLength(first.length);
    second.forEach((row, index) => { expect(row).toBe(first[index]); });
    // Including the tool row the NEXT message's result was merged into.
    expect(second[3]).toMatchObject({ item: { result: { ok: true, summary: "a.py\nb.py" } } });
    // A new live tail leaves every imported row where it was.
    const grown = expandImportedRows(mergeTranscript(IMPORTED_FIXTURE,
      [{ kind: "text", id: "text-0", text: "live", streaming: true }]));
    first.forEach((row, index) => { expect(grown[index]).toBe(row); });
  });

  it("returns an unmarked transcript untouched, by identity", () => {
    const rows = mergeTranscript(
      [{ kind: "text", id: "text-0", text: "ordinary prose", streaming: false }], []);
    expect(expandImportedRows(rows)).toBe(rows);
  });

  it("leaves a message that merely mentions the markers exactly as it is", () => {
    // The gate is a substring test, so prose ABOUT the grammar reaches the
    // parser. A row it finds nothing in must come back as the row it was —
    // not a re-trimmed copy, which would break Markdown's memo every render.
    const row: ConvItem = { kind: "text", id: "text-0", streaming: false,
      text: "The exporter writes [external unsupported block: X] lines.\n" };
    const rows = mergeTranscript([row], []);
    expect(expandImportedRows(rows)[0]).toBe(rows[0]);
  });

  it("never reads a marker out of a fenced example", () => {
    // The whole-line case, which the mention above does not cover: an agent
    // DEMONSTRATING the grammar. Split here, the message would have come back
    // as a lone ``` row, a synthetic Bash card holding the example, and a
    // second lone ``` row.
    const row: ConvItem = { kind: "text", id: "text-0", streaming: false, text: [
      "The exporter writes this:",
      "",
      "```",
      "[external_agent_tool_call: Bash]",
      "command: ls",
      "[/external_agent_tool_call]",
      "```",
      "",
      "<EXTERNAL SESSION IMPORTED>  ← and this one starts the transcript.",
    ].join("\n") };
    const rows = mergeTranscript([row], []);
    expect(expandImportedRows(rows)[0]).toBe(rows[0]);
  });

  it("leaves an opening marker with no closing tag as prose", () => {
    // Without the close, readBlock used to swallow the whole rest of the
    // message into the tool card's input.
    const row: ConvItem = { kind: "text", id: "text-0", streaming: false, text: [
      "[external_agent_tool_call: Bash]",
      "and then everything after it, which is ordinary prose.",
    ].join("\n") };
    const rows = mergeTranscript([row], []);
    expect(expandImportedRows(rows)[0]).toBe(rows[0]);
  });

  it("still reads a closed block that sits after a fenced example", () => {
    const rows = expand([
      { kind: "text", id: "text-0", streaming: false, text:
        "```\nnot a marker\n```\n[external_agent_tool_call: Bash]\ncommand: ls"
        + "\n[/external_agent_tool_call]" },
    ]);
    expect(rows.map(row => row.kind === "item" ? row.item.kind : "mark"))
      .toEqual(["text", "tool"]);
    expect(rows[1]).toMatchObject({ item: { tool: "Bash", input: { command: "ls" } } });
  });

  it("falls through an empty description to the path", () => {
    const rows = expand([
      { kind: "text", id: "text-0", streaming: false, text:
        "[external_agent_tool_call: Read]\ndescription:\nfile: /repo/a.ts"
        + "\n[/external_agent_tool_call]" },
    ]);
    expect(rows[0]).toMatchObject({ item: { summary: "/repo/a.ts" } });
  });
});

describe("approvalVerdictKey", () => {
  it("scopes an approvalId to its session", () => {
    expect(approvalVerdictKey("claude#a", "ap-1"))
      .not.toBe(approvalVerdictKey("claude#b", "ap-1"));
  });

  it("cannot be spoofed by a session id that looks like a joined key", () => {
    // NUL-joined precisely because neither half can contain one.
    expect(approvalVerdictKey("claude#a", "b c"))
      .not.toBe(approvalVerdictKey("claude#a b", "c"));
  });
});

describe("the chord that answers a blocking request", () => {
  const chord = (over: Partial<Parameters<typeof approvalKeyVerdict>[0]> = {}) =>
    ({ key: "Enter", metaKey: true, ...over });

  it("takes Cmd or Ctrl plus Enter to allow and plus Backspace to deny", () => {
    expect(approvalKeyVerdict({ key: "Enter", metaKey: true }, false)).toBe("allow");
    expect(approvalKeyVerdict({ key: "Enter", ctrlKey: true }, false)).toBe("allow");
    expect(approvalKeyVerdict({ key: "Backspace", metaKey: true }, false)).toBe("deny");
    expect(approvalKeyVerdict({ key: "Backspace", ctrlKey: true }, false)).toBe("deny");
  });

  it("refuses the bare key, the wrong key, and the decorated chord", () => {
    // Bare Enter sends a prompt and bare Backspace edits text; neither may ever
    // reach a verdict. Shift+Enter is a newline and Alt+Enter steers a turn.
    expect(approvalKeyVerdict({ key: "Enter" }, false)).toBe(null);
    expect(approvalKeyVerdict({ key: "Backspace" }, false)).toBe(null);
    expect(approvalKeyVerdict(chord({ key: "Delete" }), false)).toBe(null);
    expect(approvalKeyVerdict(chord({ shiftKey: true }), false)).toBe(null);
    expect(approvalKeyVerdict(chord({ altKey: true }), false)).toBe(null);
  });

  /* THE GUARD. Cmd+Enter in the composer sends the prompt and Cmd+Enter in the
     steering queue saves an edit; a keystroke aimed at text must never be read
     as an answer to a request the user has not looked at. */
  it("never fires while the keystroke belongs to something text is typed in", () => {
    expect(approvalKeyTyping({ tagName: "TEXTAREA" })).toBe(true);
    expect(approvalKeyTyping({ tagName: "input" })).toBe(true);
    expect(approvalKeyTyping({ tagName: "SELECT" })).toBe(true);
    expect(approvalKeyTyping({ tagName: "DIV", isContentEditable: true })).toBe(true);
    expect(approvalKeyTyping({ tagName: "BUTTON" })).toBe(false);
    expect(approvalKeyTyping(null)).toBe(false);
    expect(approvalKeyTyping(undefined)).toBe(false);
    // And the rule refuses on that answer alone, for both verdicts.
    expect(approvalKeyVerdict(chord(), true)).toBe(null);
    expect(approvalKeyVerdict(chord({ key: "Backspace" }), true)).toBe(null);
  });

  /* WHICH CARD MAY PRINT IT. The pane's listener clicks the LAST
     data-approval-key button under its root, so exactly the last answerable
     card may advertise the chord — a chip on the card above it allows a command
     the reader was not looking at. */
  describe("the one card allowed to print it", () => {
    const ask = (id: string): ConvItem =>
      ({ kind: "approval", id, tool: "Bash", summary: id, input: {} });
    const active = (...ids: string[]) => new Set(ids);

    it("names the last request still awaiting an answer", () => {
      const items = [ask("ap-1"), ask("ap-2")];
      expect(keyedApprovalId(items, true, active("ap-1", "ap-2"), {}, "claude#a"))
        .toBe("ap-2");
    });

    it("moves up to the request still open when the lower one is answered", () => {
      const items = [ask("ap-1"), ask("ap-2")];
      const answered = {
        [approvalVerdictKey("claude#a", "ap-2")]: { verdict: "allow" as const },
      };
      expect(keyedApprovalId(items, true, active("ap-1", "ap-2"), answered, "claude#a"))
        .toBe("ap-1");
    });

    it("keeps the card whose verdict FAILED, because its buttons came back", () => {
      const items = [ask("ap-1"), ask("ap-2")];
      const failed = {
        [approvalVerdictKey("claude#a", "ap-2")]:
          { verdict: "allow" as const, error: "Request is no longer outstanding" },
      };
      expect(keyedApprovalId(items, true, active("ap-1", "ap-2"), failed, "claude#a"))
        .toBe("ap-2");
    });

    it("ignores requests the engine has dropped, and a replayed transcript", () => {
      const items = [ask("ap-1"), ask("ap-2")];
      expect(keyedApprovalId(items, true, active("ap-1"), {}, "claude#a")).toBe("ap-1");
      expect(keyedApprovalId(items, true, active(), {}, "claude#a")).toBe(null);
      expect(keyedApprovalId(items, false, active("ap-2"), {}, "claude#a")).toBe(null);
      expect(keyedApprovalId([], true, active("ap-1"), {}, "claude#a")).toBe(null);
    });

    it("reaches a request nested in a subagent, in document order", () => {
      const nested: ConvItem = {
        kind: "subagent", id: "sub-1", subagentType: "explore", description: "explore",
        status: "running", children: [ask("ap-nested")],
        toolCount: 0, subagentCount: 0,
      };
      // The child draws INSIDE its parent's card, so a top-level request AFTER
      // the subagent is still the later one.
      expect(keyedApprovalId([nested, ask("ap-2")], true,
        active("ap-nested", "ap-2"), {}, "claude#a")).toBe("ap-2");
      expect(keyedApprovalId([ask("ap-1"), nested], true,
        active("ap-1", "ap-nested"), {}, "claude#a")).toBe("ap-nested");
    });
  });

  /* REVEAL, THEN ANSWER. The chord is pane-scoped, so it reaches the keyed card
     wherever it is — including far above a paused reader, with nothing on
     screen but the amber capsule. Allowing a command nobody can see is the one
     outcome the approval card exists to prevent, so an off-screen card costs
     two presses: one to show it, one to answer it. */
  describe("what the chord does about a card you cannot see", () => {
    // A 600px scroller, and a card 100px tall.
    const step = (top: number) => approvalChordStep(top, top + 100, 600);

    it("answers a card the reader can see, whole or in part", () => {
      expect(step(0)).toBe("answer");
      expect(step(240)).toBe("answer");
      // Half off the bottom edge is still a card being looked at. Charging two
      // presses for it would teach the reader to press twice always, which is
      // how this rule quietly becomes the answer-blind one it replaced.
      expect(step(550)).toBe("answer");
      expect(step(-60)).toBe("answer");
      expect(step(599)).toBe("answer");
      expect(step(-99)).toBe("answer");
    });

    it("reveals a card that is off either edge, and answers nothing", () => {
      expect(step(700)).toBe("reveal");
      expect(step(600)).toBe("reveal");
      expect(step(-100)).toBe("reveal");
      expect(step(-4000)).toBe("reveal");
    });

    it("treats a zero-height scroller as somewhere nothing can be seen", () => {
      expect(approvalChordStep(0, 100, 0)).toBe("reveal");
    });
  });

  it("prints the keys it binds, in one place both surfaces read", () => {
    expect(APPROVAL_ALLOW_KEYS.join("")).toBe("⌘↩");
    expect(APPROVAL_DENY_KEYS.join("")).toBe("⌘⌫");
    expect(APPROVAL_ALLOW_KEYSHORTCUTS).toBe("Meta+Enter Control+Enter");
    expect(APPROVAL_DENY_KEYSHORTCUTS).toBe("Meta+Backspace Control+Backspace");
    expect(APPROVAL_KEY_ATTR).toBe("data-approval-key");
  });
});

describe("truncationNotice", () => {
  it("says how many older messages there are", () => {
    expect(truncationNotice(853)).toBe("853 older messages are not shown.");
  });

  it("says one message in the singular", () => {
    expect(truncationNotice(1)).toBe("1 older message is not shown.");
  });

  it("says nothing when the read dropped nothing", () => {
    expect(truncationNotice(0)).toBeNull();
    expect(truncationNotice(0, true)).toBeNull();   // the count is the specific claim
  });

  it("falls back to the boolean for a caller that has no count", () => {
    expect(truncationNotice(undefined, true)).toBe("Older messages are not shown.");
    expect(truncationNotice(undefined, false)).toBeNull();
    expect(truncationNotice()).toBeNull();
  });

  it("ignores a count that is not a real number", () => {
    expect(truncationNotice(Number.NaN, true)).toBe("Older messages are not shown.");
    expect(truncationNotice(Number.POSITIVE_INFINITY, true))
      .toBe("Older messages are not shown.");
    expect(truncationNotice(-5)).toBeNull();
  });
});

describe("echoSeen", () => {
  const say = (id: string, text: string): ConvItem => ({ kind: "user", id, text });

  it("counts the copies already in both engine lists", () => {
    expect(echoSeen("ok", [say("u0", "ok")], [say("u1", "ok")], [])).toBe(2);
    expect(echoSeen("ok", [say("u0", "other")], [], [])).toBe(0);
  });

  it("counts an echo already queued for the same words", () => {
    // The off-by-one this exists for: two identical sends must not freeze the
    // same threshold, or one supplied copy cancels both of them.
    const queued = (id: string, seen: number): PendingEcho =>
      ({ id, text: "ok", seen, at: 0, historyKnown: true });
    const first = queued("0", echoSeen("ok", [], [], []));
    const second = queued("1", echoSeen("ok", [], [], [first]));
    expect([first.seen, second.seen]).toEqual([0, 1]);

    // One copy arrives from a history read: it cancels the first send only.
    expect(mergeTranscript([say("u0", "ok")], [], [first, second]).map(r => r.key))
      .toEqual(["h:u0", "mark:live", "e:1"]);
    // The second arrives too, and now both are accounted for.
    expect(mergeTranscript([say("u0", "ok"), say("u1", "ok")], [], [first, second])
      .map(r => r.key)).toEqual(["h:u0", "h:u1"]);
  });

  it("ignores whitespace on either side, like the drop rule it feeds", () => {
    expect(echoSeen("ok", [say("u0", " ok\n")], [],
      [{ id: "0", text: "ok ", seen: 0, at: 0, historyKnown: true }])).toBe(2);
  });
});

describe("transcriptSearchMatches", () => {
  const searchable: ConvItem[] = [
    { kind: "text", id: "assistant", text: "Assistant Alpha", streaming: false },
    { kind: "user", id: "user", text: "User Bravo" },
    { kind: "tool", id: "tool", tool: "Bash Charlie", summary: "run checks",
      input: { command: "payload Delta" }, result: { ok: true, summary: "result Echo" } },
    { kind: "approval", id: "approval", tool: "Write Foxtrot", summary: "edit file",
      input: { path: "approval Golf" } },
    { kind: "diff", id: "diff", callId: "c-diff", path: "/work/diff-Hotel.ts" },
    { kind: "notice", id: "notice", text: "Notice India" },
    { kind: "subagent", id: "sub", subagentType: "Reviewer Juliet",
      description: "audit changes", model: "model-Kilo", status: "done",
      lastActivity: "activity Lima", toolCount: 0, subagentCount: 0,
      children: [{ kind: "text", id: "nested", text: "Nested Mike", streaming: false }] },
  ];
  const rows = mergeTranscript([], searchable);

  it("finds every item kind, folded payload text, and a nested subagent child", () => {
    const cases: Array<[string, string]> = [
      ["ALPHA", "l:assistant"],
      ["bravo", "l:user"],
      ["charlie", "l:tool"],
      ["delta", "l:tool"],
      ["echo", "l:tool"],
      ["foxtrot", "l:approval"],
      ["golf", "l:approval"],
      ["hotel", "l:diff"],
      ["india", "l:notice"],
      ["juliet", "l:sub"],
      ["kilo", "l:sub"],
      ["lima", "l:sub"],
      ["mike", "l:sub/0"],
    ];
    for (const [query, key] of cases) {
      expect(transcriptSearchMatches(rows, query).map(match => match.key)).toEqual([key]);
    }
  });

  it("uses literal substring matching and returns one navigable result per item", () => {
    const literal: ConvItem = { kind: "text", id: "literal",
      text: "[a-z]+ appears twice: [A-Z]+", streaming: false };
    const literalRows = mergeTranscript([], [literal]);
    expect(transcriptSearchMatches(literalRows, "[a-z]+")).toEqual([
      { key: "l:literal", kind: "text" },
    ]);
    expect(transcriptSearchMatches(literalRows, "does-not-exist")).toEqual([]);
    expect(transcriptSearchMatches(literalRows, "")).toEqual([]);
  });

  it("indexes the Claude-style activity names painted for MCP tools", () => {
    const rows = mergeTranscript([], [{
      kind: "tool", id: "mcp", tool: "node_repl/js", summary: "node_repl/js",
      input: { code: "1+1" }, result: { ok: true, summary: "2" },
    }]);
    for (const query of ["JavaScript", "Node REPL", "Called Node REPL"]) {
      expect(transcriptSearchMatches(rows, query)).toEqual([
        { key: "l:mcp", kind: "tool" },
      ]);
    }
    // Presentation aliases augment the exact identifier; they do not replace
    // its diagnostic/search path.
    expect(transcriptSearchMatches(rows, "node_repl/js")).toEqual([
      { key: "l:mcp", kind: "tool" },
    ]);
  });

  it("keeps rendered history/live/depth-first order", () => {
    const history: ConvItem[] = [
      { kind: "text", id: "past", text: "hit past", streaming: false },
    ];
    const nested: ConvItem = { kind: "subagent", id: "agent", subagentType: "hit agent",
      description: "review", status: "done", children: [
        { kind: "notice", id: "child", text: "hit child" },
      ], toolCount: 0, subagentCount: 0 };
    const live: ConvItem[] = [nested, { kind: "user", id: "now", text: "hit now" }];
    expect(transcriptSearchMatches(mergeTranscript(history, live), "hit").map(m => m.key))
      .toEqual(["h:past", "l:agent", "l:agent/0", "l:now"]);
  });

  it("builds folded payload text once, then reuses its lower-cased corpus", () => {
    let reads = 0;
    const input = {} as Record<string, unknown>;
    Object.defineProperty(input, "command", {
      enumerable: true,
      get: () => { reads += 1; return "Run Needle Command"; },
    });
    const corpus = buildTranscriptSearchCorpus(mergeTranscript([], [{
      kind: "tool", id: "once", tool: "Bash", summary: "run", input,
    }]));
    expect(reads).toBe(1);
    expect(searchTranscriptCorpus(corpus, "NEEDLE").matches.map(match => match.key))
      .toEqual(["l:once"]);
    expect(searchTranscriptCorpus(corpus, "command").matches.map(match => match.key))
      .toEqual(["l:once"]);
    expect(reads).toBe(1);
  });

  it("collects each item once across corpus rebuilds while a transcript streams", () => {
    let reads = 0;
    const input = {} as Record<string, unknown>;
    Object.defineProperty(input, "command", {
      enumerable: true,
      get: () => { reads += 1; return "Run Needle Command"; },
    });
    const settled: ConvItem = { kind: "tool", id: "settled", tool: "Bash", summary: "run", input };
    const first = buildTranscriptSearchCorpus(mergeTranscript([], [settled,
      { kind: "text", id: "text-1", text: "Stream", streaming: true }]));
    const next = buildTranscriptSearchCorpus(mergeTranscript([], [settled,
      { kind: "text", id: "text-1", text: "Streaming on", streaming: true }]));
    expect(reads).toBe(1);
    expect(next.entries[0]).toEqual(first.entries[0]);
    expect(next.entries[1]?.text).toBe("streaming on");
    // A smaller corpus ceiling still cuts the cached text where it says.
    const bounded = buildTranscriptSearchCorpus(mergeTranscript([], [settled]), { maxCorpusChars: 8 });
    expect(bounded.entries[0]?.text).toBe(first.entries[0]?.text.slice(0, 8));
    expect(bounded.truncated).toBe(true);
    expect(reads).toBe(1);
  });

  it("bounds indexed text and results and discloses which count is partial", () => {
    const partialCorpus = buildTranscriptSearchCorpus(mergeTranscript([], [
      { kind: "text", id: "large", text: "x".repeat(100), streaming: false },
      { kind: "text", id: "later", text: "needle later", streaming: false },
    ]), { maxCorpusChars: 20, maxItemChars: 10 });
    expect(partialCorpus.truncated).toBe(true);
    expect(partialCorpus.entries.reduce((n, entry) => n + entry.text.length, 0))
      .toBeLessThanOrEqual(20);
    const partial = searchTranscriptCorpus(partialCorpus, "later");
    expect(partial.matches).toEqual([]);
    expect(transcriptSearchStatus("later", 0, partial))
      .toBe("0 matches · partial");

    const many = Array.from({ length: TRANSCRIPT_SEARCH_MAX_RESULTS + 1 }, (_, index) =>
      ({ kind: "text", id: `hit-${index}`, text: "hit", streaming: false }) as ConvItem);
    const capped = searchTranscriptCorpus(
      buildTranscriptSearchCorpus(mergeTranscript([], many)), "hit");
    expect(capped.matches).toHaveLength(TRANSCRIPT_SEARCH_MAX_RESULTS);
    expect(capped.resultsTruncated).toBe(true);
    expect(transcriptSearchStatus("hit", 0, capped))
      .toBe(`1 of ${TRANSCRIPT_SEARCH_MAX_RESULTS}+`);
  });

  it("caps programmatic queries even though the input also has maxLength", () => {
    const corpus = buildTranscriptSearchCorpus(mergeTranscript([], [
      { kind: "text", id: "query", text: "a".repeat(300), streaming: false },
    ]));
    const result = searchTranscriptCorpus(corpus,
      "a".repeat(TRANSCRIPT_SEARCH_MAX_QUERY_CHARS + 10));
    expect(result.queryTruncated).toBe(true);
    expect(result.matches).toHaveLength(1);
    expect(transcriptSearchStatus("oversized", 0, result))
      .toContain("query cap");
  });
});

describe("moveTranscriptSearchIndex", () => {
  it("wraps next and previous and clamps a stale index before moving", () => {
    expect(moveTranscriptSearchIndex(0, 3, 1)).toBe(1);
    expect(moveTranscriptSearchIndex(2, 3, 1)).toBe(0);
    expect(moveTranscriptSearchIndex(0, 3, -1)).toBe(2);
    expect(moveTranscriptSearchIndex(99, 3, -1)).toBe(1);
    expect(moveTranscriptSearchIndex(2, 0, 1)).toBe(0);
  });
});

/* ---- what is behind a reader who scrolled up ------------------------------ */

const ANSWERABLE: ReadonlySet<string> = new Set(["ask-1"]);

function askRow(id: string, resolved?: "allow" | "deny"): TranscriptRow {
  return { kind: "item", key: `l:${id}`, origin: "live",
    item: { kind: "approval", id, tool: "Bash", summary: "rm -rf /tmp/x", input: {},
      ...(resolved === undefined ? {} : { resolved }) } };
}

function textRow(id: string): TranscriptRow {
  return { kind: "item", key: `l:${id}`, origin: "live",
    item: { kind: "text", id, text: id, streaming: false } };
}

/** The unfolded pane: every transcript row is drawn on its own line. */
const oneEach = (rows: readonly TranscriptRow[]): string[][] =>
  rows.map(row => [row.key]);

describe("jumpLatestBacklog", () => {
  it("counts every drawn line that landed after the anchor", () => {
    const rows = [textRow("a"), textRow("b"), textRow("c"), textRow("d")];
    const drawn = oneEach(rows);
    expect(jumpLatestBacklog(rows, drawn, "l:b").count).toBe(2);
    expect(jumpLatestBacklog(rows, drawn, "l:a").count).toBe(3);
    // The reader was at the very bottom when the last row arrived.
    expect(jumpLatestBacklog(rows, drawn, "l:d").count).toBe(0);
  });

  it("counts a folded run as the one line the pane actually draws", () => {
    // THE COUNT IS THE RENDERED TRUTH, NOT THE ROW TOTAL. Twelve Bash calls
    // behind a paused reader resolve to a single folded line at every density
    // the dial offers; "12 new" over one line was the pane lying about itself.
    const rows = [textRow("a"), ...Array.from({ length: 12 },
      (_unused, index) => textRow(`bash-${String(index)}`))];
    const drawn = [["l:a"], rows.slice(1).map(row => row.key)];
    expect(jumpLatestBacklog(rows, drawn, "l:a").count).toBe(1);
  });

  it("counts the anchor's own line when the fold swallowed it and then grew", () => {
    // The reader paused on the first Bash call; eleven more landed and the pane
    // folded all twelve into the line their anchor is now inside. One drawn
    // line changed below them, and one is what they are told.
    const rows = Array.from({ length: 12 },
      (_unused, index) => textRow(`bash-${String(index)}`));
    const drawn = [rows.map(row => row.key)];
    expect(jumpLatestBacklog(rows, drawn, "l:bash-0").count).toBe(1);
    // Nothing after it at all: the run is entirely behind the anchor.
    expect(jumpLatestBacklog(rows, drawn, "l:bash-11").count).toBe(0);
  });

  it("does not count the live seam, which is not a message", () => {
    const rows: TranscriptRow[] = [
      textRow("a"), { kind: "mark", key: "mark:live" }, textRow("b"),
    ];
    expect(jumpLatestBacklog(rows, oneEach(rows), "l:a").count).toBe(1);
  });

  it("reports nothing while the reader is still following the stream", () => {
    const rows = [textRow("a"), textRow("b"), askRow("ask-1")];
    expect(jumpLatestBacklog(rows, oneEach(rows), null, ANSWERABLE))
      .toEqual({ count: 0, approvalWaiting: false });
  });

  it("counts nothing when the anchor row is no longer in the transcript", () => {
    const rows = [textRow("a"), textRow("b")];
    expect(jumpLatestBacklog(rows, oneEach(rows), "e:7").count).toBe(0);
  });

  it("raises the approval exception for a new answerable approval", () => {
    const rows = [textRow("a"), askRow("ask-1")];
    expect(jumpLatestBacklog(rows, oneEach(rows), "l:a", ANSWERABLE)).toEqual({
      count: 1, approvalWaiting: true,
    });
  });

  it("stays quiet for an ordinary message arriving behind the reader", () => {
    const rows = [textRow("a"), textRow("b"), textRow("c")];
    expect(jumpLatestBacklog(rows, oneEach(rows), "l:a", ANSWERABLE)).toEqual({
      count: 2, approvalWaiting: false,
    });
  });

  it("stays quiet for an approval nothing can still answer", () => {
    // A replayed transcript: the ids are real, the set the bridge will accept
    // is empty, and every old approval in it must raise nothing.
    const open = [textRow("a"), askRow("ask-1")];
    expect(jumpLatestBacklog(open, oneEach(open), "l:a").approvalWaiting).toBe(false);
    // Answered in this session: the card already carries its verdict.
    const answered = [textRow("a"), askRow("ask-1", "allow")];
    expect(jumpLatestBacklog(answered, oneEach(answered), "l:a", ANSWERABLE)
      .approvalWaiting).toBe(false);
  });

  it("still raises a pending approval when the anchor row has vanished", () => {
    // A lost anchor (an echo the history read supplied, a session switch) may
    // cost the count. It must never cost the one signal whose absence leaves
    // the agent blocked with nobody watching.
    const rows = [textRow("a"), askRow("ask-1")];
    expect(jumpLatestBacklog(rows, oneEach(rows), "e:7", ANSWERABLE))
      .toEqual({ count: 0, approvalWaiting: true });
  });

  it("scans for approvals over the transcript, not over what the fold left", () => {
    // The approval scan reads `rows` and the count reads `drawn` for exactly
    // this reason: a display list that lost track of a key must not be able to
    // lose the request the agent is blocked on.
    const rows = [textRow("a"), askRow("ask-1")];
    expect(jumpLatestBacklog(rows, [], "l:a", ANSWERABLE))
      .toEqual({ count: 0, approvalWaiting: true });
  });
});

describe("jumpLatestLabel", () => {
  it("names the count, and gives a narrow pane a shorter form to swap in", () => {
    const one = jumpLatestLabel({ count: 1, approvalWaiting: false });
    expect(one.text).toBe("1 new");
    expect(one.short).toBe("1");
    expect(one.aria).toBe("1 new below. Jump to latest.");
    expect(jumpLatestLabel({ count: 3, approvalWaiting: false }).text).toBe("3 new");
  });

  it("caps the number rather than widening the capsule without limit", () => {
    expect(jumpLatestLabel({ count: 99, approvalWaiting: false }).text).toBe("99 new");
    expect(jumpLatestLabel({ count: 100, approvalWaiting: false }).text).toBe("99+ new");
    expect(jumpLatestLabel({ count: 4210, approvalWaiting: false }).short).toBe("99+");
  });

  it("keeps the plain label when nothing has arrived", () => {
    const none = jumpLatestLabel({ count: 0, approvalWaiting: false });
    expect(none).toEqual({ text: "Latest", short: "Latest", aria: "Jump to latest" });
  });

  it("says an approval is waiting instead of a count, at both lengths", () => {
    const waiting = jumpLatestLabel({ count: 12, approvalWaiting: true });
    expect(waiting.text).toBe("Approval waiting");
    expect(waiting.short).toBe("Approval");
    expect(waiting.aria).toBe("An approval is waiting below. Jump to latest.");
  });
});

/* ---- which question the visible region is the answer to ------------------- */

function promptRow(id: string, text: string): TranscriptRow {
  return { kind: "item", key: `l:${id}`, origin: "live",
    item: { kind: "user", id, text } };
}

describe("transcriptPrompts", () => {
  it("lists the user's own turns, in transcript order, by row key", () => {
    const rows: TranscriptRow[] = [
      promptRow("user-1", "fix the queue"),
      textRow("a"),
      { kind: "mark", key: "mark:live" },
      promptRow("user-2", "now the tests"),
      askRow("ask-1"),
    ];
    expect(transcriptPrompts(rows)).toEqual([
      { key: "l:user-1", text: "fix the queue" },
      { key: "l:user-2", text: "now the tests" },
    ]);
  });

  it("flattens a pasted multi-line prompt into the one line the bar can draw", () => {
    // The bar is one line and the title attribute is one tooltip. A prompt that
    // arrives as three paragraphs must read as a sentence in both, rather than
    // as a run of raw newlines the browser collapses differently in each.
    const rows = [promptRow("user-1", "  fix the queue\n\n  then the tests \t\n")];
    expect(transcriptPrompts(rows)[0]?.text).toBe("fix the queue then the tests");
  });

  it("is empty for a transcript nobody has spoken into", () => {
    expect(transcriptPrompts([textRow("a"), askRow("ask-1")])).toEqual([]);
  });
});

describe("promptAnchorKey", () => {
  // Tops are measured from the TOP OF THE VISIBLE REGION, so a prompt scrolled
  // off the top is negative and one below the fold is past the height.
  const spots = [
    { key: "l:user-1", top: -900 },
    { key: "l:user-2", top: -400 },
    { key: "l:user-3", top: 700 },
  ];

  it("names the last prompt at or above the top of the visible region", () => {
    expect(promptAnchorKey(spots, 600)).toBe("l:user-2");
  });

  it("draws nothing while the newest prompt is on screen", () => {
    // The bar exists to name a prompt the reader cannot see. Repeating one they
    // are looking at costs a line of transcript and says nothing.
    expect(promptAnchorKey([{ key: "l:user-1", top: -900 },
      { key: "l:user-2", top: 120 }], 600)).toBeNull();
    // Its top edge exactly at the top of the region still counts as on screen.
    expect(promptAnchorKey([{ key: "l:user-1", top: 0 }], 600)).toBeNull();
  });

  it("names the newest prompt for a reader deep inside its answer", () => {
    // Scrolled past every prompt: the newest one governs, which is the common
    // case of reading the tail of a long turn.
    expect(promptAnchorKey([{ key: "l:user-1", top: -900 },
      { key: "l:user-2", top: -20 }], 600)).toBe("l:user-2");
  });

  it("draws nothing above the first prompt, where no prompt governs yet", () => {
    expect(promptAnchorKey([{ key: "l:user-1", top: 40 },
      { key: "l:user-2", top: 900 }], 600)).toBeNull();
  });

  it("draws nothing for a transcript with no prompts in it", () => {
    expect(promptAnchorKey([], 600)).toBeNull();
  });
});

describe("promptAnchorView", () => {
  const prompts = [
    { key: "l:user-1", text: "first" },
    { key: "l:user-2", text: "second" },
    { key: "l:user-3", text: "third" },
  ];

  it("carries the ordinal, the total and both steps", () => {
    expect(promptAnchorView(prompts, "l:user-2")).toEqual({
      key: "l:user-2", text: "second", ordinal: 2, total: 3,
      prevKey: "l:user-1", nextKey: "l:user-3",
    });
  });

  it("stops stepping at both ends rather than wrapping", () => {
    expect(promptAnchorView(prompts, "l:user-1")?.prevKey).toBeNull();
    expect(promptAnchorView(prompts, "l:user-1")?.nextKey).toBe("l:user-2");
    expect(promptAnchorView(prompts, "l:user-3")?.nextKey).toBeNull();
    expect(promptAnchorView(prompts, "l:user-3")?.ordinal).toBe(3);
  });

  it("draws nothing for a key this transcript no longer holds", () => {
    // A session switch, or an echo the history read superseded. The bar goes
    // rather than naming a prompt that is not there.
    expect(promptAnchorView(prompts, "e:7")).toBeNull();
    expect(promptAnchorView(prompts, null)).toBeNull();
    expect(promptAnchorView([], "l:user-1")).toBeNull();
  });
});

describe("jumpScrollBehavior", () => {
  it("asks for the reduced-motion preference and stops animating when it is set", () => {
    const asked: string[] = [];
    const behavior = jumpScrollBehavior(query => {
      asked.push(query);
      return { matches: true };
    });
    // The query itself is the assertion: theme.css cannot reach a JS scrollTo,
    // so this is the only place the preference is read for the jump.
    expect(asked).toEqual(["(prefers-reduced-motion: reduce)"]);
    expect(behavior).toBe("auto");
  });

  it("keeps the smooth scroll when nothing objects", () => {
    expect(jumpScrollBehavior(() => ({ matches: false }))).toBe("smooth");
  });

  it("keeps the smooth scroll where the preference cannot be read at all", () => {
    expect(jumpScrollBehavior()).toBe("smooth");
    expect(jumpScrollBehavior(() => { throw new Error("no matchMedia"); })).toBe("smooth");
  });
});

describe("prefersReducedMotion", () => {
  it("asks for the reduced-motion preference and reports that it is set", () => {
    const asked: string[] = [];
    const reduced = prefersReducedMotion(query => {
      asked.push(query);
      return { matches: true };
    });
    // Same query as the jump, for the same reason: no stylesheet can reach a
    // reveal that is paced from script, so the preference is read here too.
    expect(asked).toEqual(["(prefers-reduced-motion: reduce)"]);
    expect(reduced).toBe(true);
  });

  it("animates when nothing objects", () => {
    expect(prefersReducedMotion(() => ({ matches: false }))).toBe(false);
  });

  it("animates where the preference cannot be read at all", () => {
    expect(prefersReducedMotion()).toBe(false);
    expect(prefersReducedMotion(() => { throw new Error("no matchMedia"); })).toBe(false);
  });
});

describe("paced reveal", () => {
  type Arrival = { at: number; length: number };
  const FRAME = 1000 / 60;
  /** Paints `arrivals` at 60fps until `until` ms; the stream completes at `doneAt`. */
  function paint(arrivals: readonly Arrival[], until: number, doneAt = Infinity): number[] {
    let pace: RevealPace = settledPace(0);
    let next = 0;
    let done = false;
    const painted: number[] = [];
    for (let now = 0; now <= until; now += FRAME) {
      while (next < arrivals.length && arrivals[next].at <= now) {
        pace = paceArrival(pace, arrivals[next].length, arrivals[next].at, true);
        next += 1;
      }
      if (!done && now >= doneAt) {
        pace = paceArrival(pace, pace.length, doneAt, false);
        done = true;
      }
      pace = paceFrame(pace, now);
      painted.push(Math.floor(pace.shown));
    }
    return painted;
  }
  const steps = (painted: readonly number[]): number[] =>
    painted.slice(1).map((value, index) => value - painted[index]);
  /** What Opus 5.5 sent through Claude Code: a first delta, then ~280-character bursts. */
  const OPUS: Arrival[] = [
    { at: 0, length: 19 }, { at: 846, length: 300 }, { at: 1689, length: 580 },
    { at: 2532, length: 860 }, { at: 3375, length: 1140 },
  ];

  it("keeps an Opus-style burst flowing until the next one lands", () => {
    const painted = paint(OPUS, 3375);
    // From the second measured gap on, every frame paints more: no burst, then stall.
    const settledIn = steps(painted.slice(Math.ceil(1689 / FRAME) + 1));
    expect(settledIn.filter(step => step === 0)).toEqual([]);
    // And no frame paints a chunk: about 280 characters spread over a second.
    expect(Math.max(...steps(painted))).toBeLessThanOrEqual(12);
  });

  it("paints a steady trickle a frame or two behind", () => {
    // Haiku: twenty characters every 25 ms.
    const arrivals = Array.from({ length: 80 }, (_, index) =>
      ({ at: index * 25, length: (index + 1) * 20 }));
    const painted = paint(arrivals, 1990);
    for (const frame of [30, 60, 90, 119]) {
      const arrived = (Math.floor((frame * FRAME) / 25) + 1) * 20;
      expect(arrived - painted[frame]).toBeLessThanOrEqual(120);
    }
  });

  it("finishes a completed message within 400 ms instead of snapping", () => {
    const final = OPUS[OPUS.length - 1].length;
    const painted = paint(OPUS, 3375 + 600, 3380);
    expect(painted[Math.ceil(3380 / FRAME)]).toBeLessThan(final - 200);
    expect(painted[Math.ceil((3380 + 400) / FRAME) + 1]).toBe(final);
    expect(Math.max(...steps(painted))).toBeLessThan(60);
  });

  it("paints a block whole when it appears, and times the next burst from it when streaming", () => {
    const appeared = mountedPace(19, 0, true);
    expect(appeared.shown).toBe(19);
    const second = paceArrival(appeared, 300, 846, true);
    expect(second.gap).toBe(846);
    // Spread past the gap it measured, not over the first-burst default.
    expect(second.speed).toBeCloseTo(281 / (846 * 1.25), 6);
    expect(mountedPace(19, 0, false).burstAt).toBeNull();
  });

  it("treats arrivals closer than the burst window as one burst", () => {
    let pace = paceArrival(settledPace(0), 10, 0, true);
    pace = paceArrival(pace, 300, 800, true);
    expect(pace.gap).toBe(800);
    // The same burst split across a second commit leaves the gap alone.
    pace = paceArrival(pace, 320, 800 + REVEAL_BURST_MS - 1, true);
    expect(pace.gap).toBe(800);
    expect(pace.burstAt).toBe(800);
  });

  it("snaps one arrival of the flood size or more, and paces one character less", () => {
    // Resume floods and giant pastes are not typed at the reader: a replayed
    // transcript that typewrites itself is a bug, not an effect.
    expect(paceArrival(settledPace(0), REVEAL_FLOOD_CHARS, 0, true).shown).toBe(REVEAL_FLOOD_CHARS);
    expect(paceArrival(settledPace(300), 300 + REVEAL_FLOOD_CHARS, 0, true).shown)
      .toBe(300 + REVEAL_FLOOD_CHARS);
    expect(paceArrival(settledPace(0), REVEAL_FLOOD_CHARS - 1, 0, true).shown).toBe(0);
  });

  it("snaps a message past the reveal ceiling, however small the arrival", () => {
    const near = settledPace(REVEAL_SNAP_CHARS - 10);
    expect(paceArrival(near, REVEAL_SNAP_CHARS, 0, true).shown).toBe(REVEAL_SNAP_CHARS);
    expect(paceArrival(near, REVEAL_SNAP_CHARS - 1, 0, true).shown).toBe(REVEAL_SNAP_CHARS - 10);
  });

  it("snaps a tail replaced by something shorter rather than rewinding", () => {
    expect(paceArrival(settledPace(80), 50, 0, true).shown).toBe(50);
  });

  it("never paints past the tail, and counts a stalled frame as a short one", () => {
    const pace = paceArrival(settledPace(0), 100, 0, true);
    expect(paceFrame(pace, 10_000).shown).toBe(paceFrame(pace, 100).shown);
    expect(paceFrame(paceFrame(pace, 10_000), 20_000).shown).toBe(100);
  });
});

describe("toolInputBlocks", () => {
  // The exact payload the audit caught: a real Bash call from this app's own
  // transcript, whose command is three lines long.
  const BASH = {
    command: "cd /private/tmp/iface-pkg\nnpm run package 2>&1 | tail -14\n"
      + "echo \"PACKAGE_EXIT=${pipestatus[1]}\"",
    description: "Repackage with code signing",
    timeout: 900000,
  };

  it("prints the command as the command, with its line breaks intact", () => {
    const [body] = toolInputBlocks(BASH, "Repackage with code signing");
    expect(body.label).toBe("command");
    expect(body.text).toBe(BASH.command);
    expect(body.text.split("\n")).toHaveLength(3);
    // What the pane used to show instead: JSON.stringify spells every newline
    // as the two characters \ and n, on one unreadable line.
    expect(body.text).not.toContain("\\n");
  });

  it("omits leftover JSON once a body key was extracted", () => {
    // Bash-shaped payload: the command is the body; timeout and description
    // stay off the expanded card (they remain in the row title / find corpus).
    const blocks = toolInputBlocks(BASH, "Repackage with code signing");
    expect(blocks).toEqual([{ label: "command", text: BASH.command }]);
    const shown = blocks.map(block => block.text).join("\n");
    expect(shown).not.toContain("900000");
    expect(shown).not.toContain("description");
  });

  it("drops leftover keys even when the description is not the row summary", () => {
    const blocks = toolInputBlocks(BASH, "run the packager");
    expect(blocks).toEqual([{ label: "command", text: BASH.command }]);
  });

  it("leaves a payload with nothing to unescape exactly as it was", () => {
    const read = { file_path: "/private/tmp/iface-pkg/electron-builder.yml" };
    expect(toolInputBlocks(read, "read the builder config"))
      .toEqual([{ text: JSON.stringify(read, null, 2) }]);
  });

  it("gives a one-line command its own block too, because that is what ran", () => {
    const blocks = toolInputBlocks({ command: "npm run build" }, "");
    expect(blocks).toEqual([{ label: "command", text: "npm run build" }]);
  });

  it("unfolds both halves of an edit without the leftover path JSON", () => {
    const blocks = toolInputBlocks({
      file_path: "/a/b.ts", old_string: "one\ntwo", new_string: "one\ntwo\nthree",
    });
    expect(blocks.map(block => block.label)).toEqual(["old_string", "new_string"]);
    expect(blocks[1].text).toBe("one\ntwo\nthree");
    expect(blocks.map(block => block.text).join("\n")).not.toContain("/a/b.ts");
  });

  it("unescapes a multi-line argument no list anticipated", () => {
    // The engines mint tool names and argument names freely — an MCP call can
    // carry its body under any key at all — so the shape of the value decides,
    // and the key list is only what catches a body that happens to be one line.
    const blocks = toolInputBlocks({ target: "/a/b.ts", replacement: "one\ntwo\nthree" });
    expect(blocks.map(block => block.label)).toEqual(["replacement", undefined]);
    expect(blocks[0].text).toBe("one\ntwo\nthree");
    expect(blocks[1].text).toContain("/a/b.ts");
  });

  it("renders a string payload as itself and an absent one as nothing", () => {
    expect(toolInputBlocks("ls -la")).toEqual([{ text: "ls -la" }]);
    expect(toolInputBlocks(undefined)).toEqual([]);
    expect(toolInputBlocks("")).toEqual([]);
  });

  it("survives a payload JSON cannot serialize", () => {
    const cyclic: Record<string, unknown> = { command: "run" };
    cyclic.self = cyclic;
    const blocks = toolInputBlocks(cyclic, "");
    expect(blocks).toEqual([{ label: "command", text: "run" }]);
  });
});

describe("workGroupLabel", () => {
  it("writes Claude Code's mixed work sentence", () => {
    expect(workGroupLabel({
      tools: [
        { tool: "Bash", summary: "ls" },
        { tool: "Bash", summary: "pwd" },
        { tool: "Edit", summary: "adapter.ts", input: { file_path: "/repo/adapter.ts" } },
        { tool: "Read", summary: "other.ts" },
      ],
      diffs: [{ path: "/repo/adapter.ts", added: 21, removed: 6 }],
      running: false,
      root: "/repo",
    })).toBe("Ran 2 commands, edited adapter.ts +21 −6");
  });

  it("uses created when every mutation is a write", () => {
    expect(workGroupLabel({
      tools: [{ tool: "Write", summary: "New.tsx", input: { file_path: "/repo/New.tsx" } }],
      diffs: [{ path: "/repo/New.tsx", added: 122, removed: 0 }],
      running: false,
      root: "/repo",
    })).toBe("created New.tsx +122 −0");
  });

  it("hides reads and searches when a command already ran", () => {
    expect(workGroupLabel({
      tools: [
        { tool: "Read", summary: "a.ts" },
        { tool: "Grep", summary: "foo" },
        { tool: "Bash", summary: "ls" },
      ],
      running: false,
    })).toBe("Ran a command");
  });

  it("names only the inspection work when nothing ran or changed", () => {
    expect(workGroupLabel({
      tools: [
        { tool: "Read", summary: "a.ts" },
        { tool: "Read", summary: "b.ts" },
        { tool: "Grep", summary: "foo" },
        { tool: "Grep", summary: "bar" },
      ],
      running: false,
    })).toBe("Read 2 files, Searched 2 times");
  });
});

describe("editedFilesLabel", () => {
  it("prints the line counts when this window saw them", () => {
    expect(editedFilesLabel(3, { added: 12, removed: 4 })).toBe("Edited 3 files +12 −4");
  });

  it("drops the pair when counts are unknown or a net-zero", () => {
    expect(editedFilesLabel(2, null)).toBe("Edited 2 files");
    expect(editedFilesLabel(2, undefined)).toBe("Edited 2 files");
    expect(editedFilesLabel(2, { added: 0, removed: 0 })).toBe("Edited 2 files");
  });

  it("never prints +0 −0", () => {
    expect(editedFilesLabel(4, { added: 0, removed: 0 })).not.toContain("+0");
    expect(editedFilesLabel(4, null)).not.toContain("−0");
  });
});

/* ==========================================================================
   WHAT ONE TURN CHANGED. The attribution window is the whole idea: a diff
   belongs to the LAST prompt above it, so the chip on a user turn describes
   the work that turn caused and never the work of the one before or after.
   ========================================================================== */

describe("turnChangeSummaries", () => {
  const at = (key: string, item: ConvItem): TranscriptRow =>
    ({ kind: "item", key, origin: "live", item });
  const said = (id: string): ConvItem => ({ kind: "user", id, text: id });
  const wrote = (id: string, path: string, added = 1, removed = 0): ConvItem =>
    ({ kind: "diff", id, path, callId: `c-${id}`, added, removed });

  it("attributes a diff to the prompt above it and not to the one after", () => {
    const summaries = turnChangeSummaries([
      at("l:u0", said("u0")),
      at("l:d0", wrote("d0", "/repo/a.ts", 12, 4)),
      at("l:u1", said("u1")),
    ]);
    expect(summaries.get("l:u0")).toEqual({ stat: { added: 12, removed: 4 } });
    // The second prompt has produced nothing yet, so it says nothing at all.
    expect(summaries.has("l:u1")).toBe(false);
  });

  it("counts every edit's lines, even when the turn re-touched a file", () => {
    const summaries = turnChangeSummaries([
      at("l:u0", said("u0")),
      at("l:d0", wrote("d0", "/repo/a.ts", 3, 1)),
      at("l:d1", wrote("d1", "/repo/a.ts", 2, 0)),
      at("l:d2", wrote("d2", "/repo/b.ts", 5, 5)),
    ]);
    // Every line of all three edits: an edit re-touching a file still changed lines.
    expect(summaries.get("l:u0")).toEqual({ stat: { added: 10, removed: 6 } });
  });

  it("gives each turn only its own window", () => {
    const summaries = turnChangeSummaries([
      at("l:u0", said("u0")),
      at("l:d0", wrote("d0", "/repo/a.ts", 1, 0)),
      at("l:u1", said("u1")),
      at("l:d1", wrote("d1", "/repo/b.ts", 2, 0)),
      at("l:d2", wrote("d2", "/repo/c.ts", 3, 0)),
    ]);
    expect(summaries.get("l:u0")).toEqual({ stat: { added: 1, removed: 0 } });
    expect(summaries.get("l:u1")).toEqual({ stat: { added: 5, removed: 0 } });
  });

  it("drops work no prompt in this transcript can claim", () => {
    // A history read that begins mid-turn: the prompt that caused these edits
    // is above the window. Hanging them on the next prompt would blame it for
    // changes it has not made yet.
    const summaries = turnChangeSummaries([
      at("h:d0", wrote("d0", "/repo/a.ts")),
      at("h:u0", said("u0")),
    ]);
    expect(summaries.size).toBe(0);
  });

  it("does not end a window at the live seam", () => {
    // The prompt was sent before this window opened and its edits arrived
    // after: one turn, drawn across the seam, and one honest count.
    const summaries = turnChangeSummaries([
      at("h:u0", said("u0")),
      { kind: "mark", key: "mark:live" },
      at("l:d0", wrote("d0", "/repo/a.ts", 7, 2)),
    ]);
    expect(summaries.get("h:u0")).toEqual({ stat: { added: 7, removed: 2 } });
  });

  it("counts what a subagent wrote as the turn's own work", () => {
    // The human typed one prompt; a spawned agent did the editing. The files
    // changed either way, and a chip that ignored them would say nothing over
    // a turn that rewrote the repository.
    const summaries = turnChangeSummaries([
      at("l:u0", said("u0")),
      at("l:a1", { kind: "subagent", id: "a1", subagentType: "builder",
        description: "build", status: "done", toolCount: 1, subagentCount: 0,
        children: [
          wrote("in0", "/repo/a.ts", 4, 1),
          { kind: "subagent", id: "a2", subagentType: "helper", description: "help",
            status: "done", toolCount: 0, subagentCount: 0,
            children: [wrote("in1", "/repo/b.ts", 6, 0)] },
        ] }),
    ]);
    expect(summaries.get("l:u0")).toEqual({ stat: { added: 10, removed: 1 } });
  });

  it("says nothing for a turn that changed nothing", () => {
    const summaries = turnChangeSummaries([
      at("l:u0", said("u0")),
      at("l:t0", { kind: "tool", id: "t0", tool: "Read", summary: "read a.ts", input: {} }),
    ]);
    expect(summaries.size).toBe(0);
  });

  it("keeps the file count when the line counts are unknown", () => {
    // A replayed diff whose event carried no before/after: the file was
    // touched, and how many lines moved is not something to invent.
    const summaries = turnChangeSummaries([
      at("l:u0", said("u0")),
      at("l:d0", { kind: "diff", id: "d0", path: "/repo/a.ts", callId: "c" }),
    ]);
    expect(summaries.get("l:u0")).toEqual({ stat: null });
  });

  it("does not count a declined or failed edit as a change", () => {
    // The diff arrives with the call, before it is approved or runs. The call
    // coming back refused means the file was never written.
    const edit = (id: string, result?: { ok: boolean; summary: string }): ConvItem =>
      ({ kind: "tool", id: `c-${id}`, tool: "Edit", summary: "/repo/a.ts", input: {}, result });
    const summaries = turnChangeSummaries([
      at("l:u0", said("u0")),
      at("l:e0", edit("d0", { ok: false, summary: "The user doesn't want to proceed." })),
      at("l:d0", wrote("d0", "/repo/a.ts", 3, 2)),
      at("l:u1", said("u1")),
      at("l:a1", { kind: "subagent", id: "a1", subagentType: "builder",
        description: "build", status: "done", toolCount: 1, subagentCount: 0,
        children: [edit("in0", { ok: false, summary: "old_string not found" }),
          wrote("in0", "/repo/b.ts", 4, 1)] }),
      at("l:u2", said("u2")),
      // Still running: not refused, so it still counts.
      at("l:e2", edit("d2")),
      at("l:d2", wrote("d2", "/repo/c.ts", 5, 0)),
    ]);
    expect(summaries.has("l:u0")).toBe(false);
    expect(summaries.has("l:u1")).toBe(false);
    expect(summaries.get("l:u2")).toEqual({ stat: { added: 5, removed: 0 } });
  });
});

describe("turnChangesLabel", () => {
  it("spells the line counts out for a screen reader", () => {
    expect(turnChangesLabel({ stat: { added: 62, removed: 11 } }))
      .toBe("Review changes from this message: 62 lines added, 11 removed");
  });

  /* ONE FILE COUNT PER ROW, AND IT IS UNDO'S. This chip counted the diff paths
     the window saw; the Undo control beside it counts the files its checkpoint
     holds. Both can be right and disagree, so the chip stopped speaking the
     number rather than the row carrying two of them. */
  it("speaks no file count, because the control beside it owns that number", () => {
    expect(turnChangesLabel({ stat: { added: 62, removed: 11 } }))
      .not.toContain("file");
    expect(turnChangesLabel({ stat: { added: 3, removed: 0 } }))
      .toBe("Review changes from this message: 3 lines added, 0 removed");
  });

  it("claims no line counts it does not have", () => {
    expect(turnChangesLabel({ stat: null }))
      .toBe("Review changes from this message");
    expect(turnChangesLabel({ stat: { added: 0, removed: 0 } }))
      .toBe("Review changes from this message");
  });
});

/* ==========================================================================
   THE PANE ITSELF. `Conversation.tsx` is a module like any other: imported
   from a .test.ts and rendered to static markup, no DOM and no new dependency
   (react-dom is already a runtime dependency of this app). Effects and click
   handlers do not run under renderToStaticMarkup, which is exactly right for
   what is pinned here — what the pane DRAWS for a given set of props, and in
   particular that a verdict comes from the caller's map rather than from state
   the card keeps to itself.
   ========================================================================== */

import { createElement, useState, type ReactNode } from "react";
import { readFileSync } from "node:fs";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, vi } from "vitest";
import { Conversation } from "./components/Conversation";
import { ConversationComposer } from "./components/ConversationComposer";
import { ProviderCommandMenu } from "./components/ProviderCommandMenu";
import { ConversationTranscript } from "./components/ConversationTranscript";
import { findKeysProps } from "./components/ConversationRows";
import { conversationOwnsFindShortcut, turnRestoreConfirmation } from "./conversation-pane-rules";
import { createDraftStore, type DraftStore } from "./draft-store";
import type { ConversationProps } from "./components/Conversation";
import type { HistoryView, TurnRestorePoint } from "./conversation-types";
import {
  DEFAULT_TRANSCRIPT_DENSITY, TRANSCRIPT_DENSITY_KEY, TRANSCRIPT_DENSITY_MAX_SESSIONS,
  isTranscriptDensity, nextTranscriptDensities, parseTranscriptDensities,
  serializeTranscriptDensities, toolRunMinimum, transcriptDensityFor, transcriptDensityOpensItems,
  transcriptHasFoldableRows,
} from "./transcript-density";
import { workGroupParts, workGroupPartsLabel } from "./tool-activity";
import type { ApprovalVerdicts } from "./approval-keys";
import type { TranscriptDensityMap } from "./transcript-density";

const conversationCss = readFileSync(new URL("./components/Conversation.css", import.meta.url), "utf8");

const ASKING: ConvItem =
  { kind: "approval", id: "ap1", tool: "Bash", summary: "rm -rf /tmp/x", input: {} };

// The pane inlines its whole stylesheet in a <style> element, and every class
// name it can ever apply is therefore in the output whether or not anything
// uses it. Assertions are against the markup ONLY.
type PaintOptions = {
  items?: ConvItem[]; approvals?: ApprovalVerdicts; history?: HistoryView;
  sessionKey?: string; activeApprovalIds?: ReadonlySet<string>; focused?: boolean;
  draft?: string; pendingEchoes?: readonly PendingEcho[];
  turnRestorePoints?: ReadonlyMap<string, TurnRestorePoint>;
  onRestoreTurn?: (turnKey: string) => void;
  restoringTurnKey?: string | null;
  pathRoot?: string;
  selectedSubagentKey?: string;
  onSelectSubagent?: (agentKey: string, viaKeyboard: boolean) => void;
  thinking?: boolean;
  workingLabel?: string;
  turnStartedAt?: number;
  onLoadOlderHistory?: () => void;
  config?: ConversationProps["config"];
  context?: ConversationProps["context"];
  previousContext?: ContextUsage;
  commands?: ConversationProps["commands"];
  onRunCommand?: ConversationProps["onRunCommand"];
  queueWhileBusy?: boolean;
  live?: boolean;
  threadLayout?: boolean;
};

function rawPaint(over: PaintOptions = {}): string {
  return renderToStaticMarkup(createElement(Conversation, {
    items: over.items ?? [ASKING],
    thinking: over.thinking ?? false,
    ...(over.workingLabel === undefined ? {} : { workingLabel: over.workingLabel }),
    ...(over.turnStartedAt === undefined ? {} : { turnStartedAt: over.turnStartedAt }),
    ...(over.context === undefined ? {} : { context: over.context }),
    ...(over.previousContext === undefined ? {} : { previousContext: over.previousContext }),
    ...(over.commands === undefined ? {} : { commands: over.commands }),
    ...(over.queueWhileBusy === undefined ? {} : { queueWhileBusy: over.queueWhileBusy }),
    ...(over.threadLayout === undefined ? {} : { threadLayout: over.threadLayout }),
    live: over.live ?? true,
    sessionKey: over.sessionKey ?? "claude#a",
    config: over.config ?? { effort: null, models: [], canSetModel: true, effortLevels: [],
      showEffort: false, canSetEffort: true,
      pendingModel: false, pendingEffort: false, error: null },
    approvals: over.approvals ?? {},
    activeApprovalIds: over.activeApprovalIds ?? new Set(["ap1"]),
    ...(over.draft === undefined ? {} : {
      drafts: createDraftStore({ initial: { [over.sessionKey ?? "claude#a"]: over.draft } }),
    }),
    ...(over.pendingEchoes === undefined ? {} : { pendingEchoes: over.pendingEchoes }),
    ...(over.focused === undefined ? {} : { focused: over.focused }),
    ...(over.history === undefined ? {} : { history: over.history }),
    ...(over.onLoadOlderHistory === undefined
      ? {} : { onLoadOlderHistory: over.onLoadOlderHistory }),
    ...(over.turnRestorePoints === undefined
      ? {} : { turnRestorePoints: over.turnRestorePoints }),
    ...(over.onRestoreTurn === undefined ? {} : { onRestoreTurn: over.onRestoreTurn }),
    ...(over.restoringTurnKey === undefined
      ? {} : { restoringTurnKey: over.restoringTurnKey }),
    ...(over.pathRoot === undefined ? {} : { pathRoot: over.pathRoot }),
    ...(over.selectedSubagentKey === undefined
      ? {} : { selectedSubagentKey: over.selectedSubagentKey }),
    ...(over.onSelectSubagent === undefined
      ? {} : { onSelectSubagent: over.onSelectSubagent }),
    onSend: () => undefined,
    ...(over.onRunCommand === undefined ? {} : { onRunCommand: over.onRunCommand }),
    onStop: () => undefined,
    onSetModel: () => undefined, onSetEffort: () => undefined,
    onApprove: () => undefined, onDeny: () => undefined,
  }));
}

function paint(over: PaintOptions = {}): string {
  return rawPaint(over).replace(/<style[^>]*>[\s\S]*?<\/style>/g, "");
}

/* --------------------------------------------------------------------------
   THE PACED REVEAL, FROM THE MARKUP SIDE. The reveal of a streaming tail is a
   client-side effect and nothing else: renderToStaticMarkup runs no effects, so
   what it paints is the hook's INITIAL state, and that state is the whole
   string. That is the invariant worth pinning — first paint, an export, or any
   render of this tree without a DOM carries the full text, so nothing a reader
   keeps can ever be truncated by pacing meant for a live tail.
   -------------------------------------------------------------------------- */

describe("Conversation — a paced reveal never truncates what is painted", () => {
  // One coarse burst of the shape a provider actually sends: far longer than a
  // single frame's slice would be, so a reveal leaking into this output shows.
  const BURST = "The reveal paces the tail one frame at a time. ".repeat(24).trim();
  const said = (streaming: boolean): ConvItem =>
    ({ kind: "text", id: "t1", text: BURST, streaming });
  const CARET = /<span class="conv-caret"[^>]*>[^<]*<\/span>/;

  it("paints a streaming item in full", () => {
    expect(paint({ items: [said(true)] })).toContain(BURST);
  });

  it("paints a settled item in full", () => {
    expect(paint({ items: [said(false)] })).toContain(BURST);
  });

  it("leaves a settled item's markup identical to a streaming one but for the caret", () => {
    // A completed message is untouched by the hook: same bytes, minus the caret
    // that only a live tail wears.
    const streaming = paint({ items: [said(true)] });
    const settled = paint({ items: [said(false)] });
    expect(streaming).toMatch(CARET);
    expect(settled).not.toMatch(CARET);
    expect(streaming.replace(CARET, "")).toBe(settled);
  });

  it("paints a Threads pane's streaming item whole too, with nothing settling in", () => {
    // Settling in (markdown.tsx) fades only what a PACED step adds, and the
    // hook's first paint is never one: an item that mounts mid-stream, or any
    // render without a DOM, is drawn whole and plain. Its caret is the Threads
    // one, after the last word inside the last paragraph, and only one.
    const thread = paint({ items: [said(true)], threadLayout: true });
    const caret = '<span class="conv-caret" aria-hidden="true">\u2060</span>';
    expect(thread).toContain('class="conv is-thread-layout');
    expect(thread).toContain(BURST);
    expect(thread).not.toContain("conv-md-fresh");
    expect(thread).toContain(`${BURST}${caret}</p></div></div>`);
    expect(thread.split("conv-caret").length).toBe(2);
    const done = paint({ items: [said(false)], threadLayout: true });
    expect(done).not.toContain("conv-caret");
    expect(thread.replace(caret, "")).toBe(done);
  });
});

describe("Conversation — transcript find shortcut ownership", () => {
  it("advertises Cmd/Ctrl+F only for the focused conversation", () => {
    expect(rawPaint({ focused: true })).toContain('aria-keyshortcuts="Meta+F Control+F"');
    expect(rawPaint({ focused: false })).not.toContain("aria-keyshortcuts");
    expect(rawPaint()).not.toContain("aria-keyshortcuts");
  });

  it("keeps the accessible find surface closed until its shortcut is used", () => {
    expect(paint({ focused: true })).not.toContain('role="search"');
    expect(paint({ focused: true })).not.toContain("Find in conversation");
  });

  it("accepts its own or neutral focus but refuses files, sidebars, and modal inputs", () => {
    expect(conversationOwnsFindShortcut(true, true, false, false)).toBe(true);
    expect(conversationOwnsFindShortcut(false, true, false, false)).toBe(false);
    expect(conversationOwnsFindShortcut(false, true, true, false)).toBe(true);
    expect(conversationOwnsFindShortcut(false, false, true, true)).toBe(true);
    expect(conversationOwnsFindShortcut(false, false, false, false)).toBe(false);
    expect(conversationOwnsFindShortcut(true, false, false, false)).toBe(false);
    // A window-targeted synthetic event must not bypass the modal input that
    // really owns focus.
    expect(conversationOwnsFindShortcut(false, false, true, false)).toBe(false);
  });
});

describe("Conversation — composer focus treatment", () => {
  it("does not draw a selection box around the compound prompt surface", () => {
    expect(/\.conv-composer:focus-within\s*\{/u.test(rawPaint())).toBe(false);
  });
});

describe("Conversation — shell-owned switch state", () => {
  it("paints the selected session's controlled draft and local echo", () => {
    const html = paint({
      items: [],
      draft: "finish the parser",
      pendingEchoes: [{
        id: "echo-1", text: "run the focused tests", seen: 0, at: 0, historyKnown: true,
      }],
    });
    expect(html).toContain("finish the parser");
    expect(html).toContain("run the focused tests");
  });
});

describe("Conversation — subagent status cells", () => {
  function subagent(status: "running" | "done" | "failed" | "ended",
      children: ConvItem[] = []): ConvItem {
    return { kind: "subagent", id: `agent-${status}`, subagentType: "reviewer",
      description: "Audit the narrow transcript layout", model: "gpt-5.6-terra",
      status, children, toolCount: 2, subagentCount: 0,
      lastActivity: "checking keyboard focus" };
  }

  /** The subagent <section> only, sliced out of the whole pane. Every count
   *  and every "not on the cell" claim below is about THIS, not about a
   *  stylesheet that names every class the pane can ever apply. */
  function cell(html: string): string {
    const start = html.indexOf('<section class="sub');
    return html.slice(start, html.indexOf("</section>", start) + "</section>".length);
  }

  it.each([
    ["running", "running…", "dot-working dot-pulse"],
    ["done", "done", "codicon-check"],
    ["failed", "failed", "codicon-error"],
    ["ended", "ended", "codicon-debug-stop"],
  ] as const)("renders the %s state as a labelled cell", (status, label, glyph) => {
    const html = paint({ items: [subagent(status)] });
    expect(html).toContain(`class="sub sub-${status}"`);
    expect(html).toContain(`data-subagent-status="${status}"`);
    expect(html).toContain(`>${label}<`);
    expect(html).toContain(glyph);
    expect(html).toContain('role="status"');
    expect(html).toContain('aria-live="polite"');
    expect(html).toContain('aria-atomic="true"');
  });

  it("says the status word only when the word informs, and never loses it", () => {
    // SUCCESS IS QUIET — the rule every tool row already follows. A finished
    // subagent printed a check glyph AND the word "done", two elements for the
    // one outcome that carries no information, on a cell whose whole job is to
    // be scannable. running / failed / ended keep theirs.
    expect(cell(paint({ items: [subagent("done")] })))
      .toContain('<span class="conv-sr">done</span>');
    for (const status of ["running", "failed", "ended"] as const) {
      expect(cell(paint({ items: [subagent(status)] })))
        .not.toContain('class="conv-sr"');
    }
  });

  it("says everything the cell says on ONE row, the model among it", () => {
    // WAS: a bordered two-line card — the row, then a whole second line for a
    // metadata pair. Still one row, but the model is no longer reachable by
    // hover alone: a spawn that named one is running something OTHER than what
    // the parent runs, which is the one thing about it a reader cannot infer.
    const child: ConvItem = { kind: "text", id: "inside", text: "nested result",
      streaming: false };
    const one = cell(paint({ items: [subagent("running", [child])] }));
    expect(one).toContain("reviewer");
    expect(one).toContain("Audit the narrow transcript layout");
    // It joins the slot the counts already share rather than taking one of its
    // own, so the row still ellipsises as a single unit at pane width.
    expect(one).toContain("2 tools · gpt-5.6-terra · last: checking keyboard focus");
    expect(one).not.toContain("sub-meta");
    expect(one).not.toContain("sub-activity");
    // The labelled form still rides the title, where a clipped row keeps it
    // reachable and a screen reader still reads it.
    expect(one).toContain(
      'title="reviewer · Audit the narrow transcript layout · model: gpt-5.6-terra"');
    expect(one).toContain('aria-expanded="false"');
    expect(one).not.toContain("nested result");
    expect(one).not.toContain("sub-kids");
  });

  it("adds nothing at all to a cell whose spawn named no model", () => {
    // The event's contract (src/shared/engine.ts): an absent model means this
    // subagent inherits its parent's, which is the ordinary case and is not
    // worth a word. Such a cell paints exactly what it painted before the
    // field was ever read.
    const inherits = subagent("running") as Extract<ConvItem, { kind: "subagent" }>;
    delete inherits.model;
    const html = cell(paint({ items: [inherits] }));
    expect(html).toContain("2 tools · last: checking keyboard focus");
    expect(html).toContain('title="reviewer · Audit the narrow transcript layout"');
    expect(html).not.toContain("gpt-5.6-terra");
    expect(html).not.toContain("model:");
  });

  it("carries the model on a grid tile's tooltip", () => {
    // The tile face is a fixed square whose description already flexes to fill
    // what the type row and the state line leave it. A fourth line would clamp
    // that description mid-line for a field most tiles do not have, so the tile
    // puts the model where it puts the rest of what the square cannot hold.
    const two = [subagent("running"),
      { ...subagent("done"), id: "agent-2" } as ConvItem];
    const html = paint({ items: two });
    expect(html).toContain('class="sub-tile');
    expect(html).toContain(
      'title="reviewer · Audit the narrow transcript layout · model: gpt-5.6-terra"');
  });

  it("shortens the absolute paths in its activity, as every tool row does", () => {
    // pathRoot was threaded into this pane and then ignored on exactly this
    // string: a finished cell spent its whole slot on a 69-character path.
    const item = subagent("done") as Extract<ConvItem, { kind: "subagent" }>;
    item.lastActivity = "read /repo/src/renderer/theme.css";
    const html = cell(paint({ items: [item], pathRoot: "/repo" }));
    expect(html).toContain("last: read src/renderer/theme.css");
    expect(html).not.toContain("/repo/src/renderer/theme.css");
  });

  it("gives an ended cell the full truthful accessible state", () => {
    const html = paint({ items: [subagent("ended")] });
    expect(html).toContain(
      'aria-label="Ended: the stream stopped before this subagent reported a result"');
  });

  it("draws no summary slot rather than inventing one", () => {
    // WAS: "no activity" on a settled cell and "starting…" on a running one —
    // a whole second line for one word, beside a "running…" that had already
    // said it. Empty means the span is not rendered, exactly as an empty
    // toolRowSummaryText means no .conv-sum. The model is dropped from these
    // fixtures because it is a FACT rather than filler: a cell that knows one
    // has something to say, so it would not be the empty state under test.
    const settled = subagent("done") as Extract<ConvItem, { kind: "subagent" }>;
    settled.toolCount = 0;
    delete settled.lastActivity;
    delete settled.model;
    const done = cell(paint({ items: [settled] }));
    expect(done).not.toContain("no activity");
    expect(done).not.toContain("conv-sum");
    expect(done).not.toContain("sub-kids");

    const fresh = subagent("running") as Extract<ConvItem, { kind: "subagent" }>;
    fresh.toolCount = 0;
    delete fresh.lastActivity;
    delete fresh.model;
    expect(cell(paint({ items: [fresh] }))).not.toContain("starting…");
  });

  it("prints the name once when the description is that name behind a path", () => {
    // THE PHOTOGRAPHED DEFECT. Codex's MultiAgentV2 mapper derives both fields
    // from one agent path, so the cell printed its own name twice and gave the
    // duplicate MORE width than the original. Whole-string equality alone
    // cannot see it: "root interface ui code audit" is not "interface ui code
    // audit", and pathRoot is the repo, so displayPath leaves /root/… alone.
    const codex: ConvItem = { kind: "subagent", id: "a1",
      subagentType: "interface_ui_code_audit",
      description: "/root/interface_ui_code_audit", status: "running",
      children: [], toolCount: 0, subagentCount: 0 };
    const html = cell(paint({
      items: [codex], pathRoot: "/Users/x/Desktop/Projects/Interface" }));
    expect(html).toContain(">interface_ui_code_audit<");
    expect(html).not.toContain("sub-desc");
    // Nothing is destroyed: the raw string stays on the row's title (and, in
    // conversation.ts, in the find corpus).
    expect(html).toContain(
      'title="interface_ui_code_audit · /root/interface_ui_code_audit"');
  });

  it("keeps the whole NAME reachable when the row clips it", () => {
    // .conv-name's rule (Conversation.tsx:585) is that a name may ellipsise
    // BECAUSE the row's title keeps all of it reachable. The subagent row built
    // its title from description and model only, so two spawns of one task —
    // the everyday fan-out — hovered identically and the only field that told
    // them apart was the clipped one.
    const spawn = (type: string): ConvItem => ({ kind: "subagent", id: type,
      subagentType: type, description: "Audit the renderer UI", status: "running",
      children: [], toolCount: 0, subagentCount: 0 });
    const html = paint({ items: [spawn("renderer_cells_builder"),
      spawn("renderer_panel_builder")] });
    expect(html).toContain('title="renderer_cells_builder · Audit the renderer UI"');
    expect(html).toContain('title="renderer_panel_builder · Audit the renderer UI"');
    // And a cell whose description was suppressed has a title at all, rather
    // than title="" over a name it just clipped.
    const bare: ConvItem = { kind: "subagent", id: "a1", subagentType: "reviewer",
      description: "", status: "running", children: [], toolCount: 0,
      subagentCount: 0 };
    expect(cell(paint({ items: [bare] }))).toContain('title="reviewer"');
  });

  it("keeps a description that says something the type did not", () => {
    const claude: ConvItem = { kind: "subagent", id: "a1",
      subagentType: "general-purpose",
      description: "Review Task 1 (spec + quality)", status: "running",
      children: [], toolCount: 0, subagentCount: 0 };
    const html = cell(paint({ items: [claude] }));
    expect(html).toContain('<span class="sub-desc">Review Task 1 (spec + quality)</span>');
  });
});

describe("Conversation — the cell hands its subagent to a detail surface", () => {
  const spawn = (children: ConvItem[] = []): ConvItem => ({
    kind: "subagent", id: "agent-1", subagentType: "reviewer",
    description: "Audit the layout", status: "running", children,
    toolCount: 0, subagentCount: 0,
  });

  function cell(html: string): string {
    const start = html.indexOf('<section class="sub');
    return html.slice(start, html.indexOf("</section>", start) + "</section>".length);
  }

  it("is reachable when there is nothing under it — that is the one worth opening", () => {
    // A subagent between its spawn and its first event has no nested transcript
    // to unfold, which is exactly when a user wants to know what it was asked
    // to do. The whole row used to be `disabled` in that state.
    const html = cell(paint({ items: [spawn()], onSelectSubagent: () => undefined }));
    expect(html).toContain('class="sub-row"');
    expect(html).not.toContain("disabled");
    expect(html).toContain("data-agent-key=\"agent-1\"");
  });

  it("paints the selected cell, and only the one the caller named", () => {
    const two = [spawn(), { ...spawn(), id: "agent-2" } as ConvItem];
    const html = paint({ items: two, selectedSubagentKey: "agent-2",
      onSelectSubagent: () => undefined });
    expect(html).toMatch(/class="sub-tile"(?![^>]*is-selected)[^>]*data-agent-key="agent-1"/);
    expect(html).toMatch(/class="sub-tile is-selected"[^>]*data-agent-key="agent-2"/);
    expect(html).not.toMatch(/class="sub-tile is-selected"[^>]*data-agent-key="agent-1"/);
    expect(html).toContain('aria-current="true"');
  });

  it("ignores a selection it was given no handler for", () => {
    // Half-wired is the failure this project keeps hitting. A key with no
    // callback means some caller passed one prop and not the other; painting a
    // cell selected for a panel that cannot exist is a lie about the app state.
    const html = cell(paint({ items: [spawn()], selectedSubagentKey: "agent-1" }));
    expect(html).not.toContain("is-selected");
    expect(html).not.toContain("aria-current");
  });

  it("opens the ancestor path when the detail panel selects a nested subagent", () => {
    const child: ConvItem = { kind: "subagent", id: "agent-child",
      subagentType: "researcher", description: "Trace the state", status: "running",
      children: [], toolCount: 0, subagentCount: 0 };
    const html = paint({ items: [spawn([child])], selectedSubagentKey: "agent-child",
      onSelectSubagent: () => undefined });
    expect(html).toContain('class="sub sub-running sub-open"');
    expect(html).toContain('aria-label="Hide nested activity of reviewer"');
    expect(html).toContain('data-agent-key="agent-child"');
    expect(html).toContain('class="sub sub-running is-selected"');
  });

  it("keeps the fold as its own control, and only where there is a fold", () => {
    const withKids = cell(paint({
      items: [spawn([{ kind: "text", id: "t", text: "x", streaming: false }])],
      onSelectSubagent: () => undefined }));
    expect(withKids).toContain('class="sub-fold"');
    expect(withKids).toContain('aria-label="Show nested activity of reviewer"');
    // Two SIBLING buttons. A button nested inside a button is invalid HTML and
    // the inner one is unreachable by keyboard.
    expect(withKids.indexOf('class="sub-fold"'))
      .toBeLessThan(withKids.indexOf('class="sub-row"'));
    expect(withKids.slice(withKids.indexOf('class="sub-fold"'),
      withKids.indexOf('class="sub-row"'))).toContain("</button>");
    // Absent, not disabled: an empty subagent adds no dead tab stop, and the
    // column it would have held is padding instead.
    const flat = cell(paint({ items: [spawn()], onSelectSubagent: () => undefined }));
    expect(flat).not.toContain("sub-fold");
    expect(flat).toContain("sub-head sub-head-flat");
    expect(flat).not.toContain("aria-expanded");
  });

  it("falls back to unfolding in place when no surface is wired", () => {
    // The degrade path: a build where App has not passed the callback yet must
    // behave the way this cell always did, not become an inert row.
    const html = cell(paint({
      items: [spawn([{ kind: "text", id: "t", text: "x", streaming: false }])] }));
    expect(html).toContain('class="sub-row"');
    expect(html).not.toContain("aria-current");
    expect(html).toContain('class="sub-fold"');
  });
});

describe("Conversation — the approval verdict is the caller's, not the card's", () => {
  it("offers Allow and Deny for a request nothing has answered", () => {
    const html = paint({ approvals: {} });
    expect(html).toContain("conv-approval-pending");
    expect(html).toContain(">Allow<");
    expect(html).toContain(">Deny<");
    expect(html).not.toContain("Allow sent");
  });

  it("shows a verdict sent from the OTHER surface, and takes the buttons away", () => {
    // The defect this closes: allow on one surface, open the session, and the
    // card offered Allow/Deny for a request the engine had already run — the
    // second verdict is dropped by the bridge with a console warning, and the
    // pane printed it as the outcome.
    const html = paint({
      approvals: { [approvalVerdictKey("claude#a", "ap1")]: { verdict: "allow" } },
    });
    expect(html).toContain("Allow sent");
    expect(html).not.toContain("conv-approval-pending");
    expect(html).not.toContain(">Deny<");
  });

  it("says SENT, not settled — the engine has not taken the request back", () => {
    const html = paint({
      approvals: { [approvalVerdictKey("claude#a", "ap1")]: { verdict: "deny" } },
    });
    expect(html).toContain("Deny sent");
    expect(html).not.toContain(">Denied<");
  });

  it("ignores a verdict recorded for the same approvalId in another session", () => {
    const html = paint({
      approvals: { [approvalVerdictKey("claude#b", "ap1")]: { verdict: "allow" } },
    });
    expect(html).toContain("conv-approval-pending");
    expect(html).not.toContain("Allow sent");
  });

  it("puts the controls back and says why when the verdict did not land", () => {
    const html = paint({
      approvals: {
        [approvalVerdictKey("claude#a", "ap1")]:
          { verdict: "allow", error: "Session is not running — resume it first" },
      },
    });
    expect(html).toContain("Session is not running — resume it first");
    expect(html).toContain('role="alert"');
    expect(html).toContain("conv-approval-pending");
    expect(html).toContain(">Allow<");
    expect(html).not.toContain("Allow sent");
  });

  it("renders a historical approval as closed instead of offering stale controls", () => {
    const html = paint({ approvals: {}, activeApprovalIds: new Set() });
    expect(html).toContain("Request closed");
    expect(html).not.toContain(">Allow<");
    expect(html).not.toContain(">Deny<");
  });
});

describe("Conversation — what the pane says about a shortened transcript", () => {
  it("prints the engine's own count when the caller passes it", () => {
    const html = paint({
      items: [], history: { status: "ready", items: [], olderMessages: 853, truncated: true },
    });
    expect(html).toContain("853 older messages are not shown.");
  });

  it("falls back to the bare disclosure when only the boolean is known", () => {
    const html = paint({ items: [], history: { status: "ready", items: [], truncated: true } });
    expect(html).toContain("Older messages are not shown.");
  });

  it("offers the caller-owned next page while older messages remain", () => {
    const html = paint({
      items: [],
      history: { status: "ready", items: [], olderMessages: 853, canLoadOlder: true },
      onLoadOlderHistory: () => undefined,
    });
    expect(html).toContain('<button type="button" class="btn">Load older</button>');
  });

  it("keeps the current page visible and disables the control while paging", () => {
    const html = paint({
      items: [],
      history: {
        status: "ready",
        items: [{ kind: "text", id: "h1", text: "already loaded", streaming: false }],
        olderMessages: 853,
        loadingOlder: true,
        canLoadOlder: true,
      },
      onLoadOlderHistory: () => undefined,
    });
    expect(html).toContain("already loaded");
    expect(html).toContain("Loading earlier messages…");
    expect(html).toContain("disabled=\"\"");
    expect(html).toContain(">Loading…<");
  });

  it("reports a page failure without discarding the ready transcript", () => {
    const html = paint({
      items: [],
      history: {
        status: "ready",
        items: [{ kind: "text", id: "h1", text: "still here", streaming: false }],
        olderMessages: 853,
        loadOlderError: "The older page could not be read.",
        canLoadOlder: true,
      },
      onLoadOlderHistory: () => undefined,
    });
    expect(html).toContain("still here");
    expect(html).toContain("The older page could not be read.");
    expect(html).toContain('role="alert"');
    expect(html).toContain(">Load older<");
  });

  it("keeps the truncation disclosure but removes a dead control at the hard ceiling", () => {
    const html = paint({
      items: [],
      history: { status: "ready", items: [], olderMessages: 12, canLoadOlder: false },
      onLoadOlderHistory: () => undefined,
    });
    expect(html).toContain("12 older messages are not shown.");
    expect(html).not.toContain(">Load older<");
  });

  it("keeps the existing prepend anchor when a strict-superset page adds rows", () => {
    const source = readFileSync(new URL("./components/ConversationTranscript.tsx", import.meta.url), "utf8");
    const anchor = source.slice(
      source.indexOf("const height = el.scrollHeight"),
      source.indexOf("// Search navigation deliberately breaks"),
    );
    expect(anchor).toContain("past.length !== lastPastCount.current && grew > 0");
    expect(anchor).toContain("el.scrollTop += grew;");
    expect(anchor).toContain("lastPastCount.current = past.length;");
  });
});

describe("Conversation — the live composer is one capsule", () => {
  it("keeps attach-model-effort-access-send in the same 720px sheet", () => {
    const html = paint();
    const css = conversationCss;
    const sheet = html.indexOf('class="conv-composer"');
    const controls = html.indexOf('class="conv-controls"');
    const pills = html.indexOf('class="conv-pills"');
    const send = html.indexOf("conv-send");
    expect(sheet).toBeGreaterThanOrEqual(0);
    expect(controls).toBeGreaterThan(sheet);
    expect(pills).toBeGreaterThan(controls);
    expect(send).toBeGreaterThan(pills);
    expect(html).not.toContain("conv-toolbar");
    expect(html.split('class="conv-controls"')).toHaveLength(2);
    expect(html).toContain("Message this session…   (Enter to send)");
    expect(html).toContain('class="conv-ta-row"');
    expect(html).toContain('class="conv-prompt-mark" aria-hidden="true">❯<');
    expect(css).toMatch(/\.conv-composer \{[^}]*var\(--conv-measure\)/u);
    expect(css).toContain("border-radius:50%");
    // The 12% meter is on the left of the control row. Anchoring the panel to
    // the right grew it into the sidebar; the tile clips overflow.
    expect(css).toMatch(/\.conv-context-meter-panel \{[^}]*left:0/u);
    expect(css).not.toMatch(/\.conv-context-meter-panel \{[^}]*right:0/u);
  });
});

describe("Conversation — a locked composer says so once", () => {
  const REASON = "Choose a model before sending…";
  const empty = { items: [] as ConvItem[] };

  it("locks only a brand-new empty session that still has no model", () => {
    const html = paint(empty);
    expect(html).toContain("conv-lock-reason");
    // Twice: the banner the textarea's aria-describedby points at, and the send
    // button's tooltip. The third copy was the placeholder, which stacked the
    // identical sentence 40px under the banner.
    expect(html.split(REASON)).toHaveLength(3);
    expect(html).not.toContain(`placeholder="${REASON}"`);
  });

  it("gives the empty field a neutral placeholder that promises no Enter", () => {
    expect(paint(empty)).toContain('placeholder="Message this session…"');
    expect(paint(empty)).not.toContain("(Enter to send)");
  });

  it("still describes the disabled textarea by the banner", () => {
    const html = paint(empty);
    const described = /aria-describedby="([^"]+)"/u.exec(html);
    expect(described).not.toBeNull();
    expect(html).toContain(`id="${described![1]}" class="conv-lock-reason"`);
  });

  it("does not lock a resumed session that already has transcript", () => {
    const html = paint();
    expect(html).not.toContain("conv-lock-reason");
    expect(html).not.toContain(REASON);
    expect(html).toContain("Message this session…   (Enter to send)");
  });

  it("treats ready history items as transcript even when live items are empty", () => {
    const html = paint({
      items: [],
      history: {
        status: "ready",
        items: [{ kind: "text", id: "h1", text: "prior", streaming: false }],
      },
    });
    expect(html).not.toContain("conv-lock-reason");
    expect(html).not.toContain(REASON);
  });

  it("treats local echoes as transcript", () => {
    const html = paint({
      items: [],
      pendingEchoes: [{
        id: "e1", text: "just sent", seen: 0, at: 0, historyKnown: true,
      }],
    });
    expect(html).not.toContain("conv-lock-reason");
    expect(html).not.toContain(REASON);
  });

  it("keeps a running model visible even when it is not in the catalogue", () => {
    const html = paint({
      items: [ASKING],
      config: {
        engine: "claude", engineLabel: "Claude",
        model: "claude-opus-4-6", effort: "high",
        models: [], canSetModel: true, effortLevels: ["low", "medium", "high"],
        showEffort: true, canSetEffort: true,
        pendingModel: false, pendingEffort: false, error: null,
      },
    });
    expect(html).toContain("claude-opus-4-6");
    expect(html).not.toContain(">Choose model<");
    expect(html).toContain("aria-label=\"Effort: High\"");
    expect(html).toContain('class="conv-pill-sep"');
    expect(html).toContain("·");
    expect(html).not.toContain("conv-session-line");
    expect(html).not.toContain("⌃⇥ agents");
  });

  it("keeps refined chrome on ordinary pills, not the CLI session footer", () => {
    const html = paint({
      items: [ASKING],
      context: { used: 0, size: 1_000_000, percent: 0 },
      config: {
        engine: "claude", engineLabel: "Claude",
        model: "claude-fable-5", effort: "xhigh",
        models: [{ value: "claude-fable-5", displayName: "Fable 5",
          supportsEffort: true, effortLevels: ["low", "xhigh"] }],
        canSetModel: true, effortLevels: ["low", "xhigh"],
        showEffort: true, canSetEffort: true,
        canSetPermission: true, permissionMode: "auto",
        permissionModes: ["default", "auto", "plan"],
        pendingModel: false, pendingEffort: false, error: null,
      },
    });
    expect(html).toContain("Fable 5");
    expect(html).toContain("aria-label=\"Effort: Extra high\"");
    expect(html).toContain("conv-iconbtn conv-context-meter-btn");
    expect(html).not.toContain("conv-session-star");
    expect(html).not.toContain("auto mode on");
    expect(html).not.toContain("⌃⇥ agents");
    expect(html).not.toContain("0 / 1.0M tokens · 0%");
  });

  it("prints the Claude Code footer under the prompt only in CLI chrome", () => {
    const previous = (globalThis as { document?: unknown }).document;
    Object.defineProperty(globalThis, "document", {
      configurable: true,
      value: { documentElement: { dataset: { chrome: "cli" } } },
    });
    try {
      const config = {
        engine: "claude", engineLabel: "Claude",
        model: "claude-fable-5", effort: "xhigh",
        models: [{ value: "claude-fable-5", displayName: "Fable 5",
          supportsEffort: true, effortLevels: ["low", "xhigh"] }],
        canSetModel: true, effortLevels: ["low", "xhigh"],
        showEffort: true, canSetEffort: true,
        canSetPermission: true, permissionMode: "auto",
        permissionModes: ["default", "auto", "plan"],
        pendingModel: false, pendingEffort: false, error: null,
      };
      const html = paint({
        items: [ASKING],
        context: { used: 0, size: 1_000_000, percent: 0 },
        config,
      });
      expect(html).toContain("conv-session-star");
      expect(html).toContain("Fable 5");
      // The meter drops the noun on screen and keeps it in the accessible name
      // the pills carry, which is the one place a sentence is wanted.
      expect(html).toContain(">0 / 1.0M · 0%<");
      expect(html).toContain("0 / 1.0M tokens · 0%");
      // The access mode is a pane-header control now, and this row's name
      // stopped saying it a second time.
      expect(html).not.toContain("auto mode on");
      expect(html).toContain("aria-label=\"Effort: Extra high\"");
      expect(html).toContain("⌃⇥ agents · / commands");
      expect(html).toContain('class="conv-session-line conv-session-hints"');
      expect(html.split("conv-session-line")).toHaveLength(3);

      // WITH NO PERCENT REPORTED there is no chip label to announce, and the
      // fallback has to be the sentence, not the meter's bare figures: the
      // visible run may drop the noun, an accessible name may not.
      const noPercent = paint({
        items: [ASKING],
        context: { used: 12_000, size: 200_000, percent: null },
        config,
      });
      expect(noPercent).toContain(">12k / 200k<");
      expect(noPercent).toContain('aria-label="Session context 12k / 200k tokens"');
      expect(noPercent).toContain('title="12k / 200k tokens"');
    } finally {
      if (previous === undefined) {
        delete (globalThis as { document?: unknown }).document;
      } else {
        Object.defineProperty(globalThis, "document", {
          configurable: true, value: previous,
        });
      }
    }
  });

  it("does not lock a resume while history is still loading or failed", () => {
    expect(paint({ items: [], history: { status: "loading" } }))
      .not.toContain("conv-lock-reason");
    expect(paint({ items: [], history: { status: "error", message: "could not read" } }))
      .not.toContain(REASON);
    expect(paint({ items: [], history: { status: "unavailable", message: "no store" } }))
      .not.toContain(REASON);
  });
});

describe("Conversation — the cold open and the older-message fetch are two waits", () => {
  it("opens a session over the shape of the transcript it is reading", () => {
    const html = paint({ items: [], history: { status: "loading" } });
    expect(html).toContain("Loading conversation…");
    expect(html).toContain("conv-skeleton");
    expect(html).toContain("conv-skel-row");
    // Nothing is on screen yet, so there is nothing for messages to be earlier
    // than — and a lone spinner over a blank pane reads as stalled.
    expect(html).not.toContain("Loading earlier messages…");
    expect(html).not.toContain("codicon-modifier-spin");
  });

  it("keeps the paged wording once the pane actually holds something", () => {
    const html = paint({
      items: [{ kind: "text", id: "t1", text: "already here", streaming: false }],
      history: { status: "loading" },
    });
    expect(html).toContain("Loading earlier messages…");
    expect(html).not.toContain("Loading conversation…");
    expect(html).not.toContain("conv-skeleton");
  });

  it("leaves the skeleton out of the accessibility tree it duplicates", () => {
    const html = paint({ items: [], history: { status: "loading" } });
    expect(html).toContain('class="conv-skeleton" aria-hidden="true"');
  });
});

describe("Conversation — a produced diff is a row, not an orphaned box", () => {
  const DIFF: ConvItem = { kind: "diff", id: "d1", callId: "c-d1", path: "/tmp/iface/electron-builder.yml" };

  it("carries the same columns as the tool rows it stands between", () => {
    const html = paint({ items: [DIFF] });
    expect(html).toContain("conv-diffrow");
    expect(html).toContain("/tmp/iface/electron-builder.yml");
    // The same leading glyph column its siblings have. It used to hold a 22px
    // spacer in front of that glyph, which was right while a diff row was
    // always a CHILD of the tool row above it and wrong the moment a file
    // change became one row: it indented a top-level row against nothing.
    // The trailing state slot is NOT drawn: this row has no state to report and
    // nothing to open, so the slot held only a chevron that promised a click.
    expect(html).not.toContain('class="conv-state conv-inline"');
    expect(html).not.toContain("conv-diff-lead");
    expect(html).toContain('<div class="conv-row conv-diffrow" '
      + 'title="/tmp/iface/electron-builder.yml">'
      + '<span class="codicon codicon-git-compare conv-kind"');
  });

  it("wears no outline, because four stacked boxes are not emphasis", () => {
    const css = conversationCss;
    // The one boxed row in a pane that is otherwise a flat column of rows. A
    // file edit produces four or five of these at a time, so the outline was
    // multiplied by exactly the case it was meant to distinguish.
    const rule = /\.conv-diffrow \{([^}]*)\}/u.exec(css);
    expect(rule?.[1] ?? "").not.toContain("border");
    // .conv-approval is a blocking decision and stays the only outlined thing.
    expect(css).toContain(".conv-approval { ");
  });

  it("drops the two words that never varied and states the path alone", () => {
    const html = paint({ items: [DIFF] });
    // "Diff" repeated the git-compare glyph beside it; "Open" was identical on
    // every diff row ever rendered — and the row opens nothing now, so neither
    // word comes back as an accessible name either. The path is the row.
    expect(html).not.toContain(">Diff<");
    expect(html).not.toContain(">Open");
    expect(html).not.toContain("Open diff");
    expect(html).toContain(">/tmp/iface/electron-builder.yml<");
    expect(html).toContain("codicon-git-compare");
    // No chevron of either kind. A right chevron is what a COLLAPSED fold looks
    // like on the rows around this one (.conv-chev closes at rotate(-90deg)),
    // so on a row that opens nothing it advertised a click that cannot happen.
    expect(html).not.toContain("codicon-chevron-right");
    expect(html).not.toContain("conv-chev");
  });

  it("prints the path relative to the session root and keeps the absolute one", () => {
    const html = paint({
      pathRoot: "/tmp/iface",
      items: [DIFF],
    });
    expect(html).toContain(">electron-builder.yml<");
    expect(html).toContain('<div class="conv-row conv-diffrow" '
      + 'title="/tmp/iface/electron-builder.yml">');
  });

  it("renders exactly as it always did when the caller supplies no root", () => {
    const html = paint({ items: [DIFF] });
    expect(html).toContain(">/tmp/iface/electron-builder.yml<");
  });

  it("folds consecutive diffs into one Edited N files control", () => {
    const html = paint({
      items: [
        { kind: "diff", id: "d1", callId: "c-d1", path: "/tmp/a.ts" },
        { kind: "diff", id: "d2", callId: "c-d2", path: "/tmp/b.ts" },
        { kind: "diff", id: "d3", callId: "c-d3", path: "/tmp/c.ts", added: 12, removed: 4 },
      ],
    });
    expect(html).toContain("Edited 3 files");
    expect(html).not.toContain(">/tmp/a.ts<");
    expect(html).not.toContain(">/tmp/b.ts<");
    expect(html).not.toContain(">/tmp/c.ts<");
    expect(html).not.toContain("+0 −0");
  });

  it("prints the line counts the window recorded and never +0 −0", () => {
    const counted = paint({
      items: [
        { kind: "diff", id: "d1", callId: "c-d1", path: "/tmp/a.ts", added: 12, removed: 4 },
        { kind: "diff", id: "d2", callId: "c-d2", path: "/tmp/b.ts", added: 0, removed: 0 },
      ],
    });
    expect(counted).toContain("Edited 2 files +12 −4");
    expect(counted).not.toContain("+0 −0");
    const unknown = paint({
      items: [
        { kind: "diff", id: "d1", callId: "c-d1", path: "/tmp/a.ts" },
        { kind: "diff", id: "d2", callId: "c-d2", path: "/tmp/b.ts" },
      ],
    });
    expect(unknown).toContain("Edited 2 files");
    expect(unknown).not.toContain("+");
  });

  it("paints the fold's two counts in the diff vocabulary, not in body grey", () => {
    // The label is still ONE string for the accessible name — that sentence is
    // what a screen reader reads and it did not change — while the eye gets the
    // added and removed halves in the colours every diff tool uses. Asserting
    // the CLASSES is the point: a toContain over the visible text stays green
    // on the single grey string this replaced, and so does the aria-label.
    const html = paint({
      items: [
        { kind: "diff", id: "d1", callId: "c-d1", path: "/tmp/a.ts", added: 8, removed: 3 },
        { kind: "diff", id: "d2", callId: "c-d2", path: "/tmp/b.ts", added: 4, removed: 1 },
      ],
    });
    expect(html).toContain('<span class="conv-stat-add">+12</span>');
    expect(html).toContain('<span class="conv-stat-del">−4</span>');
    expect(html).toContain('aria-label="Edited 2 files +12 −4"');
  });

  it("counts distinct files rather than the times each was redrawn", () => {
    // A patch the agent keeps refining re-emits the same path, so the fold was
    // counting EVENTS: three rows over two files read "Edited 3 files". The
    // composer's dirty pill has always de-duped; this is the same rule.
    const html = paint({ items: [
      { kind: "diff", id: "d1", callId: "c-d1", path: "/tmp/a.ts", added: 1, removed: 0 },
      { kind: "diff", id: "d2", callId: "c-d2", path: "/tmp/a.ts", added: 2, removed: 1 },
      { kind: "diff", id: "d3", callId: "c-d3", path: "/tmp/b.ts", added: 3, removed: 0 },
    ] });
    expect(html).toContain("Edited 2 files +6 −1");
    expect(html).not.toContain("Edited 3 files");
  });

  it("keeps the folded control findable under every row it stands for", () => {
    // This fold navigates and never reveals its members, so a find hit on one
    // of them has nothing of its own to scroll to. Its sibling fold was deleted
    // for exactly this; the one that survives carries the keys instead.
    const html = paint({ items: [
      { kind: "diff", id: "d1", callId: "c-x", path: "/tmp/a.ts", added: 1, removed: 0 },
      { kind: "diff", id: "d2", callId: "c-x", path: "/tmp/b.ts", added: 2, removed: 0 },
    ] });
    expect(html).toContain("Edited 2 files");
    expect(html).toContain('data-find-keys="l:d1 l:d2"');
  });

  it("keeps a single diff as the path pill", () => {
    const html = paint({ items: [DIFF] });
    expect(html).not.toContain("Edited 1 file");
    expect(html).toContain(">/tmp/iface/electron-builder.yml<");
  });
});

/* --------------------------------------------------------------------------
   ONE FILE CHANGE, ONE ROW. Every engine drew each edit twice — a tool row
   carrying the verb and the outcome, a diff row carrying the counts — so a
   five-edit stretch was ten rows saying five things. These
   pin the merged row, and in particular that the merge never buys a shorter
   transcript with a row that lies: a delete still says "Delete", a refused
   patch still says "declined", and EVERY state still prints its counts.
   -------------------------------------------------------------------------- */
describe("Conversation — a file change is one row, not the same fact twice", () => {
  const P = "/tmp/p";
  const patch = (over: Partial<Extract<ConvItem, { kind: "tool" }>> = {},
    id = "fc1"): ConvItem => ({
    kind: "tool", id, tool: "fileChange", summary: `update ${P}/a.ts`,
    input: { changes: [{ path: `${P}/a.ts` }] },
    result: { ok: true, summary: `update ${P}/a.ts` }, ...over,
  });
  // callId defaults to the patch helper's own id: a diff belongs to the CALL
  // that produced it, and the merge pairs on that and not on what came next.
  const edit = (id: string, path: string, added: number, removed: number,
    callId = "fc1"): ConvItem => ({ kind: "diff", id, path, callId, added, removed });
  const rows = (html: string): number => (html.match(/class="conv-row/gu) ?? []).length;
  // The TRANSCRIPT only. The composer's dirty pill carries the same line counts
  // for the whole session, and a bare toContain over the pane cannot tell the
  // two apart — which is how "a failed row prints no stat" passes while the row
  // prints one.
  const draw = (over: PaintOptions): string => {
    const html = paint(over);
    return html.slice(html.indexOf('<div class="conv-list"'),
      html.indexOf('<div class="conv-sr" role="status"'));
  };
  // A row located BY ITS OWN NAME, never "somewhere in the page". An unscoped
  // toContain over a pane holding three rows that all wear a class stays green
  // while the one row the rule exists for loses it — which is how a CSS rule
  // went dead here with the suite fully green.
  const rowByAria = (html: string, pattern: RegExp): string => {
    for (const tag of html.match(/<div[^>]*>/gu) ?? []) {
      if (pattern.test(/aria-label="([^"]*)"/u.exec(tag)?.[1] ?? "")) return tag;
    }
    throw new Error(`no row whose accessible name matches ${String(pattern)}`);
  };
  const classOf = (tag: string): string => /class="([^"]*)"/u.exec(tag)?.[1] ?? "";

  it("prints the verb, the path and the counts — once each", () => {
    const html = draw({ pathRoot: P,
      items: [patch(), edit("d1", `${P}/a.ts`, 14, 23)] });
    expect(rows(html)).toBe(1);
    expect(html.match(/>Updated</gu)).toHaveLength(1);
    expect(html.match(/>a\.ts</gu)).toHaveLength(1);
    expect(html).toContain("+14");
    expect(html).toContain("−23");
    // A settled change with no body ends in nothing: the right chevron it used
    // to end in is the collapsed-fold glyph, and this row opens nothing. It
    // also no longer wears the spacer that made the second copy read as an
    // indented child of the first.
    expect(html).not.toContain("codicon-chevron-right");
    expect(html).not.toContain("conv-diff-lead");
    expect(html).not.toContain(">File change<");
  });

  it("paints the two halves of the diffstat in the diff vocabulary", () => {
    // The counts used to print as one --fg-muted string, so the row's only
    // quantitative fact wore the same grey as the words beside it. The CLASSES
    // are what is asserted, because a toContain over the visible digits stays
    // green on that one grey span.
    const html = draw({ pathRoot: P,
      items: [patch(), edit("d1", `${P}/a.ts`, 14, 23)] });
    expect(html).toContain('<span class="conv-stat-add">+14</span>');
    expect(html).toContain('<span class="conv-stat-del">−23</span>');
    // The accessible name is unchanged and still spells the same fact in words.
    expect(html).toContain("14 lines added, 23 removed");
    // Silence stays formatDiffStat's rule and must survive the split: a
    // net-zero patch prints neither span rather than a green "+0".
    const zero = draw({ pathRoot: P,
      items: [patch(), edit("d1", `${P}/a.ts`, 0, 0)] });
    expect(zero).not.toContain("conv-stat-add");
    expect(zero).not.toContain("conv-stat-del");
  });

  it("keeps every path on the row while printing the short one", () => {
    const html = draw({ pathRoot: P,
      items: [patch(), edit("d1", `${P}/a.ts`, 14, 23)] });
    expect(html).toContain(`title="update ${P}/a.ts`);
    expect(html).toContain(
      'aria-label="Updated, a.ts, 14 lines added, 23 removed, tool fileChange, '
      + 'completed"');
  });

  it("builds the title from the diff rows, not the summary the engine cut", () => {
    // The engine's changeList is sliced at 120 chars with no ellipsis, so a
    // three-file patch's third filename existed NOWHERE on the row: not in the
    // text, not in the title, not in the accessible name. The cut is the real
    // one, not a shortened stand-in.
    const dir = `${P}/src/renderer/components`;
    const paths = [`${dir}/AlphaPanel.tsx`, `${dir}/BetaPanel.tsx`,
      `${dir}/GammaPanel.tsx`];
    const summary = paths.map(x => `delete ${x}`).join(", ").slice(0, 120);
    expect(summary).not.toContain("GammaPanel.tsx");    // the cut really cuts
    const html = draw({ pathRoot: P, items: [
      patch({ summary, result: { ok: true, summary: "3 files" } }),
      ...paths.map((path, i) => edit(`d${String(i)}`, path, 0, 2)),
    ] });
    for (const path of paths) expect(html).toContain(`delete ${path}`);
    // And the kind survives the cut too, so an all-delete patch does not go
    // neutral just because the list ran out of characters.
    expect(html).toContain(">Deleted<");
    expect(html).toContain(">3 files<");
  });

  it("says DELETE for a delete, because the diff row never could", () => {
    // The kind lives only in the tool row's summary. Dropping that row instead
    // of merging it would have made a deleted file indistinguishable from an
    // edited one.
    const html = draw({ pathRoot: P,
      items: [patch({ summary: `delete ${P}/a.ts`,
        result: { ok: true, summary: `delete ${P}/a.ts` } }),
        edit("d1", `${P}/a.ts`, 0, 11)] });
    expect(html).toContain(">Deleted<");
    expect(html).not.toContain(">Updated<");
  });

  it("never guesses a verb it cannot read — an overwrite is Changed, not Added", () => {
    // Claude's Write mapper hard-codes before:"" for an overwrite, so "" on one
    // side proves nothing. This is the assertion that kills a helper which
    // infers add/delete from an empty side.
    const html = draw({ pathRoot: P, items: [
      { kind: "tool", id: "w1", tool: "Write", summary: `${P}/new.ts`, input: {},
        result: { ok: true, summary: "ok" } },
      edit("d1", `${P}/new.ts`, 9, 0, "w1"),
    ] });
    expect(html).toContain(">Changed<");
    expect(html).not.toContain(">Added<");
  });

  it("will not call an ACP edit an update — that kind also creates the file", () => {
    // Grok names the tool from the ACP kind, and "edit" there covers creation.
    const html = draw({ pathRoot: P, items: [
      { kind: "tool", id: "g1", tool: "edit", summary: "Edit file", input: {},
        result: { ok: true, summary: "ok" } },
      edit("d1", `${P}/grok-made.ts`, 2, 0, "g1"),
    ] });
    expect(html).toContain(">Changed<");
    expect(html).not.toContain(">Updated<");
  });

  it("sizes a FAILED patch, and says what went wrong", () => {
    // A patch that failed is exactly the one a reader needs measured, so the
    // counts are printed on it like on any other state — nothing here waits
    // for the patch to settle before it will say how big it was.
    const html = draw({ pathRoot: P,
      items: [patch({ summary: `delete ${P}/a.ts`,
        result: { ok: false, summary: "failed" } }), edit("d1", `${P}/a.ts`, 14, 23)] });
    const row = rowByAria(html, /^Delete, a\.ts,/u);
    expect(classOf(row)).toBe("conv-row conv-diffrow conv-filerow");
    expect(row).toContain('aria-label="Delete, a.ts, 14 lines added, 23 removed, '
      + 'tool fileChange, failed"');
    expect(html).toContain("conv-state-fail");
    expect(html).toContain("conv-glyph-fail");
    expect(html).toContain("failed</span>");
    expect(html).toContain("+14");
    expect(html).toContain("−23");
    // The reason lives behind the fold, which is the row's one control and a
    // SIBLING of the row rather than a child of it.
    expect(html).toContain('class="conv-filefold"');
    expect(html).toContain("conv-chev");
  });

  it("says DECLINED when the patch was refused rather than broken", () => {
    const html = draw({ pathRoot: P,
      items: [patch({ result: { ok: false, summary: "declined" } }),
        edit("d1", `${P}/a.ts`, 14, 23)] });
    expect(classOf(rowByAria(html, /declined$/u)))
      .toBe("conv-row conv-diffrow conv-filerow");
    expect(html).toContain('aria-label="Update, a.ts, 14 lines added, 23 removed, '
      + 'tool fileChange, declined"');
    expect(html).toContain("declined</span>");
    expect(html).not.toContain("failed</span>");
  });

  it("never puts a patch that did not happen in the past tense", () => {
    // "Deleted src/old.ts declined" says the file both went and stayed.
    const html = draw({ pathRoot: P,
      items: [patch({ summary: `delete ${P}/a.ts`,
        result: { ok: false, summary: "declined" } }), edit("d1", `${P}/a.ts`, 0, 11)] });
    expect(html).toContain(">Delete<");
    expect(html).not.toContain(">Deleted<");
  });

  it("sizes a patch still being asked about, which is the point", () => {
    // Codex emits its diffs on the STARTED edge so an approval can be READ
    // before it is signed — so the row that names the patch carries its counts
    // while the question is still open, not once it has been answered.
    const html = draw({ pathRoot: P,
      items: [patch({ result: undefined }), edit("d1", `${P}/a.ts`, 14, 23)] });
    expect(classOf(rowByAria(html, /running$/u)))
      .toBe("conv-row conv-diffrow conv-filerow");
    expect(html).toContain('aria-label="Updating, a.ts, 14 lines added, 23 removed, '
      + 'tool fileChange, running"');
    expect(html).toContain("dot-pulse");
    expect(html).toContain("running…</span>");
    expect(html).toContain("+14");
    expect(html).toContain("−23");
  });

  it("keeps the tool's own identifier reachable after the verb took its slot", () => {
    // HEAD printed "File change" in the name column. The verb is there now, so
    // the identifier moves to the title, the accessible name and — once the row
    // is unfolded — the same conv-tool-id block the generic tool row uses.
    const html = draw({ pathRoot: P,
      items: [patch({ result: { ok: false, summary: "boom" } }),
        edit("d1", `${P}/a.ts`, 1, 1)] });
    expect(html).toContain('title="update /tmp/p/a.ts\ntool fileChange"');
    expect(html).toContain("tool fileChange, failed");      // in the accessible name
  });

  it("unfolds the reason, the arguments and the tool id at detailed density", () => {
    // The dial's own promise. The pane reads it from storage at mount, so this
    // is the one place the suite has to supply a window.
    const original = (globalThis as { window?: unknown }).window;
    (globalThis as { window?: unknown }).window = { localStorage: {
      getItem: () => '{"claude#a":"detailed"}', setItem: () => undefined } };
    try {
      const html = draw({ pathRoot: P,
        items: [patch({ result: { ok: false, summary: "patch failed to apply" } }),
          edit("d1", `${P}/a.ts`, 1, 1)] });
      expect(html).toContain('class="conv-tool-id"');
      expect(html).toContain("fileChange</div>");
      expect(html).toContain("patch failed to apply");
      expect(html).toContain('aria-expanded="true"');
    } finally {
      (globalThis as { window?: unknown }).window = original;
    }
  });

  it("collapses one multi-file patch to a count and sums its counts", () => {
    const html = draw({ pathRoot: P, items: [
      patch({ summary: `update ${P}/a.ts, update ${P}/b.ts`,
        result: { ok: true, summary: "2 files" } }),
      edit("d1", `${P}/a.ts`, 6, 12), edit("d2", `${P}/b.ts`, 8, 11),
    ] });
    expect(rows(html)).toBe(1);
    expect(html).toContain(">Updated<");
    expect(html).toContain(">2 files<");
    expect(html).toContain("+14");
    expect(html).toContain("−23");
  });

  it("says a delete happened inside a multi-file patch without anyone hovering", () => {
    // "Changed 2 files" is the shape in the screenshot, and the deletion in it
    // was reachable only on hover.
    const html = draw({ pathRoot: P, items: [
      patch({ summary: `update ${P}/keep.ts, delete ${P}/gone.ts`,
        result: { ok: true, summary: "2 files" } }),
      edit("d1", `${P}/keep.ts`, 2, 0), edit("d2", `${P}/gone.ts`, 0, 3),
    ] });
    expect(html).toContain(">1 deleted, 1 updated<");
    expect(html).not.toContain(">2 files<");
  });

  it("names the kind once for a multi-file patch that took only one", () => {
    const html = draw({ pathRoot: P, items: [
      patch({ summary: [`delete ${P}/a.ts`, `delete ${P}/b.ts`, `delete ${P}/c.ts`]
        .join(", "), result: { ok: true, summary: "3 files" } }),
      edit("d1", `${P}/a.ts`, 0, 3), edit("d2", `${P}/b.ts`, 0, 4),
      edit("d3", `${P}/c.ts`, 0, 5),
    ] });
    expect(html).toContain(">Deleted<");
    expect(html).toContain(">3 files<");
  });

  it("keeps the filename when one file is edited over and over", () => {
    // The modal editing rhythm, and the shape in the screenshot this came from.
    const html = draw({ pathRoot: P, items: [0, 1, 2, 3, 4].flatMap(i => [
      patch({}, `fc${String(i)}`),
      edit(`d${String(i)}`, `${P}/a.ts`, 3, 2, `fc${String(i)}`),
    ]) });
    expect(rows(html)).toBe(5);
    expect(html).not.toContain("Edited 1 file");
    expect(html).not.toContain("Edited 5 files");
    expect(html.match(/>a\.ts</gu)).toHaveLength(5);
  });

  it("draws two files edited in one message as two rows a find can land on", () => {
    // One Claude assistant message with two Edit blocks — the single most
    // common thing this pane renders. Folding them into "Edited 2 files" left
    // one row with no per-file diff, no per-file counts, no state and no find
    // anchor for either call: the corpus still held both keys, so find counted
    // two matches and highlighted nothing.
    const html = draw({ pathRoot: P, items: [
      { kind: "tool", id: "e1", tool: "Edit", summary: `${P}/registry.ts`, input: {},
        result: { ok: true, summary: "ok" } },
      edit("d1", `${P}/registry.ts`, 3, 1, "e1"),
      { kind: "tool", id: "e2", tool: "Edit", summary: `${P}/bridge.ts`, input: {},
        result: { ok: true, summary: "ok" } },
      edit("d2", `${P}/bridge.ts`, 1, 1, "e2"),
    ] });
    expect(rows(html)).toBe(2);
    expect(html).not.toContain("Edited 2 files");
    expect(html).toContain(">registry.ts<");
    expect(html).toContain(">bridge.ts<");
    expect(html).toContain('data-find-keys="l:e1 l:d1"');
    expect(html).toContain('data-find-keys="l:e2 l:d2"');
  });

  it("refuses to put one call's change under another call's outcome", () => {
    // ACP announces both edits before either resolves: call(a), call(b),
    // diff(a), diff(b). Walking forward over "whatever diff rows come next"
    // gave call(b) a row claiming two files, one of them call(a)'s SUCCESSFUL
    // change, under call(b)'s word "failed".
    const html = draw({ pathRoot: P, items: [
      { kind: "tool", id: "a", tool: "edit", summary: "Edit alpha", input: {},
        result: { ok: true, summary: "ok" } },
      { kind: "tool", id: "b", tool: "edit", summary: "Edit beta", input: {},
        result: { ok: false, summary: "permission denied" } },
      edit("d1", `${P}/alpha.ts`, 1, 1, "a"),
      edit("d2", `${P}/beta.ts`, 1, 1, "b"),
    ] });
    expect(html).not.toContain("2 files");
    // Neither call absorbed a diff that is not its own, so both stay tool rows
    // and both diffs keep a row of their own.
    expect(html).toContain(">alpha.ts<");
    expect(html).toContain(">beta.ts<");
  });

  it("gives the verb the quiet tier every other row name wears", () => {
    // The ROW must wear the class the rule targets, or the selector is dead —
    // and the class has to be read off THAT row, since the pane holds other
    // rows that would satisfy a bare toContain.
    const html = draw({ pathRoot: P,
      items: [patch(), edit("d1", `${P}/a.ts`, 14, 23)] });
    expect(classOf(rowByAria(html, /^Updated, a\.ts,/u)))
      .toBe("conv-row conv-diffrow conv-filerow");
    expect(conversationCss).toContain(
      ".conv-tool-row .conv-name, .conv-filerow .conv-name { color:var(--fg-muted); }");
  });

  it("holds the verb at its own width instead of shrinking it with the path", () => {
    // .conv-name and .conv-path shrank at equal priority, and flex shrink is
    // weighted by content basis — so at the 420px pane every row read "Upda…".
    // Scoped to this row: the same rule on a tool row clips the run-state word
    // off a 60-character MCP name, which the comment above .conv-name records.
    const html = draw({ pathRoot: P,
      items: [patch(), edit("d1", `${P}/a.ts`, 14, 23)] });
    expect(classOf(rowByAria(html, /^Updated, a\.ts,/u))).toContain("conv-filerow");
    expect(conversationCss).toContain(".conv-filerow .conv-name { flex:none; }");
  });

  it("starts every glyph in the transcript in one column", () => {
    // The lead spacer put a diff row's glyph 22px in, which was right while a
    // diff row was a CHILD of the row above it. Nothing is a child now, so it
    // indented top-level rows against nothing — the false indent the merge
    // existed to remove, reappearing in any mixed pane. The expectation is
    // derived from the pane rather than restated as a constant.
    const html = draw({ pathRoot: P, items: [
      patch(), edit("d1", `${P}/a.ts`, 1, 1),
      { kind: "diff", id: "o1", callId: "gone", path: `${P}/x.ts`, added: 1, removed: 0 },
      { kind: "diff", id: "o2", callId: "gone", path: `${P}/y.ts`, added: 1, removed: 0 },
    ] });
    const firstSlots = new Set((html.match(/<(?:button|div)[^>]*class="conv-row[^"]*"[^>]*>\s*<span[^>]*/gu) ?? [])
      .map(m => /<span[^>]*class="([^"]*)"/u.exec(m)?.[1] ?? ""));
    expect([...firstSlots]).toEqual(["codicon codicon-git-compare conv-kind"]);
  });

  it("leaves a tool row that produced no diff exactly as it was", () => {
    // A MultiEdit emits no diff.produced at all. There is no pair to merge, so
    // the row must not be rewritten into a file-change row with no files.
    const html = draw({ pathRoot: P, items: [
      { kind: "tool", id: "me", tool: "MultiEdit", summary: `${P}/a.ts`, input: {},
        result: { ok: true, summary: "ok" } },
    ] });
    expect(html).toContain("conv-tool-row");
    expect(html).toContain(">Multi edit<");
    expect(html).not.toContain(">Updated<");
  });
});

describe("findKeysProps", () => {
  // A ref never runs in a static render, so the registration is the one wire on
  // this row that markup cannot see. It is a pure function so that it can be
  // called directly, and it emits the SAME array as an attribute so that the
  // call site's argument is visible in markup — the two halves together are
  // what make a row standing for three keys provably findable under all three.
  const recorder = () => {
    const seen: string[] = [];
    return { seen, find: { active: false, matches: new Set<string>(), currentKey: null,
      register: (key: string) => seen.push(key) } };
  };

  it("registers the row under every key it absorbed, not just the first", () => {
    const { seen, find } = recorder();
    const props = findKeysProps(["t1", "d1", "d2"], find);
    props.ref(null);
    expect(seen).toEqual(["t1", "d1", "d2"]);
  });

  it("prints the same keys it registers, so the markup is evidence about the ref", () => {
    const { find } = recorder();
    expect(findKeysProps(["t1", "d1"], find)["data-find-keys"]).toBe("t1 d1");
  });

  it("clears every key when React hands back a null node", () => {
    // Unmount passes null. Registering the node under one key and leaving the
    // others pointing at a detached element is how find scrolls to nothing.
    const cleared: (HTMLDivElement | null)[] = [];
    findKeysProps(["a", "b"], { active: false, matches: new Set<string>(),
      currentKey: null, register: (_key, node) => cleared.push(node) }).ref(null);
    expect(cleared).toEqual([null, null]);
  });
});

describe("Conversation — working presence", () => {
  it("labels the strip with the live activity and keeps the clock and stop", () => {
    // REFINED, which is what a node render is: Claude Code's one-line busy
    // sentence is CLI's, and this chrome keeps the word and the clock in the
    // two spans it shipped with. The word is the live one the shell computed,
    // not a constant: four tiles all saying "Working" report nothing.
    const html = paint({ thinking: true, workingLabel: "Read `/repo/App.tsx`" });
    expect(html).toContain('class="conv-working"');
    expect(html).toContain('data-provider="claude"');
    expect(html).toContain('class="conv-working-label" aria-hidden="true">Read `/repo/App.tsx`<');
    expect(html).not.toContain("Working…");
    expect(html).not.toContain("Read `/repo/App.tsx`…");
    expect(html).toContain('class="conv-working-elapsed"');
    // The clock reticks every second inside a polite live region, so the
    // visible text is hidden from assistive tech and one static word is
    // announced beside it instead.
    expect(html).toContain('<span class="conv-sr">Working</span>');
    expect(html).toContain('aria-label="Stop"');
  });

  it("prints elapsed from the owned clock", () => {
    const html = paint({ thinking: true, turnStartedAt: Date.now() - 72_000 });
    expect(html).toContain(">Working<");
    expect(html).toContain(">1m 12s<");
  });

  it("keeps the one-second state inside the elapsed leaf, outside Conversation", () => {
    const read = (file: string): string =>
      readFileSync(new URL(`./components/${file}`, import.meta.url), "utf8");
    const rows = read("ConversationRows.tsx");
    // The pane (the shell and both of its halves) holds no clock of its own.
    const conversation = [read("Conversation.tsx"), read("ConversationTranscript.tsx"),
      read("ConversationComposer.tsx")].join("\n");
    const elapsed = rows.slice(rows.indexOf("function LiveTurnElapsed"),
      rows.indexOf("function findItemClass"));
    expect(conversation).not.toContain("setInterval(");
    expect(conversation).not.toContain("setLiveElapsedMs");
    expect(elapsed).toContain("setElapsedMs");
    expect(elapsed).toContain("window.setInterval(tick, 1_000)");
    expect(elapsed).toContain("window.clearInterval(id)");
  });
});

describe("Conversation — the dirty pill above the composer", () => {
  const diffs: ConvItem[] = [
    { kind: "diff", id: "d1", callId: "c-d1", path: "/tmp/a.ts", added: 12, removed: 4 },
    { kind: "diff", id: "d2", callId: "c-d2", path: "/tmp/b.ts", added: 0, removed: 1 },
    { kind: "diff", id: "d3", callId: "c-d3", path: "/tmp/a.ts", added: 2, removed: 0 },
  ];

  it("prints distinct files and the session line pair above the composer", () => {
    const html = paint({ items: diffs });
    const pill = html.indexOf('class="conv-dirty"');
    const composer = html.indexOf('class="conv-composer"');
    expect(pill).toBeGreaterThanOrEqual(0);
    expect(pill).toBeLessThan(composer);
    expect(html).toContain("2 files changed +14 −5");
  });

  /* A spawned agent's edits are the session's edits — the same rule the
     per-turn chip applies. Counting only top-level rows left a session whose
     work all happened inside a Task with no line at all. */
  it("counts the files a subagent edited, not just the top-level ones", () => {
    const html = paint({ items: [
      { kind: "subagent", id: "a1", subagentType: "builder", description: "build",
        status: "done", children: diffs, toolCount: 0, subagentCount: 0 },
    ] });
    expect(html).toContain("2 files changed +14 −5");
  });

  it("omits the pill when counts are unknown or zero", () => {
    expect(paint({ items: [
      { kind: "diff", id: "d1", callId: "c-d1", path: "/tmp/a.ts" },
      { kind: "diff", id: "d2", callId: "c-d2", path: "/tmp/b.ts" },
    ] })).not.toContain("conv-dirty");
    expect(paint({ items: [
      { kind: "diff", id: "d1", callId: "c-d1", path: "/tmp/a.ts", added: 0, removed: 0 },
    ] })).not.toContain("conv-dirty");
    expect(paint({ items: [ASKING] })).not.toContain("conv-dirty");
  });

  it("omits the pill when the only edit was declined", () => {
    const html = paint({ items: [
      { kind: "user", id: "u1", text: "rename foo to bar" },
      { kind: "tool", id: "c-d1", tool: "Edit", summary: "/tmp/a.ts", input: {},
        result: { ok: false, summary: "The user doesn't want to proceed with this tool use." } },
      diffs[0],
    ] });
    expect(html).not.toContain("conv-dirty");
    expect(html).not.toContain("file changed");
  });
});

describe("Conversation — an approval shows the command, not an escaped JSON line", () => {
  it("prints a multi-line command with its line breaks", () => {
    const html = paint({
      items: [{ kind: "approval", id: "ap1", tool: "Bash", summary: "package the app",
        input: { command: "cd /tmp/iface\nnpm run package", timeout: 900000 } }],
    });
    expect(html).toContain("conv-input-label");
    expect(html).toContain("cd /tmp/iface\nnpm run package");
    expect(html).not.toContain("cd /tmp/iface\\nnpm run package");
    expect(html).not.toContain("900000");
  });

  it("leaves a payload with nothing to unescape as the object it is", () => {
    const html = paint({
      items: [{ kind: "approval", id: "ap1", tool: "Read", summary: "read the config",
        input: { file_path: "/tmp/iface/electron-builder.yml" } }],
    });
    expect(html).not.toContain("conv-input-label");
    expect(html).toContain("&quot;file_path&quot;");
  });
});

describe("Conversation — the reading column is sized by the pane, not the viewport", () => {
  it("keeps one centered 720px measure for prose, tools, and the composer", () => {
    const css = conversationCss;
    expect(css).toContain("--conv-measure:720px");
    expect(/\.conv-column \{[^}]*max-width:var\(--conv-measure\)/u.test(css)).toBe(true);
    expect(/\.conv-composer \{[^}]*var\(--conv-measure\)/u.test(css)).toBe(true);
    expect(css).not.toMatch(/\.conv-text \{[^}]*max-width:700px/u);
    expect(css).not.toContain("--conv-bleed");
    expect(css).not.toContain("max-width:960px");
  });

  it("does not widen or bleed the column in a wide pane", () => {
    const css = conversationCss;
    expect(/\.conv \{[^}]*container-type:inline-size/u.test(css)).toBe(true);
    expect(css).not.toMatch(/@container \(min-width: 1000px\)/);
    // Slice 4: assistant prose is a document, not a 14px/1.6 bubble.
    expect(css).toMatch(/\.conv-text \{[^}]*font-size:var\(--text-prose\)/u);
    expect(css).toMatch(/\.conv-text \{[^}]*line-height:var\(--lh-prose\)/u);
    expect(css).toMatch(/\.conv-markdown \{[^}]*font-size:var\(--text-prose\)/u);
    expect(css).toMatch(/\.conv-markdown \{[^}]*line-height:var\(--lh-prose\)/u);
    const inline = /\.conv-md-inline-code \{([^}]*)\}/u.exec(css)?.[1] ?? "";
    expect(inline).toContain("background:var(--surface)");
    expect(inline).not.toContain("border:");
    const user = /\.conv-turn-user \{([^}]*)\}/u.exec(css)?.[1] ?? "";
    expect(user).not.toContain("--surface-raised");
    expect(user).toContain("border:var(--border-w) solid var(--border)");
  });
});

/* ==========================================================================
   TRANSCRIPT DENSITY — the per-tile dial, its persistence, and the pane it
   changes. The tiled workspace is the reason it exists: one verbose agent used
   to flood its 383px pane with no global control to fold it back.
   ========================================================================== */

describe("transcript density — the stored dial", () => {
  it("recognises exactly the three steps", () => {
    expect(isTranscriptDensity("compact")).toBe(true);
    expect(isTranscriptDensity("balanced")).toBe(true);
    expect(isTranscriptDensity("detailed")).toBe(true);
    expect(isTranscriptDensity("verbose")).toBe(false);
    expect(isTranscriptDensity(undefined)).toBe(false);
    expect(isTranscriptDensity(2)).toBe(false);
  });

  it("degrades every corrupt, foreign or hand-edited value to nothing configured", () => {
    expect(parseTranscriptDensities(null)).toEqual({});
    expect(parseTranscriptDensities("not json")).toEqual({});
    expect(parseTranscriptDensities("null")).toEqual({});
    expect(parseTranscriptDensities("[]")).toEqual({});
    expect(parseTranscriptDensities('"compact"')).toEqual({});
    // A value from a future version with a fourth step is ignored rather than
    // rendered as a mode this build cannot draw.
    expect(parseTranscriptDensities('{"claude#a":"cinematic"}')).toEqual({});
    expect(parseTranscriptDensities('{"claude#a":"compact","claude#b":7}'))
      .toEqual({ "claude#a": "compact" });
  });

  it("never reads back a key that says only what absence already says", () => {
    expect(parseTranscriptDensities('{"claude#a":"balanced","claude#b":"detailed"}'))
      .toEqual({ "claude#b": "detailed" });
    // "" is this run's keyless pane. Reading one back would hand one pane's
    // choice to every keyless pane opened afterwards.
    expect(parseTranscriptDensities('{"":"compact"}')).toEqual({});
  });

  it("stops reading at the ceiling instead of loading an unbounded file", () => {
    const many: Record<string, string> = {};
    for (let i = 0; i < TRANSCRIPT_DENSITY_MAX_SESSIONS + 25; i += 1) {
      many[`claude#s${String(i)}`] = "compact";
    }
    const parsed = parseTranscriptDensities(JSON.stringify(many));
    expect(Object.keys(parsed)).toHaveLength(TRANSCRIPT_DENSITY_MAX_SESSIONS);
  });

  it("reports the default for a session nobody has touched", () => {
    expect(transcriptDensityFor({}, "claude#a")).toBe(DEFAULT_TRANSCRIPT_DENSITY);
    expect(transcriptDensityFor({ "claude#a": "compact" }, "claude#b"))
      .toBe(DEFAULT_TRANSCRIPT_DENSITY);
    expect(transcriptDensityFor({ "claude#a": "compact" }, "claude#a")).toBe("compact");
  });

  it("removes a session set back to the default rather than storing the word", () => {
    const set = nextTranscriptDensities({}, "claude#a", "detailed");
    expect(set).toEqual({ "claude#a": "detailed" });
    expect(nextTranscriptDensities(set, "claude#a", "balanced")).toEqual({});
  });

  it("keeps a keyless pane's choice in memory and off disk", () => {
    const map = nextTranscriptDensities({ "claude#a": "compact" }, "", "detailed");
    expect(transcriptDensityFor(map, "")).toBe("detailed");
    expect(JSON.parse(serializeTranscriptDensities(map)))
      .toEqual({ "claude#a": "compact" });
  });

  it("evicts the oldest choices once the ceiling is passed", () => {
    let map: TranscriptDensityMap = {};
    for (let i = 0; i < TRANSCRIPT_DENSITY_MAX_SESSIONS + 3; i += 1) {
      map = nextTranscriptDensities(map, `claude#s${String(i)}`, "compact");
    }
    const keys = Object.keys(map);
    expect(keys).toHaveLength(TRANSCRIPT_DENSITY_MAX_SESSIONS);
    expect(keys[0]).toBe("claude#s3");
    expect(map["claude#s0"]).toBeUndefined();
    // Re-choosing an existing session moves it to the newest slot rather than
    // adding a second entry for it.
    const again = nextTranscriptDensities(map, "claude#s3", "detailed");
    expect(Object.keys(again)).toHaveLength(TRANSCRIPT_DENSITY_MAX_SESSIONS);
    expect(Object.keys(again).at(-1)).toBe("claude#s3");
  });

  it("makes each step a real difference in what the transcript draws", () => {
    // 0 is not 1. A run always has at least one member, so both clear the
    // length test — 0 is the word for "no minimum at all", which the fold pass
    // reads as licence to fold a run of one too.
    expect(toolRunMinimum("compact")).toBe(0);
    expect(toolRunMinimum("balanced")).toBe(1);
    // Unreachable by construction: no run of finite length folds, so every call
    // keeps its own row, which is what "detailed" means.
    expect(toolRunMinimum("detailed")).toBe(Number.POSITIVE_INFINITY);
    expect(transcriptDensityOpensItems("compact")).toBe(false);
    expect(transcriptDensityOpensItems("balanced")).toBe(false);
    expect(transcriptDensityOpensItems("detailed")).toBe(true);
  });
});

describe("Conversation — shell commands collapse like Claude Code", () => {
  const bash = (id: string, over: Partial<Extract<ConvItem, { kind: "tool" }>> = {}):
    ConvItem => ({
    kind: "tool", id, tool: "Bash", summary: id, input: { command: id },
    result: { ok: true, summary: "done" }, ...over,
  });

  it("collapses a mixed stretch of reads and searches into one inspection line", () => {
    const html = paint({
      items: [
        { kind: "tool", id: "r1", tool: "Read", summary: "a.ts", input: {},
          result: { ok: true, summary: "ok" } },
        { kind: "tool", id: "g1", tool: "Grep", summary: "foo", input: {},
          result: { ok: true, summary: "ok" } },
        { kind: "tool", id: "r2", tool: "Read", summary: "b.ts", input: {},
          result: { ok: true, summary: "ok" } },
        { kind: "tool", id: "g2", tool: "Grep", summary: "bar", input: {},
          result: { ok: true, summary: "ok" } },
      ],
    });
    expect(html).toContain("Read 2 files, Searched 2 times");
    expect(html).not.toContain("Called Grep");
    expect(html).not.toContain("conv-card-shell");
  });

  it("collapses commands and edits into Claude Code's work sentence", () => {
    const html = paint({
      items: [
        { kind: "tool", id: "b1", tool: "Bash", summary: "npm test",
          input: { command: "npm test" }, result: { ok: true, summary: "ok" } },
        { kind: "tool", id: "e1", tool: "search_replace", summary: "adapter.ts",
          input: { file_path: "/repo/src/adapter.ts" },
          result: { ok: true, summary: "ok" } },
        { kind: "diff", id: "d1", callId: "e1", path: "/repo/src/adapter.ts",
          added: 21, removed: 6 },
      ],
    });
    expect(html).toContain("Ran a command");
    expect(html).toContain("edited adapter.ts");
    expect(html).toContain("+21");
    expect(html).toContain("−6");
    expect(html).not.toContain("Search replace");
    expect(html).not.toContain("Called Search");
  });

  it("labels a finished pair the way Claude Code does", () => {
    const html = paint({ items: [bash("one"), bash("two")] });
    expect(html).toContain("Ran 2 commands");
    expect(html).not.toContain("2 Bash calls");
    expect(html).not.toContain("completed");
    expect(html).not.toContain("conv-card-shell");
  });

  it("keeps the command itself on a finished run of one", () => {
    // A shell row folds regardless of count, so "Ran 1 command" was replacing
    // an informative line with a strictly less informative one.
    const html = paint({ items: [bash("ls")] });
    expect(html).toContain(">ls<");
    expect(html).not.toContain("Ran 1 command");
  });

  it("keeps an in-flight system call on its own row instead of a folded line", () => {
    // A fold is past tense. While the command is still going, the row saying so
    // outranks any summary of it, so nothing folds and no line claims "Running
    // command" over a call the reader can still watch.
    const html = paint({ items: [bash("npm test", { result: undefined })] });
    expect(html).toContain("conv-state-run");
    expect(html).toContain(">npm test<");
    expect(html).not.toContain("Running command");
    expect(html).not.toContain("conv-card-shell");
  });

  it("leaves a failed command out of the fold rather than inside it", () => {
    // isShellToolRow folded on the tool name alone, so a failed Bash collapsed
    // into the same dim grey line a passing one did. A run now speaks only for
    // calls that finished and worked; the failure keeps its own row, its glyph
    // and its word, and the commands either side of it fold without it.
    const one = paint({ items: [bash("npm run build",
      { result: { ok: false, summary: "exit 2" } })] });
    expect(one).toContain("conv-glyph-fail");
    expect(one).toContain(">failed<");

    const some = paint({ items: [bash("a"), bash("b",
      { result: { ok: false, summary: "exit 1" } }), bash("c"), bash("d")] });
    expect(some).not.toContain("Ran 4 commands");
    expect(some).toContain("conv-glyph-fail");
    expect(some).toContain(">b<");
    // The point of splitting rather than refusing to fold: c and d are still
    // one line, and only the call that broke is outside it.
    expect(some).toContain("Ran 2 commands");

    const clean = paint({ items: [bash("a"), bash("b")] });
    expect(clean).toContain("Ran 2 commands");
    expect(clean).not.toContain("conv-glyph-fail");
    expect(clean).not.toContain("failed");
  });

  it("puts a folded command run on the same grid as every row around it", () => {
    const css = conversationCss;
    // The override said 22px/0 2px against --row-h (28px) and 0 6px, so the
    // one line that folds a command run sat shorter than and left of its
    // neighbours. Height and padding are inherited from the base rule now.
    const base = /\.conv-tool-run-summary \{([^}]*)\}/u.exec(css)?.[1] ?? "";
    expect(base).toContain("min-height:var(--row-h)");
    expect(base).toContain("padding:0 6px");
    const override = /\.conv-tool-run-commands \.conv-tool-run-summary \{([^}]*)\}/u
      .exec(css)?.[1] ?? "";
    expect(override).not.toContain("min-height");
    expect(override).not.toContain("padding");
  });

  it("hides a successful tool's 'done' word and keeps it for the screen reader", () => {
    const html = paint({ items: [{
      kind: "tool", id: "r1", tool: "Read", summary: "Read `/repo/a.ts`",
      input: { target_file: "/repo/a.ts" }, result: { ok: true, summary: "12 lines" },
    }] });
    expect(html).toContain('class="conv-sr">done</span>');
    expect(html).not.toMatch(/class="conv-state conv-inline">done/);
  });

  it("keeps running and failed words visible", () => {
    const running = paint({ items: [{
      kind: "tool", id: "r1", tool: "Read", summary: "Read `/repo/a.ts`",
      input: {},
    }] });
    expect(running).toContain("running…");
    expect(running).not.toContain('class="conv-sr">running');
    const failed = paint({ items: [{
      kind: "tool", id: "r1", tool: "Read", summary: "Read `/repo/a.ts`",
      input: {}, result: { ok: false, summary: "missing" },
    }] });
    expect(failed).toContain("failed");
    expect(failed).not.toContain('class="conv-sr">failed');
  });
});

describe("transcriptHasFoldableRows", () => {
  const prose: ConvItem = { kind: "text", id: "t", text: "hello", streaming: false };
  const call: ConvItem = { kind: "tool", id: "c1", tool: "Bash", summary: "ls", input: {} };
  const agent: ConvItem = { kind: "subagent", id: "a1", subagentType: "reviewer",
    description: "review", status: "done", children: [], toolCount: 0, subagentCount: 0 };

  it("says no over a transcript the dial cannot change", () => {
    expect(transcriptHasFoldableRows([])).toBe(false);
    expect(transcriptHasFoldableRows(mergeTranscript([], [prose]))).toBe(false);
    expect(transcriptHasFoldableRows(mergeTranscript([prose], [prose]))).toBe(false);
  });

  it("says yes for a tool call or a subagent card, on either side of the seam", () => {
    expect(transcriptHasFoldableRows(mergeTranscript([], [call]))).toBe(true);
    expect(transcriptHasFoldableRows(mergeTranscript([], [agent]))).toBe(true);
    expect(transcriptHasFoldableRows(mergeTranscript([call], [prose]))).toBe(true);
  });
});

/* The pane's own localStorage boundary. `environment: "node"` gives these tests
   no window at all, which is itself the "storage is unavailable" case the
   reader has to survive; a stub supplies the other one. */
function withStoredDensities(stored: string | null, run: () => void): void {
  const cell = { value: stored };
  vi.stubGlobal("window", {
    localStorage: {
      getItem: (key: string) => (key === TRANSCRIPT_DENSITY_KEY ? cell.value : null),
      setItem: (key: string, value: string) => {
        if (key === TRANSCRIPT_DENSITY_KEY) cell.value = value;
      },
    },
  });
  try { run(); } finally { vi.unstubAllGlobals(); }
}

afterEach(() => { vi.unstubAllGlobals(); });

describe("Conversation — the on-demand transcript detail control", () => {
  const call = (id: string): ConvItem =>
    ({ kind: "tool", id, tool: "Read", summary: `read ${id}.ts`,
      input: { file_path: `/repo/${id}.ts` }, result: { ok: true, summary: "12 lines" } });
  const prose: ConvItem =
    { kind: "text", id: "t1", text: "Reading the parser.", streaming: false };

  it("charges no chrome to a transcript it would not change", () => {
    const html = paint({ items: [prose] });
    expect(html).not.toContain('class="conv-iconbtn conv-detail"');
  });

  it("moves session controls beneath the primary actions in a narrow pane", () => {
    const html = conversationCss;
    expect(html).toContain("@container (max-width: 420px)");
    expect(html).toContain(".conv-pills { order:2; flex:1 1 100%; width:100%; }");
    expect(html).toContain(".conv-actions { order:1; }");
  });

  it("shows the current detail and next step without a permanent control strip", () => {
    const html = paint({ items: [prose, call("a")] });
    expect(html).toContain('class="conv-iconbtn conv-detail"');
    expect(html).toContain('aria-label="Transcript detail: Balanced. Change to Detailed: Everything open"');
    expect(html).not.toContain('role="group" aria-label="Transcript detail"');
    expect(html).not.toContain('class="conv-density-opt');
    // THE STEP IS VISIBLE, not only in the tooltip. One glyph across all three
    // states made a cycling control that could not say where the cycle was.
    expect(html).toContain('data-density="balanced"');
    expect(html).toContain("codicon-list-selection");
  });

  it("draws a different glyph on each rung of the dial", () => {
    const items = [prose, call("a")];
    withStoredDensities('{"claude#a":"compact"}', () => {
      const html = paint({ items });
      expect(html).toContain('data-density="compact"');
      expect(html).toContain("codicon-list-flat");
    });
    withStoredDensities('{"claude#a":"detailed"}', () => {
      const html = paint({ items });
      expect(html).toContain('data-density="detailed"');
      expect(html).toContain("codicon-list-tree");
    });
  });

  it("folds a pair of reads at balanced the same way compact does", () => {
    const two = [call("a"), call("b")];
    expect(paint({ items: two })).toContain("Read 2 files");
    withStoredDensities('{"claude#a":"compact"}', () => {
      const html = paint({ items: two });
      expect(html).toContain("Read 2 files");
      expect(html).toContain('aria-label="Transcript detail: Compact. Change to Balanced: Repeats fold · single calls show"');
    });
  });

  it("folds a lone call at compact, which is the step balanced leaves open", () => {
    // THE THREE-WAY DIFFERENCE, on the one transcript that used to expose the
    // dial's lie: a single call. Balanced kept it as its own named row and so
    // did compact, so the leftmost step changed nothing at all.
    const one = [call("a")];
    const balanced = paint({ items: one });
    expect(balanced).toContain('class="conv-row conv-tool-row" aria-expanded="false"');
    expect(balanced).toContain("read a.ts");
    expect(balanced).not.toContain("Read file");
    withStoredDensities('{"claude#a":"compact"}', () => {
      const html = paint({ items: one });
      expect(html).toContain("Read file");
      expect(html).not.toContain('class="conv-row conv-tool-row"');
      expect(html).not.toContain("read a.ts");
    });
    withStoredDensities('{"claude#a":"detailed"}', () => {
      const html = paint({ items: one });
      expect(html).toContain('class="conv-row conv-tool-row" aria-expanded="true"');
      expect(html).toContain("/repo/a.ts");
      expect(html).toContain("12 lines");
    });
  });

  it("folds a stretch of pure edits at compact, and keeps a row per file otherwise", () => {
    const edits: ConvItem[] = [
      { kind: "tool", id: "e1", tool: "Edit", summary: "edit a.ts",
        input: { file_path: "/repo/a.ts" }, result: { ok: true, summary: "ok" } },
      { kind: "diff", id: "d1", callId: "e1", path: "/repo/a.ts", added: 4, removed: 1 },
      { kind: "tool", id: "e2", tool: "Edit", summary: "edit b.ts",
        input: { file_path: "/repo/b.ts" }, result: { ok: true, summary: "ok" } },
      { kind: "diff", id: "d2", callId: "e2", path: "/repo/b.ts", added: 2, removed: 0 },
    ];
    const balanced = paint({ items: edits });
    expect(balanced).toContain("conv-filerow");
    expect(balanced).not.toContain("edited 2 files");
    withStoredDensities('{"claude#a":"compact"}', () => {
      const html = paint({ items: edits });
      expect(html).toContain("edited 2 files");
      expect(html).not.toContain("conv-filerow");
    });
    withStoredDensities('{"claude#a":"detailed"}', () => {
      const html = paint({ items: edits });
      expect(html).toContain("conv-filerow");
      expect(html).not.toContain("edited 2 files");
    });
  });

  it("never folds a pending approval out of reach at compact", () => {
    // The binding constraint on folding everything: an approval is the one card
    // the user MUST be able to act on, and compact folds harder than any step
    // before it. It survives because isWorkItemRow matches tool and diff items
    // only — this pins that rather than trusting the reading of it.
    withStoredDensities('{"claude#a":"compact"}', () => {
      const html = paint({ items: [ASKING, call("a")] });
      expect(html).toContain('class="conv-approval conv-approval-pending"');
      expect(html).toContain("Allow</button>");
      expect(html).toContain("Deny</button>");
    });
  });

  it("says declined, not failed, for a patch the user refused", () => {
    // A refusal folded into a run used to borrow the only word the run knew.
    // Reporting the user's own "no" as a breakage is the one thing this pane
    // must never do, at any step of the dial.
    const refused: ConvItem[] = [
      { kind: "tool", id: "e1", tool: "apply_patch", summary: "edit a.ts",
        input: { file_path: "/repo/a.ts" }, result: { ok: false, summary: "declined" } },
      { kind: "diff", id: "d1", callId: "e1", path: "/repo/a.ts", added: 4, removed: 1 },
    ];
    withStoredDensities('{"claude#a":"compact"}', () => {
      const html = paint({ items: refused });
      expect(html).toContain("declined");
      expect(html).not.toContain("failed");
    });
    // The word the folded line borrows is the word the row inside it uses, so
    // opening the run cannot change the verdict.
    withStoredDensities('{"claude#a":"detailed"}', () => {
      const html = paint({ items: refused });
      expect(html).toContain("declined");
      expect(html).not.toContain("failed");
    });
    // One of each still says both — but on the row of the call each belongs to,
    // because neither a refusal nor a failure folds into a shared line any more.
    const mixed: ConvItem[] = [...refused,
      { kind: "tool", id: "b1", tool: "Bash", summary: "npm test",
        input: { command: "npm test" }, result: { ok: false, summary: "exit 1" } }];
    withStoredDensities('{"claude#a":"compact"}', () => {
      const html = paint({ items: mixed });
      expect(html).toContain("declined");
      expect(html).toContain("failed");
      expect(html).not.toContain("1 failed, 1 declined");
    });
  });

  it("gives every step its own hint, now that every step does its own thing", () => {
    const hints = (["compact", "balanced", "detailed"] as const).map(density => {
      let html = "";
      withStoredDensities(`{"claude#a":"${density}"}`, () => {
        html = paint({ items: [prose, call("a")] });
      });
      return html;
    });
    expect(hints[0]).toContain('Change to Balanced: Repeats fold · single calls show"');
    expect(hints[1]).toContain('Change to Detailed: Everything open"');
    expect(hints[2]).toContain('Change to Compact: Finished runs fold · summaries only"');
  });

  it("unfolds every card at detailed, and keeps each repeated call on its own row", () => {
    const three = [call("a"), call("b"), call("c")];
    // Balanced groups three identical calls behind one summary row.
    const folded = paint({ items: three });
    expect(folded).toContain("Read 3 files");
    expect(folded).not.toContain('class="conv-row conv-tool-row"');
    expect(folded).not.toContain("/repo/a.ts");
    withStoredDensities('{"claude#a":"detailed"}', () => {
      const html = paint({ items: three });
      expect(html).not.toContain("Read 3 files");
      expect(html).not.toContain('class="conv-row conv-tool-row" aria-expanded="false"');
      expect(html.match(/class="conv-row conv-tool-row" aria-expanded="true"/gu))
        .toHaveLength(3);
      // Unfolded means the arguments and the result are really on screen.
      expect(html).toContain("/repo/a.ts");
      expect(html).toContain("12 lines");
    });
  });

  it("seeds a subagent card from the same dial", () => {
    const agent: ConvItem = { kind: "subagent", id: "a1", subagentType: "reviewer",
      description: "audit the pane", status: "done", toolCount: 1, subagentCount: 0,
      children: [call("x")] };
    expect(paint({ items: [agent] })).not.toContain("sub-kids");
    withStoredDensities('{"claude#a":"detailed"}', () => {
      expect(paint({ items: [agent] })).toContain("sub-kids");
    });
  });

  it("reads one session's dial without applying it to another", () => {
    withStoredDensities('{"claude#a":"detailed"}', () => {
      expect(paint({ items: [call("a")], sessionKey: "claude#b" }))
        .toContain('aria-label="Transcript detail: Balanced. Change to Detailed: Everything open"');
    });
  });

  it("still draws a working dial when storage is unavailable", () => {
    // No window at all: reading must not throw, and the default must stand.
    const html = paint({ items: [call("a")] });
    expect(html).toContain('aria-label="Transcript detail: Balanced. Change to Detailed: Everything open"');
  });
});

/* ==========================================================================
   THE SUBAGENT TREE — what a card claims about the work it delegated, and how
   a nested card names the agent that spawned it.
   ========================================================================== */

describe("conversationItems — a subagent counts the agents it spawned", () => {
  it("counts a nested spawn as a subagent rather than as one of its own tools", () => {
    const items = conversationItems([
      { kind: "tool.call", callId: "a1", tool: "Task", summary: "explore", input: {} },
      { kind: "subagent.started", agentKey: "a1", subagentType: "explore",
        description: "map the repo" },
      { kind: "tool.call", callId: "a2", tool: "Task", summary: "review", input: {},
        agentKey: "a1" },
      { kind: "subagent.started", agentKey: "a2", subagentType: "review",
        description: "read the diff" },
      { kind: "tool.call", callId: "c1", tool: "Read", summary: "read", input: {},
        agentKey: "a2" },
      { kind: "subagent.finished", agentKey: "a2", ok: true, summary: "done" },
      { kind: "subagent.finished", agentKey: "a1", ok: true, summary: "mapped" },
    ]);
    const parent = sub(items[0]);
    expect(parent.subagentCount).toBe(1);
    // The parent called nothing itself: its ONE tool.call was the spawn, and
    // the card IS that call.
    expect(parent.toolCount).toBe(0);
    const child = sub(parent.children[0]);
    expect(child.subagentCount).toBe(0);
    expect(child.toolCount).toBe(1);
  });

  it("is zero for a subagent that delegated nothing", () => {
    const items = conversationItems([
      { kind: "subagent.started", agentKey: "a1", subagentType: "explore",
        description: "map the repo" },
      { kind: "tool.call", callId: "c1", tool: "Read", summary: "read", input: {},
        agentKey: "a1" },
    ]);
    expect(sub(items[0]).subagentCount).toBe(0);
  });
});

describe("conversationItems — what the subagent REPORTED, apart from what it did", () => {
  it("records the closing summary as the result, not only as activity", () => {
    // Two different claims. `lastActivity` falls back to whatever the agent was
    // doing when the stream stopped, so a surface that prints one under the
    // heading "result" would put a half-finished tool summary there.
    const items = conversationItems([
      { kind: "subagent.started", agentKey: "a1", subagentType: "explore",
        description: "map the repo" },
      { kind: "tool.call", callId: "c1", tool: "Read", summary: "read a.ts",
        input: {}, agentKey: "a1" },
      { kind: "subagent.finished", agentKey: "a1", ok: true,
        summary: "Mapped 12 modules; the reducer is the only shared surface." },
    ]);
    expect(sub(items[0]).result)
      .toBe("Mapped 12 modules; the reducer is the only shared surface.");
    expect(sub(items[0]).lastActivity).toBe(sub(items[0]).result);
  });

  it("leaves the result unset when the agent never reported one", () => {
    const running = conversationItems([
      { kind: "subagent.started", agentKey: "a1", subagentType: "explore",
        description: "map the repo" },
      { kind: "tool.call", callId: "c1", tool: "Read", summary: "read a.ts",
        input: {}, agentKey: "a1" },
    ]);
    expect(sub(running[0]).result).toBeUndefined();
    expect(sub(running[0]).lastActivity).toBe("read a.ts");

    // Finished with an empty summary is still "reported nothing".
    const silent = conversationItems([
      { kind: "subagent.started", agentKey: "a1", subagentType: "explore",
        description: "map the repo" },
      { kind: "subagent.finished", agentKey: "a1", ok: true, summary: "" },
    ]);
    expect(sub(silent[0]).result).toBeUndefined();
  });
});

describe("Conversation — the subagent tree", () => {
  function orchestrator(children: ConvItem[]): ConvItem {
    return { kind: "subagent", id: "a1", subagentType: "orchestrator",
      description: "split the audit", status: "done", toolCount: 0,
      subagentCount: children.filter(c => c.kind === "subagent").length, children };
  }
  const worker: ConvItem = { kind: "subagent", id: "a2", subagentType: "reviewer",
    description: "read the diff", status: "done", toolCount: 3, subagentCount: 0,
    children: [], lastActivity: "Read src/renderer/App.tsx" };

  it("reports delegated work instead of claiming there was none", () => {
    // The defect: toolCount deliberately excludes a nested spawn, so a card
    // that split all of its work across two others had no tools, no activity
    // line of its own, and printed "no activity" over two finished agents.
    const html = paint({ items: [orchestrator([worker, { ...worker, id: "a3" }])] });
    expect(html).toContain("2 subagents");
    expect(html).not.toContain("no activity");
  });

  it("says one subagent in the singular, and keeps tools and last activity after it", () => {
    const one = orchestrator([worker]) as Extract<ConvItem, { kind: "subagent" }>;
    one.toolCount = 2;
    one.lastActivity = "Bash npm test";
    expect(paint({ items: [one] }))
      .toContain("1 subagent · 2 tools · last: Bash npm test");
  });

  it("names the agent that spawned a nested card, and only a nested one", () => {
    withStoredDensities('{"claude#a":"detailed"}', () => {
      const html = paint({ items: [orchestrator([worker])] });
      expect(html).toContain('aria-label="orchestrator subagent"');
      expect(html).toContain('aria-label="reviewer subagent, spawned by orchestrator"');
      expect(html).toContain('data-subagent-parent="orchestrator"');
    });
  });
});

/* ==========================================================================
   INLINE PER-TURN CHECKPOINT RESTORE. The pane owns no checkpoint machinery:
   it draws a control exactly where the caller says a restore point stands, and
   nowhere else.
   ========================================================================== */

describe("turnRestoreConfirmation", () => {
  const point: TurnRestorePoint = { description: "Restores 4 files." };
  const points = new Map([["l:user-1", point]]);

  it("is nothing to confirm until a turn is pending", () => {
    expect(turnRestoreConfirmation(null, points)).toBeNull();
  });

  it("resolves a pending turn to the point it named", () => {
    expect(turnRestoreConfirmation("l:user-1", points)).toEqual({ key: "l:user-1", point });
  });

  it("takes the dialog down when the point behind it is gone", () => {
    // The caller rebuilds this map — a refreshed checkpoint list, a restore
    // that just landed. A confirm button still pointed at a checkpoint that no
    // longer exists is the failure this control was added to prevent.
    expect(turnRestoreConfirmation("l:user-1", new Map())).toBeNull();
    expect(turnRestoreConfirmation("l:user-1", undefined)).toBeNull();
  });
});

describe("Conversation — restore at the turn the mistake is visible from", () => {
  const turn: ConvItem = { kind: "user", id: "user-0", text: "delete the parser" };
  const point: TurnRestorePoint = {
    description: "Restores 14 files in ~/Projects/Interface to the checkpoint taken "
      + "6 minutes ago. Later edits to those files are lost.",
  };
  const points = new Map([["l:user-0", point]]);

  it("draws nothing without BOTH halves of the contract", () => {
    expect(paint({ items: [turn] })).not.toContain("conv-turn-restore");
    expect(paint({ items: [turn], turnRestorePoints: points }))
      .not.toContain("conv-turn-restore");
    expect(paint({ items: [turn], onRestoreTurn: () => undefined }))
      .not.toContain("conv-turn-restore");
  });

  it("draws it only on a turn the caller supplied a restore point for", () => {
    const html = paint({
      items: [turn, { kind: "user", id: "user-9", text: "no, keep it" }],
      turnRestorePoints: points, onRestoreTurn: () => undefined,
    });
    expect(html.match(/class="conv-turn-restore"/gu)).toHaveLength(1);
    expect(html).toContain('aria-label="Restore files to before this message"');
    expect(html).toContain(">Restore<");
    // Both turns still read as the user's own words.
    expect(html.match(/class="conv-turn-who"/gu)).toHaveLength(2);
  });

  it("uses the caller's own short label when it supplied one", () => {
    const labelled = new Map([["l:user-0", { ...point, label: "Undo · 14 files" }]]);
    expect(paint({ items: [turn], turnRestorePoints: labelled,
      onRestoreTurn: () => undefined })).toContain("Undo · 14 files");
  });

  it("says which restore is running and refuses a second one meanwhile", () => {
    const two = new Map([["l:user-0", point], ["l:user-9", point]]);
    const html = paint({
      items: [turn, { kind: "user", id: "user-9", text: "no, keep it" }],
      turnRestorePoints: two, onRestoreTurn: () => undefined,
      restoringTurnKey: "l:user-0",
    });
    expect(html).toContain("Restoring…");
    // Two concurrent rollbacks of one working tree is not a state to offer.
    expect(html.match(/class="conv-turn-restore" disabled=""/gu)).toHaveLength(2);
  });

  it("never offers a rollback for a prompt the human did not type", () => {
    // A user turn nested inside a subagent is the PARENT AGENT's prompt to it.
    // No checkpoint stands before it, and the caller's key space cannot name it.
    withStoredDensities('{"claude#a":"detailed"}', () => {
      const html = paint({
        items: [{ kind: "subagent", id: "a1", subagentType: "reviewer",
          description: "review", status: "done", toolCount: 0, subagentCount: 0,
          children: [{ kind: "user", id: "inner", text: "delete the parser" }] }],
        turnRestorePoints: new Map([["l:a1/0", point]]),
        onRestoreTurn: () => undefined,
      });
      expect(html).toContain("sub-kids");
      expect(html).not.toContain("conv-turn-restore");
    });
  });
});

/* ==========================================================================
   THE PER-TURN CHANGES CHIP. Orientation and nothing else: it says what this
   turn did to the working tree, and it opens nothing.
   ========================================================================== */

describe("Conversation — the changes chip on a user turn", () => {
  const turn: ConvItem = { kind: "user", id: "user-0", text: "fix the reorder" };
  const wrote = (id: string, path: string, added: number, removed: number): ConvItem =>
    ({ kind: "diff", id, path, callId: `c-${id}`, added, removed });

  it("says what the turn changed, in the diff vocabulary and not a pill", () => {
    const html = paint({
      items: [turn, wrote("d0", "/repo/a.ts", 40, 7), wrote("d1", "/repo/b.ts", 22, 4)],
    });
    expect(html.match(/class="conv-turn-changes"/gu)).toHaveLength(1);
    expect(html).toContain("+62");
    expect(html).toContain("−11");
    // The delta and nothing else: the row's one file count belongs to Undo.
    // Scoped to the chip on purpose — the diff row below it and the dirty pill
    // in the strip are different surfaces and keep counts of their own.
    const chip = html.slice(html.indexOf('class="conv-turn-changes"'));
    expect(chip.slice(0, chip.indexOf("</span>"))).not.toContain("file");
    expect(html).toContain(
      'aria-label="Review changes from this message: 62 lines added, 11 removed"');
    // Text, not a badge: the counts wear the same classes the file rows use.
    expect(html).toContain('class="conv-stat-add"');
    expect(html).toContain('class="conv-stat-del"');
  });

  it("draws nothing at all over a turn that changed nothing", () => {
    expect(paint({ items: [turn] }))
      .not.toContain('class="conv-turn-changes"');
    expect(paint({
      items: [turn, { kind: "tool", id: "t0", tool: "Read", summary: "read a.ts", input: {} }],
    })).not.toContain('class="conv-turn-changes"');
  });

  it("hangs the changes on the prompt that caused them", () => {
    const html = paint({
      items: [turn, wrote("d0", "/repo/a.ts", 3, 1),
        { kind: "user", id: "user-9", text: "now revert it" }],
    });
    // One chip for two user turns: the second has produced nothing yet.
    expect(html.match(/class="conv-turn-changes"/gu)).toHaveLength(1);
    expect(html.match(/class="conv-turn-who"/gu)).toHaveLength(2);
  });

  /* A diff row with no line counts leaves the chip with nothing to say — the
     file count it used to fall back on is Undo's now — so it is not drawn.
     Empty is not a state a control gets to be in. */
  it("draws no chip at all when no line counts were seen", () => {
    const html = paint({
      items: [turn, { kind: "diff", id: "d0", path: "/repo/a.ts", callId: "c" }],
    });
    expect(html).not.toContain('class="conv-turn-changes"');
    expect(html).not.toContain("1 file");
    expect(html).not.toContain("+0");
  });

  it("stands beside the restore control rather than instead of it", () => {
    const html = paint({
      items: [turn, wrote("d0", "/repo/a.ts", 3, 1)],
      turnRestorePoints: new Map([["l:user-0", { description: "Restores 1 file." }]]),
      onRestoreTurn: () => undefined,
    });
    expect(html).toContain('class="conv-turn-changes"');
    expect(html).toContain('class="conv-turn-restore"');
  });

  it("states the delta without pretending to be a control", () => {
    const html = paint({ items: [turn, wrote("d0", "/repo/a.ts", 3, 1)] });
    expect(html).toContain('<span class="conv-turn-changes"');
    expect(html).not.toContain('class="conv-turn-changes" disabled=""');
  });

  it("never hangs a chip on a prompt the human did not type", () => {
    // A user turn nested inside a subagent is the parent agent's prompt to it.
    // Its edits are already counted on the human turn that spawned the agent.
    withStoredDensities('{"claude#a":"detailed"}', () => {
      const html = paint({
        items: [{ kind: "subagent", id: "a1", subagentType: "reviewer",
          description: "review", status: "done", toolCount: 0, subagentCount: 0,
          children: [{ kind: "user", id: "inner", text: "rewrite the parser" },
            wrote("in0", "/repo/a.ts", 3, 1)] }],
      });
      expect(html).toContain("sub-kids");
      expect(html).not.toContain('class="conv-turn-changes"');
    });
  });
});

describe("the shell's half of the restore contract", () => {
  // This contract shipped complete and TESTED, and dark: no caller passed
  // either prop, so the control could not appear on any turn in the app. The
  // assertions below are about the WIRING, which is the part a rewrite of App
  // can silently drop again without a single test above going red.
  const app = readFileSync(new URL("./App.tsx", import.meta.url), "utf8");
  const sessionTile = readFileSync(new URL("./components/SessionTile.tsx", import.meta.url), "utf8");
  const turnRestore = readFileSync(new URL("./use-turn-restore.ts", import.meta.url), "utf8");

  it("passes both halves from the workspace tile", () => {
    expect(app).toContain("turnRestorePoints={turnRestorePoints[key]}");
    expect(sessionTile).toContain("onRestoreTurn={turnKey => { restoreTurnCheckpoint(key, turnKey); }}");
  });

  it("takes the row key from the pane rather than guessing one", () => {
    // The key for a just-sent message is its local echo, and the echo's id is
    // minted INSIDE the pane. onPendingEcho hands that echo over one statement
    // before onSend, so the shell names the row mergeTranscript will build
    // (`e:<id>`) instead of predicting it from the three lists.
    expect(sessionTile).toContain("checkpointTurn(key, `e:${echo.id}`)");
  });

  it("checkpoints every user dispatch, and never blocks one", () => {
    // The queue path has no transcript echo by design, so it passes no row.
    expect(app).toContain("checkpointTurn(key, null);");
    // "turn" is what makes an automatic capture prunable and, when it found
    // nothing, unwritten — see createReviewCheckpoint in main.
    expect(turnRestore).toContain("void api.createReviewCheckpoint(root, \"turn\")");
    expect(turnRestore).toContain("console.warn(\"[app] could not checkpoint this turn:\", err);");
    // No entry, no control: an empty checkpoint is never recorded.
    expect(turnRestore).toContain("if (turnKey === null || created.fileCount === 0) return;");
  });

  it("reports the restore where the pane deliberately does not", () => {
    expect(turnRestore).toContain("void api.restoreReviewCheckpoint(summary.cwd, id)");
    expect(turnRestore).toContain("skipped or conflicted.");
    expect(turnRestore).toContain("A recovery checkpoint was saved first.");
    // A clean restore is a result, not an error strip.
    expect(turnRestore).toContain("error: result.conflicts.length > 0,");
  });

  it("refuses the rollback while the turn is still open", () => {
    // Undo stands on the turn the user wants back, and that turn may still be
    // running. Putting old bytes under a live writer leaves the tree as
    // neither the checkpoint nor the agent's work, so it is refused with the
    // reason rather than quietly disabled — and `waiting` counts: a tile asking
    // for an approval looks stopped, which is when the click is likeliest, and
    // answering it resumes writing over whatever the restore put back. Only the
    // two settled states get through.
    expect(turnRestore).toContain("if (summary.status !== \"idle\" && summary.status !== \"failed\") {");
    expect(turnRestore).toContain("This agent is still running or waiting on an approval.");
  });

  it("takes a forgotten session's checkpoints with it", () => {
    // Its echoes are gone, so every row key those entries name is gone; the
    // origin's quota is shared with the steering queue and the layout.
    expect(app).toContain("setTurnRestorePoints(map => withoutKey(map, key));");
    expect(app).toContain("forgetTurnCheckpoints(key);");
  });

  it("follows the turn when the transcript settles the echo it was hung on", () => {
    // Every ACP engine streams the user's own message back, which drops the
    // echo — without this the control disappears a second after the send.
    expect(turnRestore).toContain("const settled = settledEchoRowKey(past, live, echo);");
    expect(turnRestore).toContain("rekeyTurnCheckpoint(key, oldKey, settled);");
    // And the walk is paid for only when a live user turn actually arrived.
    expect(app).toContain("if (items.some(item => item.evt.kind === \"user.message\")) {");
  });
});

describe("Conversation — a subagent card with nothing under it yet", () => {
  const spawned = (children: ConvItem[]): ConvItem => ({
    kind: "subagent", id: "a1", subagentType: "reviewer", description: "audit",
    status: "running", toolCount: 0, subagentCount: 0, children,
  });

  /** The cell only. The pane's own chrome has disclosures of its own, and an
   *  assertion against the whole markup passes on those instead. */
  function cell(html: string): string {
    const start = html.indexOf('<section class="sub');
    return html.slice(start, html.indexOf("</section>", start) + "</section>".length);
  }

  it("draws no disclosure at all over a panel that is not there", () => {
    // The ordinary state between subagent.started and that subagent's first
    // event. The dial seeds every card open at Detailed, and the panel was
    // correctly withheld while there was nothing to put in it — so the outline,
    // the caret and aria-expanded were the three that lied. The caret is now
    // absent rather than hidden, which is the same truth with no tab stop.
    withStoredDensities('{"claude#a":"detailed"}', () => {
      const html = cell(paint({ items: [spawned([])] }));
      expect(html).not.toContain("aria-expanded");
      expect(html).not.toContain("sub-fold");
      expect(html).not.toContain("sub-open");
      expect(html).not.toContain("conv-chev-open");
      expect(html).not.toContain("aria-controls");
      expect(html).not.toContain("sub-kids");
    });
  });

  it("opens the moment that subagent has something to show", () => {
    withStoredDensities('{"claude#a":"detailed"}', () => {
      const html = paint({
        items: [spawned([{ kind: "text", id: "t", text: "reading", streaming: false }])],
      });
      expect(html).toContain('aria-expanded="true"');
      expect(html).toContain("sub-open");
      expect(html).toContain("conv-chev-open");
      expect(html).toContain("sub-kids");
    });
  });
});

/* ==========================================================================
   THE PANE, PRESSED.

   Everything above pins what the pane DRAWS. renderToStaticMarkup cannot press
   anything, so every handler in this file was unpinned — including the one
   control in the transcript that overwrites files on disk, whose confirm,
   cancel and session-switch paths all survived being deleted.

   The pane is driven here instead, with no DOM and no new dependency, by
   calling `Conversation` as the plain function it is from inside a probe
   component's own render. Its hooks are then allocated on the PROBE, so its
   state is real state and its own setters are render-phase updates on the
   component React is currently rendering — the documented case React answers
   by throwing the pass away and running it again. Pressing a handler taken out
   of the returned element tree therefore produces the next frame, exactly as a
   click would.

   Two rules make it honest rather than clever:
     · every component the probe reaches into is invoked on EVERY pass, in the
       same order, or the hooks shift under React and it says so loudly;
     · a pass that schedules a re-seed has not settled yet, so each step gets a
       settle pass of its own before the next press. What a test reads is the
       frame the user would have been looking at.
   ========================================================================== */

type PressEl = { type: unknown; props: Record<string, unknown> };

const isPressEl = (value: unknown): value is PressEl =>
  typeof value === "object" && value !== null && "type" in value && "props" in value;

/** One component's own output, depth first. Children of nested components are
 *  NOT in here — those are separate renders, reached with `into`. */
function elements(tree: unknown, out: PressEl[] = []): PressEl[] {
  if (Array.isArray(tree)) {
    for (const node of tree) elements(node, out);
    return out;
  }
  if (!isPressEl(tree)) return out;
  out.push(tree);
  elements((tree.props as { children?: unknown }).children, out);
  return out;
}

function only(tree: unknown, match: (el: PressEl) => boolean): PressEl {
  const hits = elements(tree).filter(match);
  if (hits.length !== 1) {
    throw new Error(`expected exactly one match, found ${String(hits.length)}`);
  }
  return hits[0];
}

const has = (tree: unknown, match: (el: PressEl) => boolean): boolean =>
  elements(tree).some(match);

/**
 * The function a node would render, or null when the node is a host element.
 * A memo()'d component is an object holding the real function on `.type`, so
 * this looks through one — the transcript's row boundary is memoized and the
 * probe still has to be able to call it.
 */
function componentOf(el: PressEl): ((props: unknown) => unknown) | null {
  if (typeof el.type === "function") return el.type as (props: unknown) => unknown;
  const inner = (el.type as { type?: unknown } | null)?.type;
  return typeof inner === "function" ? inner as (props: unknown) => unknown : null;
}

const isComponent = (el: PressEl): boolean => componentOf(el) !== null;

/** Render one nested component. Its hooks land on the probe too, which is what
 *  gives a card the fold state it really keeps between presses. */
const into = (el: PressEl): unknown => {
  const render = componentOf(el);
  if (render === null) throw new Error("into() was handed a host element");
  return render(el.props);
};

/** The row the pane drew for one transcript key. */
const itemRow = (tree: unknown, findKey: string): PressEl =>
  only(tree, el => isComponent(el) && el.props.findKey === findKey);

/** That row, rendered all the way down to the card that owns the fold or the
 *  control: SearchableItem wraps Item wraps the card, and none of the three is
 *  exported. Every hop INSIDE here is unconditional so the hook order cannot
 *  drift. One caller — the toolFold probe below — deliberately skips this whole
 *  chain at Compact, where the call it looks for has folded into a run and has
 *  no card at all; that is the single sanctioned exception, and the pressed-dial
 *  tests cover the frames on both sides of it. */
function itemCard(tree: unknown, findKey: string): unknown {
  const item = only(into(itemRow(tree, findKey)), el => isComponent(el));
  return into(only(into(item), el => isComponent(el)));
}

type Frame = { tree: unknown; card: unknown };

/**
 * The pane as the probe draws it: the shell's own output, then its two
 * memoized halves (the transcript and the composer) rendered on the probe too,
 * always in that order so the hooks never shift. The frame is all three trees.
 */
function drawPane(props: ConversationProps): unknown {
  const shell = (Conversation as unknown as (p: ConversationProps) => unknown)(props);
  const halves = elements(shell).filter(el =>
    el.type === ConversationTranscript || el.type === ConversationComposer);
  return [shell, ...halves.map(into)];
}

/**
 * Render `panes` side by side and press through `acts`, one press per step.
 * Gives back the settled frame for every step, plus the frame after the last
 * press — `frames[acts.length]` is where a test looks for the end state.
 */
function press(
  panes: readonly ((step: number) => ConversationProps)[],
  acts: readonly ((frames: readonly Frame[]) => void)[],
  reach: (tree: unknown) => unknown = () => null,
): Frame[][] {
  const frames: Frame[][] = [];
  function Probe() {
    const [at, setAt] = useState({ step: 0, settled: false });
    const drawn = panes.map(pane => {
      const tree = drawPane(pane(at.step));
      return { tree, card: reach(tree) };
    });
    frames[at.step] = drawn;
    if (!at.settled) setAt({ step: at.step, settled: true });
    else if (at.step < acts.length) {
      acts[at.step](drawn);
      setAt({ step: at.step + 1, settled: false });
    }
    return null;
  }
  renderToStaticMarkup(createElement(Probe));
  return frames;
}

function pressProps(over: PaintOptions & { sessionKey?: string } = {}): ConversationProps {
  return {
    items: over.items ?? [],
    thinking: false,
    live: true,
    sessionKey: over.sessionKey ?? "claude#a",
    config: { effort: null, models: [], canSetModel: true, effortLevels: [],
      showEffort: false, canSetEffort: true,
      pendingModel: false, pendingEffort: false, error: null },
    approvals: {},
    activeApprovalIds: new Set<string>(),
    ...(over.context === undefined ? {} : { context: over.context }),
    ...(over.previousContext === undefined ? {} : { previousContext: over.previousContext }),
    ...(over.commands === undefined ? {} : { commands: over.commands }),
    ...(over.queueWhileBusy === undefined ? {} : { queueWhileBusy: over.queueWhileBusy }),
    ...(over.turnRestorePoints === undefined
      ? {} : { turnRestorePoints: over.turnRestorePoints }),
    ...(over.onRestoreTurn === undefined ? {} : { onRestoreTurn: over.onRestoreTurn }),
    ...(over.selectedSubagentKey === undefined
      ? {} : { selectedSubagentKey: over.selectedSubagentKey }),
    ...(over.onSelectSubagent === undefined
      ? {} : { onSelectSubagent: over.onSelectSubagent }),
    onSend: () => undefined,
    ...(over.onRunCommand === undefined ? {} : { onRunCommand: over.onRunCommand }),
    onStop: () => undefined,
    onSetModel: () => undefined, onSetEffort: () => undefined,
    onApprove: () => undefined, onDeny: () => undefined,
  } as ConversationProps;
}

/** The one on-demand detail control, as the user's pointer finds it. */
const detailButton = (tree: unknown): PressEl =>
  only(tree, el => el.type === "button"
    && el.props.className === "conv-iconbtn conv-detail");

const click = (el: PressEl): void => { (el.props.onClick as () => void)(); };

describe("Conversation — a refused send, pressed", () => {
  /* The composer empties at dispatch; a send the engine definitely refused
     hands its words back, but only into the session it was typed in, and
     never over newer typing. The echo's refusal always reaches the session the
     send came from. Driven through the pane's own Send button, with the
     caller's draft store the shell passes in production. */
  const said: ConvItem = { kind: "text", id: "t1", text: "Ready.", streaming: false };
  type Refusal = { reject: (why: Error) => void; errors: string[][] };
  const sending = (): Refusal & { promise: Promise<unknown> } => {
    let reject: (why: Error) => void = () => undefined;
    const promise = new Promise((_resolve, fail) => { reject = fail; });
    return { promise, reject, errors: [] };
  };
  const paneFor = (key: string, drafts: DraftStore, refusal: Refusal & {
    promise: Promise<unknown>;
  }): ConversationProps => ({
    ...pressProps({ sessionKey: key, items: [said] }),
    drafts,
    onSend: () => refusal.promise,
    onPendingEchoError: (id, why) => { refusal.errors.push([key, id, why]); },
  });
  const sendButton = (frame: Frame): PressEl =>
    only(frame.tree, el => el.type === "button" && el.props["aria-label"] === "Send");
  const settle = async (): Promise<void> => {
    for (let turn = 0; turn < 4; turn += 1) await Promise.resolve();
  };

  it("gives a definitely failed send its words back", async () => {
    const drafts = createDraftStore({ initial: { "claude#a": "Run the checks" } });
    const refusal = sending();
    press([() => paneFor("claude#a", drafts, refusal)], [([frame]) => click(sendButton(frame))]);
    expect(drafts.get("claude#a")).toBe("");
    refusal.reject(new Error("The engine refused it."));
    await settle();
    expect(drafts.get("claude#a")).toBe("Run the checks");
    expect(refusal.errors).toEqual([["claude#a", "0", "The engine refused it."]]);
  });

  it("never clobbers text typed while the failed send was in flight", async () => {
    const drafts = createDraftStore({ initial: { "claude#a": "Run the checks" } });
    const refusal = sending();
    press([() => paneFor("claude#a", drafts, refusal)], [([frame]) => click(sendButton(frame))]);
    drafts.set("claude#a", "a newer thought");
    refusal.reject(new Error("The engine refused it."));
    await settle();
    expect(drafts.get("claude#a")).toBe("a newer thought");
  });

  it("hands a failed send back to the session it was typed in, and no other", async () => {
    // Routes 2 and 3 keep ONE instance across a session switch. Session A's
    // refused sentence must not land in B's composer, and its echo's refusal
    // is still addressed to A.
    const drafts = createDraftStore({
      initial: { "claude#a": "Run the checks", "claude#b": "B's own words" },
    });
    const refusal = sending();
    press([step => paneFor(step === 0 ? "claude#a" : "claude#b", drafts, refusal)],
      [([frame]) => click(sendButton(frame)), () => undefined]);
    refusal.reject(new Error("The engine refused it."));
    await settle();
    expect(drafts.get("claude#b")).toBe("B's own words");
    expect(drafts.get("claude#a")).toBe("");
    expect(refusal.errors).toEqual([["claude#a", "0", "The engine refused it."]]);
  });
});

describe("Conversation — steer and stop, pressed", () => {
  const said: ConvItem = { kind: "text", id: "t1", text: "Working on it.", streaming: true };
  const busyPane = (calls: string[], over: Partial<ConversationProps> = {}): ConversationProps => ({
    ...pressProps({ sessionKey: "claude#a", items: [said] }),
    thinking: true,
    canSteer: true,
    drafts: createDraftStore({ initial: { "claude#a": "Also run the linter" } }),
    onSend: text => { calls.push(`send:${text}`); },
    onQueue: text => { calls.push(`queue:${text}`); return true; },
    onStop: () => { calls.push("stop"); },
    ...over,
  });
  const button = (frame: Frame, label: string): PressEl =>
    only(frame.tree, el => el.type === "button" && el.props["aria-label"] === label);
  const textarea = (frame: Frame): PressEl => only(frame.tree, el => el.type === "textarea");
  const key = (name: string, calls: string[]) => ({
    key: name, shiftKey: false, altKey: false, metaKey: false, ctrlKey: false,
    nativeEvent: {}, preventDefault: () => { calls.push("preventDefault"); },
    stopPropagation: () => { calls.push("stopPropagation"); },
  });

  it("delivers Steer through the send path, and Enter's control still queues", () => {
    const steered: string[] = [];
    press([() => busyPane(steered)], [([frame]) => click(button(frame, "Steer"))]);
    expect(steered).toEqual(["send:Also run the linter"]);
    const queued: string[] = [];
    press([() => busyPane(queued)],
      [([frame]) => click(button(frame, "Queue after current turn"))]);
    expect(queued).toEqual(["queue:Also run the linter"]);
  });

  it("stops a running turn on Escape, and keeps the key out of the pane's other listeners", () => {
    const calls: string[] = [];
    press([() => busyPane(calls)], [([frame]) => {
      (textarea(frame).props.onKeyDown as (event: unknown) => void)(key("Escape", calls));
    }]);
    expect(calls).toEqual(["preventDefault", "stopPropagation", "stop"]);
  });

  it("lets Escape close an open slash menu instead of stopping the turn", () => {
    const calls: string[] = [];
    press([() => busyPane(calls, {
      drafts: createDraftStore({ initial: { "claude#a": "/" } }),
      commands: [{ name: "review", description: "Review changes" }],
      onRunCommand: () => undefined,
    })], [([frame]) => {
      (textarea(frame).props.onKeyDown as (event: unknown) => void)(key("Escape", calls));
    }]);
    expect(calls).not.toContain("stop");
  });
});

describe("Conversation — the slash menu, pressed", () => {
  // The menu the launch card shares (use-provider-command-menu.ts), pressed
  // through this pane's own textarea. A row's insert moves focus a frame later.
  afterEach(() => { vi.unstubAllGlobals(); });
  const said: ConvItem = { kind: "text", id: "t1", text: "Ready.", streaming: false };
  const commands = [
    { name: "review", description: "Review changes", argumentHint: "<target>" },
    { name: "compact", description: "Compact context" },
  ];
  const menuPane = (drafts: DraftStore, calls: string[]): ConversationProps => ({
    ...pressProps({ sessionKey: "claude#a", items: [said], commands }),
    drafts,
    onRunCommand: text => { calls.push(`run:${text}`); },
    onSend: text => { calls.push(`send:${text}`); },
  });
  const key = (frame: Frame, name: string): void => {
    const area = only(frame.tree, el => el.type === "textarea");
    (area.props.onKeyDown as (event: unknown) => void)({
      key: name, shiftKey: false, altKey: false, metaKey: false, ctrlKey: false,
      nativeEvent: {}, preventDefault: () => undefined, stopPropagation: () => undefined,
    });
  };
  const menuShown = (frame: Frame): boolean =>
    has(frame.tree, el => el.type === ProviderCommandMenu);

  it("moves with the arrows and runs the active row on Enter", () => {
    const drafts = createDraftStore({ initial: { "claude#a": "/" } });
    const calls: string[] = [];
    const frames = press([() => menuPane(drafts, calls)], [
      ([frame]) => { key(frame, "ArrowDown"); },
      ([frame]) => { key(frame, "Enter"); },
    ]);
    expect(menuShown(frames[0][0])).toBe(true);
    expect(calls).toEqual(["run:/compact"]);
    expect(drafts.get("claude#a")).toBe("");
    expect(menuShown(frames[2][0])).toBe(false);
  });

  it("inserts the active row on Tab for the user to finish", () => {
    vi.stubGlobal("window", { requestAnimationFrame: () => 0 });
    const drafts = createDraftStore({ initial: { "claude#a": "/" } });
    const calls: string[] = [];
    const frames = press([() => menuPane(drafts, calls)], [([frame]) => { key(frame, "Tab"); }]);
    expect(drafts.get("claude#a")).toBe("/review ");
    expect(calls).toEqual([]);
    expect(menuShown(frames[1][0])).toBe(false);
  });

  it("closes on Escape and leaves the draft as typed", () => {
    const drafts = createDraftStore({ initial: { "claude#a": "/" } });
    const calls: string[] = [];
    const frames = press([() => menuPane(drafts, calls)], [([frame]) => { key(frame, "Escape"); }]);
    expect(menuShown(frames[1][0])).toBe(false);
    expect(drafts.get("claude#a")).toBe("/");
    expect(calls).toEqual([]);
  });
});

describe("Conversation — context meter, pressed", () => {
  it("shows provider facts and dispatches only the advertised compact command", () => {
    const run = vi.fn();
    const usage = { used: 125, size: 200, percent: 63,
      autoCompact: true, autoCompactThreshold: 180,
      categories: [{ name: "Tools", tokens: 30 }] };
    const previousContext = { used: 80, size: 200, percent: 40 };
    const frames = press([() => pressProps({
      items: [{ kind: "text", id: "answer", text: "working session", streaming: false }],
      context: usage,
      previousContext,
      commands: [{ name: "compress", description: "Compact", aliases: ["compact"] }],
      onRunCommand: run,
    })], [
      ([frame]) => click(only(frame.card, el => el.type === "button"
        && typeof el.props["aria-label"] === "string"
        && el.props["aria-label"].startsWith("Session context"))),
      ([frame]) => click(only(frame.card, el => el.type === "button"
        && el.props.children === "Compact now")),
    ], tree => into(only(tree, el => typeof el.type === "function"
      && (el.type as { name?: string }).name === "ContextMeter"
      && el.props.variant === undefined)));

    const open = renderToStaticMarkup(frames[1][0].card as ReactNode);
    expect(open).toContain("Window remaining");
    expect(open).toContain("75");
    expect(open).toContain("Since prior report");
    expect(open).toContain("+45");
    expect(open).toContain("Auto-compact threshold");
    expect(open).toContain("Provider reported");
    expect(run).toHaveBeenCalledWith("/compress");
  });
});

describe("Conversation — transcript detail, pressed", () => {
  const call: ConvItem = { kind: "tool", id: "c1", tool: "Read", summary: "read a.ts",
    input: { file_path: "/repo/a.ts" }, result: { ok: true, summary: "12 lines" } };
  // `only` with one extra state: ZERO is an answer here (see toolFold below),
  // but two cards for one call still means the probe read the wrong tree, and
  // silently reporting the first would hide that.
  const cardOpen = (frame: Frame): unknown => {
    const rows = elements(frame.card)
      .filter(el => el.props.className === "conv-row conv-tool-row");
    if (rows.length > 1) {
      throw new Error(`expected at most one card, found ${String(rows.length)}`);
    }
    return rows.length === 0 ? "folded away" : rows[0].props["aria-expanded"];
  };
  /* AT COMPACT THERE IS NO CARD. The call folds into a work run whose members
     are not rendered while it is closed, so the chain itemCard walks does not
     exist — reaching for it anyway would fail on a step that is behaving
     correctly. `null` here and "folded away" above are the same fact: the dial's
     first step removed the row rather than redrawing it. */
  const toolFold = (tree: unknown): unknown => {
    const rows = elements(tree)
      .filter(el => isComponent(el) && el.props.findKey === "l:c1");
    return rows.length === 1 ? itemCard(tree, "l:c1") : null;
  };

  it("keeps a search-forced tool group open when its summary is activated", () => {
    const calls: ConvItem[] = ["c1", "c2", "c3"].map(id => ({
      kind: "tool", id, tool: "node_repl/js", summary: "node_repl/js",
      input: { code: id }, result: { ok: true, summary: "ok" },
    }));
    const forcedRun = (tree: unknown): unknown => {
      const run = only(tree, el => isComponent(el)
        && (el.props.run as { kind?: string } | undefined)?.kind === "tool-run");
      const find = run.props.find as Record<string, unknown>;
      return componentOf(run)!({
        ...run.props,
        find: { ...find, active: true, currentKey: "l:c1", matches: new Set(["l:c1"]) },
      });
    };
    const frame = press([() => pressProps({ items: calls })], [], forcedRun)[0][0];
    const details = only(frame.card, el => el.type === "details");
    const summary = only(frame.card,
      el => el.type === "summary" && el.props.className === "conv-tool-run-summary");
    const preventDefault = vi.fn();
    (summary.props.onClick as (event: { preventDefault: () => void }) => void)(
      { preventDefault },
    );
    expect(preventDefault).toHaveBeenCalledOnce();
    const currentTarget = { open: false };
    (details.props.onToggle as (event: { currentTarget: { open: boolean } }) => void)(
      { currentTarget },
    );
    expect(currentTarget.open).toBe(true);
  });

  it("cycles through detail levels and re-seeds every fold", () => {
    withStoredDensities("{}", () => {
      const frames = press([() => pressProps({ items: [call] })], [
        f => { click(detailButton(f[0].tree)); },
        f => { click(only(f[0].card,
          el => el.props.className === "conv-row conv-tool-row")); },
        f => { click(detailButton(f[0].tree)); },
        f => { click(detailButton(f[0].tree)); },
        f => { click(detailButton(f[0].tree)); },
      ], toolFold);
      expect(frames.map(f => cardOpen(f[0]))).toEqual([
        false,          // Balanced, the shipped default
        true,           // Detailed unfolds the card
        false,          // the user shut that one card by hand, and that still wins
        // Compact folds the run itself, so this call has no card of its own to
        // be open or closed — the step below Balanced now removes a row rather
        // than repeating the row Balanced already drew.
        "folded away",
        false,          // Balanced draws the card again, folded
        true,           // Detailed expands everything again
      ]);
    });
  });

  it("keeps a card the user opened by hand until the next press, not forever", () => {
    withStoredDensities("{}", () => {
      const frames = press([() => pressProps({ items: [call] })], [
        f => { click(only(f[0].card,
          el => el.props.className === "conv-row conv-tool-row")); },
        f => { click(detailButton(f[0].tree)); },
        f => { click(detailButton(f[0].tree)); },
      ], toolFold);
      // Hand-opened, still open at Detailed, and gone at Compact — where the
      // call folds into a run instead of keeping a card at all.
      expect(frames.map(f => cardOpen(f[0]))).toEqual([false, true, true, "folded away"]);
    });
  });

  it("does not let one tile's choice erase the tile beside it", () => {
    // Two panes of one workspace, both mounted before either was touched, both
    // serialising the same shared key. The pane that writes second used to
    // write its own mount-time snapshot over the other's choice.
    withStoredDensities("{}", () => {
      press([
        () => pressProps({ items: [call], sessionKey: "claude#s1" }),
        () => pressProps({ items: [call], sessionKey: "codex#s2" }),
      ], [
        f => { click(detailButton(f[0].tree)); },
        f => { click(detailButton(f[0].tree)); },
        f => { click(detailButton(f[1].tree)); },
      ]);
      expect(parseTranscriptDensities(
        window.localStorage.getItem(TRANSCRIPT_DENSITY_KEY)))
        .toEqual({ "claude#s1": "compact", "codex#s2": "detailed" });
    });
  });

  it("still reads back the step a pane was left on", () => {
    withStoredDensities("{}", () => {
      press([() => pressProps({ items: [call] })],
        [
          f => { click(detailButton(f[0].tree)); },
          f => { click(detailButton(f[0].tree)); },
        ]);
      expect(transcriptDensityFor(parseTranscriptDensities(
        window.localStorage.getItem(TRANSCRIPT_DENSITY_KEY)), "claude#a"))
        .toBe("compact");
    });
  });
});

describe("Conversation — the restore control, pressed", () => {
  const turn: ConvItem = { kind: "user", id: "user-0", text: "delete the parser" };
  const other: ConvItem = { kind: "user", id: "user-9", text: "no, keep it" };
  const point: TurnRestorePoint = { description: "Restores 14 files. Later edits are lost." };
  const points = new Map([["l:user-0", point]]);
  const userTurn = (tree: unknown): unknown => itemCard(tree, "l:user-0");
  const restoreButton = (frame: Frame): PressEl =>
    only(frame.card, el => el.props.className === "conv-turn-restore");
  const dialog = (frame: Frame): PressEl | undefined =>
    elements(frame.tree).find(el => isComponent(el)
      && (el.type as { name?: string }).name === "ConfirmDialog");

  function restoreProps(over: { sessionKey?: string; onRestoreTurn?: (k: string) => void }) {
    return pressProps({
      items: [turn, other], turnRestorePoints: points,
      onRestoreTurn: over.onRestoreTurn ?? (() => undefined),
      ...(over.sessionKey === undefined ? {} : { sessionKey: over.sessionKey }),
    });
  }

  it("asks before it overwrites anything, and restores the turn that was asked about", () => {
    const restored: string[] = [];
    // Sampled while the question is on screen: pressing Restore must ASK, not do.
    let askedWithoutDoing = -1;
    const frames = press(
      [() => restoreProps({ onRestoreTurn: key => { restored.push(key); } })],
      [
        f => { click(restoreButton(f[0])); },
        f => {
          askedWithoutDoing = restored.length;
          (dialog(f[0])?.props.onConfirm as () => void)();
        },
      ], userTurn);

    // Nothing is asked until the control is pressed, and nothing is written
    // until the question is answered.
    expect(dialog(frames[0][0])).toBeUndefined();
    expect(askedWithoutDoing).toBe(0);

    const asking = dialog(frames[1][0]);
    expect(asking).toBeDefined();
    expect(asking?.props.open).toBe(true);
    expect(asking?.props.tone).toBe("danger");
    expect(asking?.props.title).toBe("Restore files to before this message?");
    expect(asking?.props.description).toBe(point.description);
    expect(asking?.props.confirmLabel).toBe("Restore files");

    // Confirmed: the caller is told which turn, once, and the question goes away.
    expect(restored).toEqual(["l:user-0"]);
    expect(dialog(frames[2][0])).toBeUndefined();
  });

  it("touches nothing at all when the question is answered no", () => {
    const restored: string[] = [];
    const frames = press(
      [() => restoreProps({ onRestoreTurn: key => { restored.push(key); } })],
      [
        f => { click(restoreButton(f[0])); },
        f => { (dialog(f[0])?.props.onCancel as () => void)(); },
      ], userTurn);
    expect(dialog(frames[1][0])).toBeDefined();
    expect(dialog(frames[2][0])).toBeUndefined();
    expect(restored).toEqual([]);
  });

  it("never carries a pending confirmation across to another session", () => {
    // The dialog names A's turn and stands over A's working tree. Answering it
    // while B is on screen would roll back a repository the user is no longer
    // looking at, at a turn key that means something else there.
    const restored: string[] = [];
    const frames = press(
      [s => restoreProps({
        sessionKey: s >= 2 ? "claude#b" : "claude#a",
        onRestoreTurn: key => { restored.push(key); },
      })],
      [
        f => { click(restoreButton(f[0])); },
        () => undefined,        // the user switches tiles: sessionKey changes
      ], userTurn);
    expect(dialog(frames[1][0])).toBeDefined();
    expect(dialog(frames[2][0])).toBeUndefined();
    expect(restored).toEqual([]);
  });

  it("hands the transcript no opener at all when the pane cannot restore", () => {
    // Both halves of the contract or nothing: a row that could open a dialog
    // the pane has no callback to answer is a button that does nothing.
    const frames = press([() => pressProps({ items: [turn] })], []);
    expect(itemRow(frames[0][0].tree, "l:user-0").props.onRequestRestore).toBeUndefined();
    expect(has(itemCard(frames[0][0].tree, "l:user-0"),
      el => el.props.className === "conv-turn-restore")).toBe(false);

    const wired = press([() => restoreProps({})], []);
    expect(typeof itemRow(wired[0][0].tree, "l:user-0").props.onRequestRestore)
      .toBe("function");
    expect(has(itemCard(wired[0][0].tree, "l:user-0"),
      el => el.props.className === "conv-turn-restore")).toBe(true);
  });
});

describe("Conversation — the subagent cell, pressed", () => {
  const agent = (children: ConvItem[] = []): ConvItem => ({
    kind: "subagent", id: "a1", subagentType: "reviewer", description: "audit",
    status: "running", children, toolCount: 0, subagentCount: 0,
  });
  const kid: ConvItem = { kind: "text", id: "t", text: "reading", streaming: false };

  const subCard = (tree: unknown): unknown => itemCard(tree, "l:a1");
  const row = (frame: Frame): PressEl =>
    only(frame.card, el => el.props.className === "sub-row");
  const fold = (frame: Frame): PressEl =>
    only(frame.card, el => el.props.className === "sub-fold");
  /** A click carrying the one field the row reads. detail 0 is Enter/Space. */
  const clickWith = (el: PressEl, detail: number): void => {
    (el.props.onClick as (e: { detail: number }) => void)({ detail });
  };

  it("hands the caller the agentKey, and says how the row was activated", () => {
    // The caller moves focus into what it opened for a KEYBOARD user only: a
    // pointer user is still reading the transcript the cell sits in.
    const opened: [string, boolean][] = [];
    press([() => pressProps({ items: [agent()],
      onSelectSubagent: (key, viaKeyboard) => { opened.push([key, viaKeyboard]); } })],
      [
        f => { clickWith(row(f[0]), 1); },
        f => { clickWith(row(f[0]), 0); },
      ], subCard);
    expect(opened).toEqual([["a1", false], ["a1", true]]);
  });

  it("does not unfold the transcript in place when it opened a surface instead", () => {
    // Two controls, two jobs. A row that both selected and unfolded would
    // undo the point of the small cell on every click.
    const frames = press([() => pressProps({ items: [agent([kid])],
      onSelectSubagent: () => undefined })],
      [f => { clickWith(row(f[0]), 1); }], subCard);
    expect(fold(frames[0][0]).props["aria-expanded"]).toBe(false);
    expect(fold(frames[1][0]).props["aria-expanded"]).toBe(false);
  });

  it("unfolds in place when no surface is wired, exactly as it always did", () => {
    // THE DEGRADE PATH. A build where App has not passed the callback must
    // behave the way this cell shipped, not become a row that does nothing.
    const frames = press([() => pressProps({ items: [agent([kid])] })],
      [f => { clickWith(row(f[0]), 1); }], subCard);
    expect(fold(frames[0][0]).props["aria-expanded"]).toBe(false);
    expect(fold(frames[1][0]).props["aria-expanded"]).toBe(true);
  });

  it("keeps the fold working next to a wired row", () => {
    const frames = press([() => pressProps({ items: [agent([kid])],
      onSelectSubagent: () => undefined })],
      [f => { click(fold(f[0])); }], subCard);
    expect(fold(frames[0][0]).props["aria-label"])
      .toBe("Show nested activity of reviewer");
    expect(fold(frames[1][0]).props["aria-expanded"]).toBe(true);
    expect(fold(frames[1][0]).props["aria-label"])
      .toBe("Hide nested activity of reviewer");
  });
});

import {
  absolutePathsIn, displayPath, homeRelativePath, shortenSummaryPaths,
} from "./display-paths";
import {
  commandRunFailureLabel, diffPathsByToolRow, fileChangeTargetText, fileChangeVerbWord,
  fileChangeVerbs, findSubagentItem, groupFinishedTools, subagentRowDescription,
  summaryRelistsDiffPaths, TOOL_NAME_CACHE_LIMIT, toolActivityGroupKey, toolActivityGroupName,
  toolActivityKind, toolActivityName, toolRowSummaryText, toolSummaryRepeatsName,
} from "./tool-activity";
import type { ConvItem as ConvItemType } from "./conversation-reducer";
import type { TranscriptRow as TranscriptRowType } from "./transcript-merge";

const REPO = "/Users/exampleuser/Desktop/Projects/Interface";

describe("displayPath", () => {
  it("shows a path inside the session root relative to it", () => {
    // The screenshot case: 69 characters of which the first 45 were identical
    // boilerplate on every one of four stacked rows.
    const abs = `${REPO}/src/main/bridge.test.ts`;
    expect(abs).toHaveLength(69);
    expect(displayPath(abs, REPO)).toBe("src/main/bridge.test.ts");
  });

  it("names the directory rather than printing nothing when the path IS the root", () => {
    expect(displayPath(REPO, REPO)).toBe("Interface");
    expect(displayPath(`${REPO}/`, REPO)).toBe("Interface");
  });

  it("tolerates a trailing separator on the root", () => {
    expect(displayPath(`${REPO}/src/App.tsx`, `${REPO}/`)).toBe("src/App.tsx");
    expect(displayPath(`${REPO}/src/App.tsx`, `${REPO}///`)).toBe("src/App.tsx");
  });

  it("requires a separator boundary, so a longer sibling root is never rewritten", () => {
    // A bare startsWith turns this into "X/a.ts" — a path that never existed.
    // It falls through to the home rule instead, which keeps every segment.
    const sibling = `${REPO}X/a.ts`;
    expect(displayPath(sibling, REPO)).toBe("~/Desktop/Projects/InterfaceX/a.ts");
    expect(displayPath(sibling, REPO)).not.toBe("X/a.ts");
    // With no home to fall back to either, it stays exactly as it arrived.
    expect(displayPath("/srv/appX/a.ts", "/srv/app")).toBe("/srv/appX/a.ts");
  });

  it("leaves every path alone when no root is supplied", () => {
    const abs = `${REPO}/src/main/bridge.test.ts`;
    expect(displayPath(abs)).toBe(abs);
    expect(displayPath(abs, "")).toBe(abs);
    expect(displayPath(abs, "   ")).toBe(abs);
  });

  it("falls back to ~ for a path outside the root but under the same home", () => {
    // The home directory is derived from the ROOT, never from an environment
    // the renderer does not have.
    expect(displayPath("/Users/exampleuser/Desktop/Projects/Other/x.ts", REPO))
      .toBe("~/Desktop/Projects/Other/x.ts");
    expect(displayPath("/home/ann/work/x.ts", "/home/ann/work/repo"))
      .toBe("~/work/x.ts");
  });

  it("keeps a path that is under neither the root nor the home absolute", () => {
    expect(displayPath("/opt/homebrew/bin/node", REPO)).toBe("/opt/homebrew/bin/node");
    expect(displayPath("/Users/other/notes.md", REPO)).toBe("/Users/other/notes.md");
  });

  it("handles a Windows path and root, including its own separator", () => {
    const root = "C:\\Users\\ann\\Projects\\Interface";
    expect(displayPath(`${root}\\src\\main\\bridge.ts`, root)).toBe("src\\main\\bridge.ts");
    expect(displayPath(root, root)).toBe("Interface");
    expect(displayPath("C:\\Users\\ann\\Desktop\\note.txt", root))
      .toBe("~\\Desktop\\note.txt");
    // A Windows path against a POSIX root shares no prefix and is left alone.
    expect(displayPath(`${root}\\src\\main\\bridge.ts`, REPO))
      .toBe(`${root}\\src\\main\\bridge.ts`);
  });

  it("never throws on input that is not an absolute path", () => {
    expect(displayPath("", REPO)).toBe("");
    expect(displayPath("src/App.tsx", REPO)).toBe("src/App.tsx");
    expect(displayPath("~/already", REPO)).toBe("~/already");
  });
});

describe("homeRelativePath", () => {
  it("writes a path inside a home the way a terminal prints it", () => {
    expect(homeRelativePath(REPO)).toBe("~/Desktop/Projects/Interface");
    expect(homeRelativePath("/home/ann/work/repo")).toBe("~/work/repo");
    expect(homeRelativePath("C:\\Users\\ann\\Projects")).toBe("~\\Projects");
  });

  it("says ~ when the path IS the home directory, trailing separator or not", () => {
    expect(homeRelativePath("/Users/exampleuser")).toBe("~");
    // A cwd arrives with or without its slash depending on who set it, and
    // "~/" is not how a terminal writes the home directory.
    expect(homeRelativePath("/Users/exampleuser/")).toBe("~");
    expect(homeRelativePath(`${REPO}/`)).toBe("~/Desktop/Projects/Interface");
    expect(homeRelativePath("C:\\Users\\ann\\")).toBe("~");
  });

  it("leaves a path with no home alone, and never throws on one that is not a path", () => {
    expect(homeRelativePath("/opt/homebrew/bin/node")).toBe("/opt/homebrew/bin/node");
    expect(homeRelativePath("src/App.tsx")).toBe("src/App.tsx");
    expect(homeRelativePath("")).toBe("");
    // The trailing separator goes for EVERY path, not only the ones under a
    // home: the same cwd printed "~/repo" one way and "/opt/work/" the other.
    expect(homeRelativePath("/opt/work/")).toBe("/opt/work");
    expect(homeRelativePath("D:\\build\\")).toBe("D:\\build");
    // …but "/" is a path, and stripping it to "" would be a lie about which.
    expect(homeRelativePath("/")).toBe("/");
  });
});

describe("shortenSummaryPaths", () => {
  it("rewrites every absolute path in a summary and keeps the prose around them", () => {
    const summary = `update ${REPO}/src/main/bridge.ts, add ${REPO}/src/x.ts`;
    expect(shortenSummaryPaths(summary, REPO))
      .toBe("update src/main/bridge.ts, add src/x.ts");
  });

  it("still shortens a list the engine already cut mid-word", () => {
    // mapper.ts slices a summary at 120 chars with no ellipsis, so the LAST
    // path arrives as a fragment. The fragment is still under the root.
    const full = `update ${REPO}/src/engines/claude/adapter.test.ts, update ${REPO}/src/main/bridge.test.ts`;
    const cut = full.slice(0, 120);
    expect(cut.endsWith("Deskt")).toBe(true);
    expect(shortenSummaryPaths(cut, REPO))
      .toBe("update src/engines/claude/adapter.test.ts, update ~/Deskt");
  });

  it("leaves prose that merely contains a slash untouched", () => {
    const prose = "read 3 of 4 files; see docs/setup and n/a";
    expect(shortenSummaryPaths(prose, REPO)).toBe(prose);
  });

  it("is a no-op without a root", () => {
    const summary = `update ${REPO}/src/x.ts`;
    expect(shortenSummaryPaths(summary)).toBe(summary);
    expect(shortenSummaryPaths(summary, "")).toBe(summary);
  });

  it("finds paths in the order they appear", () => {
    expect(absolutePathsIn("update /a/b.ts, add /c/d.ts")).toEqual(["/a/b.ts", "/c/d.ts"]);
    expect(absolutePathsIn("nothing absolute here")).toEqual([]);
  });
});

describe("toolSummaryRepeatsName", () => {
  it("suppresses a summary that is the tool's own name again", () => {
    // Codex returns toolName() as an mcpToolCall's summary, so the row printed
    // "node_repl/js node_repl/js".
    expect(toolSummaryRepeatsName("node_repl/js", "node_repl/js")).toBe(true);
  });

  it("sees through the spellings one name gets in two places", () => {
    expect(toolSummaryRepeatsName("fileChange", "file change")).toBe(true);
    expect(toolSummaryRepeatsName("node_repl/js", "Node Repl JS")).toBe(true);
    expect(toolSummaryRepeatsName("apply_patch", "apply-patch")).toBe(true);
  });

  it("keeps a summary that merely STARTS with the tool name", () => {
    // A prefix rule would delete the most informative field on the row.
    expect(toolSummaryRepeatsName("Bash", "bash -lc 'npm run build'")).toBe(false);
    expect(toolSummaryRepeatsName("Read", "src/x.ts")).toBe(false);
    expect(toolSummaryRepeatsName("fileChange", "file change and more")).toBe(false);
  });

  it("treats a blank summary as nothing to print", () => {
    expect(toolSummaryRepeatsName("Read", "")).toBe(true);
    expect(toolSummaryRepeatsName("Read", "   ")).toBe(true);
  });
});

describe("Claude-style tool activity labels", () => {
  it("names individual MCP rows by method and their group by server", () => {
    expect(toolActivityName("node_repl/js")).toBe("JavaScript");
    expect(toolActivityName("mcp__slack__search_messages")).toBe("Search messages");
    expect(toolActivityGroupName("node_repl/js")).toBe("Node REPL");
    expect(toolActivityGroupName("mcp__slack__search_messages")).toBe("Slack");
  });

  it("turns ordinary provider identifiers into sentence-ready labels", () => {
    expect(toolActivityName("read_file")).toBe("Read file");
    expect(toolActivityName("fileChange")).toBe("File change");
    expect(toolActivityName("js")).toBe("JavaScript");
  });

  it("groups methods on one MCP server without merging different servers", () => {
    expect(toolActivityGroupKey("node_repl/js"))
      .toBe(toolActivityGroupKey("node_repl/screenshot"));
    expect(toolActivityGroupKey("mcp__node_repl__js"))
      .toBe(toolActivityGroupKey("node_repl/python"));
    expect(toolActivityGroupKey("node_repl/js"))
      .not.toBe(toolActivityGroupKey("slack/search"));
  });

  it("answers the same from its name caches, including past their cap", () => {
    // Every tool row classifies its name several times a frame, so the name
    // functions remember their answers. The cap restarts a cache rather than
    // letting open-ended MCP names grow it forever.
    const names = ["Bash", "read_file", "fileChange", "apply_patch", "mcp__slack__search",
      "node_repl/js", "WebSearch", "  Grep  ", "list_dir", "Task"];
    const answer = (name: string) => [toolActivityKind(name), toolActivityGroupKey(name),
      toolActivityName(name), toolActivityGroupName(name)];
    const first = names.map(answer);
    expect(first.map(entry => entry[0])).toEqual(["command", "read", "edit", "edit", "other",
      "other", "search", "search", "list", "other"]);
    expect(names.map(answer)).toEqual(first);
    for (let index = 0; index <= TOOL_NAME_CACHE_LIMIT * 2; index += 1) {
      answer(`mcp__server${String(index)}__method`);
    }
    expect(names.map(answer)).toEqual(first);
  });
});

describe("toolRowSummaryText", () => {
  const fileChange = (paths: string[]): { tool: string; summary: string } => ({
    tool: "fileChange",
    summary: paths.map(p => `update ${p}`).join(", ").slice(0, 120),
  });
  const four = [
    `${REPO}/src/engines/claude/adapter.test.ts`,
    `${REPO}/src/main/bridge.test.ts`,
    `${REPO}/src/main/registry.test.ts`,
    `${REPO}/src/renderer/conversation.test.ts`,
  ];

  it("collapses a re-listing of the files its own diff rows name to a count", () => {
    expect(toolRowSummaryText(fileChange(four), { root: REPO, diffPaths: four }))
      .toBe("4 files");
  });

  it("counts DISTINCT files, so a file edited five times is not five files", () => {
    const repeated = [four[0], four[0], four[0], four[1], four[1]];
    expect(toolRowSummaryText(fileChange([four[0], four[1]]),
      { root: REPO, diffPaths: repeated })).toBe("2 files");
  });

  it("keeps the list when the diff rows name different files", () => {
    expect(toolRowSummaryText({ tool: "fileChange", summary: `update ${REPO}/a.ts, update ${REPO}/b.ts` },
      { root: REPO, diffPaths: ["/elsewhere/a.ts", "/elsewhere/b.ts"] }))
      .toBe("update a.ts, update b.ts");
  });

  it("keeps the list when only one diff row follows", () => {
    expect(toolRowSummaryText(fileChange([four[0]]), { root: REPO, diffPaths: [four[0]] }))
      .toBe("update src/engines/claude/adapter.test.ts");
  });

  it("prints nothing for a summary that repeats the tool name", () => {
    expect(toolRowSummaryText({ tool: "node_repl/js", summary: "node_repl/js" }))
      .toBe("");
  });

  it("shortens paths for every ordinary summary", () => {
    expect(toolRowSummaryText({ tool: "Read", summary: `${REPO}/package.json` },
      { root: REPO })).toBe("package.json");
    expect(toolRowSummaryText({ tool: "Read", summary: `${REPO}/package.json` }))
      .toBe(`${REPO}/package.json`);
  });
});

describe("subagentRowDescription", () => {
  it("prints nothing for a description that is the type again", () => {
    // The plain case, and the same whole-string test toolSummaryRepeatsName
    // makes: a Claude spawn with no description of its own falls back to the
    // type at the mapper.
    expect(subagentRowDescription("code-reviewer", "code-reviewer")).toBe("");
    expect(subagentRowDescription("code-reviewer", "Code Reviewer")).toBe("");
    expect(subagentRowDescription("code_reviewer", "code-reviewer")).toBe("");
    expect(subagentRowDescription("reviewer", "  ")).toBe("");
  });

  it("prints nothing when the description is that name behind a namespace", () => {
    // THE PHOTOGRAPHED CASE. Codex's MultiAgentV2 mapper sets description to
    // the agent path and the type to its last segment, so this fires on every
    // such cell — and the root here is the user's repo, which is why
    // displayPath cannot shorten /root/… and whole-string equality alone
    // leaves the duplicate on screen.
    expect(subagentRowDescription("interface_ui_code_audit",
      "/root/interface_ui_code_audit", "/Users/x/Projects/Interface")).toBe("");
    expect(subagentRowDescription("reviewer", "/root/reviewer")).toBe("");
    expect(subagentRowDescription("reviewer", "agents\\reviewer")).toBe("");
  });

  it("keeps prose that merely CONTAINS the type, because prose informs", () => {
    // The line this rule does not cross: never a prefix or substring test on
    // prose. A description that starts with the type still says something the
    // type did not.
    expect(subagentRowDescription("reviewer", "reviewer for the narrow layout"))
      .toBe("reviewer for the narrow layout");
    expect(subagentRowDescription("general-purpose", "Review Task 1 (spec + quality)"))
      .toBe("Review Task 1 (spec + quality)");
    // Path-shaped is separator AND no whitespace: this is a sentence.
    expect(subagentRowDescription("reviewer", "audit /a/reviewer"))
      .toBe("audit /a/reviewer");
  });

  it("shortens the paths it does keep, against the session root", () => {
    expect(subagentRowDescription("reviewer", `${REPO}/src/renderer/theme.css`, REPO))
      .toBe("src/renderer/theme.css");
    expect(subagentRowDescription("reviewer", `read ${REPO}/a.ts and stop`, REPO))
      .toBe("read a.ts and stop");
  });
});

describe("findSubagentItem", () => {
  const leaf = (id: string, children: ConvItem[] = []): ConvItem => ({
    kind: "subagent", id, subagentType: id, description: id, status: "done",
    children, toolCount: 0, subagentCount: 0,
  });

  it("finds a nested agent by key alone, at any depth", () => {
    // A detail surface holds the key it was handed, not the path it was found
    // at, and the whole tree is re-reduced on every event.
    const tree: ConvItem[] = [
      { kind: "text", id: "t", text: "hi", streaming: false },
      leaf("a1", [leaf("a2", [leaf("a3")])]),
    ];
    expect(findSubagentItem(tree, "a3")?.id).toBe("a3");
    expect(findSubagentItem(tree, "a1")?.id).toBe("a1");
  });

  it("says so when the key is gone, which is how a stale selection is closed", () => {
    expect(findSubagentItem([leaf("a1")], "a9")).toBeUndefined();
    expect(findSubagentItem([], "a1")).toBeUndefined();
  });
});

describe("fileChangeVerbs", () => {
  const REPO2 = "/Users/ann/Projects/Interface";

  it("reads the kind out of the one field that carries it", () => {
    const paths = [`${REPO2}/a.ts`, `${REPO2}/b.ts`];
    expect([...fileChangeVerbs(
      { tool: "fileChange", summary: `update ${paths[0]}, delete ${paths[1]}` },
      paths).entries()])
      .toEqual([[paths[0], "update"], [paths[1], "delete"]]);
  });

  it("resolves the entry the engine cut mid-word when only one file can be it", () => {
    // changeList is clipped, so the LAST entry can be a fragment. Dropping it
    // made an all-delete patch of three files read "Changed 3 files" — the
    // truncation manufacturing an unknown over the one kind that most needs
    // saying. A cut path that prefixes exactly one unclaimed diff path
    // identifies it; it is not a guess.
    const paths = [`${REPO2}/a.ts`, `${REPO2}/b.ts`];
    const verbs = fileChangeVerbs(
      { tool: "fileChange", summary: `update ${paths[0]}, delete ${REPO2}/b.t` }, paths);
    expect(verbs.get(paths[0])).toBe("update");
    expect(verbs.get(paths[1])).toBe("delete");
  });

  it("resolves a fragment carrying the real clip marker, not a hand-written one", () => {
    // The regression this pins: the summary carries the marker the Codex mapper
    // actually writes, so the fragment ends in "…". Prefix-matching that against
    // a path matches nothing, and an all-delete patch silently became "Changed".
    // The clip rule is Rust-owned — interface_contracts::retention::clip_label,
    // crates/interface-contracts/src/retention.rs — and charges the ellipsis to
    // the 120-unit budget rather than adding it on top, which is what the head
    // slice below reproduces.
    const paths = [`${REPO2}/alpha.ts`, `${REPO2}/bravo.ts`, `${REPO2}/charlie.ts`];
    const summary = `${paths.map(p => `delete ${p}`).join(", ").slice(0, 119)}…`;
    expect(summary).toHaveLength(120);
    expect(summary.endsWith("…")).toBe(true);
    const verbs = fileChangeVerbs({ tool: "fileChange", summary }, paths);
    expect([...verbs.values()]).toEqual(["delete", "delete", "delete"]);
  });

  it("refuses a fragment the clip cut back to the marker itself", () => {
    // Nothing survives of the path, so a prefix of "" would match every
    // candidate. One is still a coin toss dressed as an identification.
    const paths = [`${REPO2}/only.ts`];
    const verbs = fileChangeVerbs({ tool: "fileChange", summary: "delete …" }, paths);
    expect(verbs.get(paths[0])).toBe("unknown");
  });

  it("still refuses the fragment when two files could be the one that was cut", () => {
    // Two candidates is a coin toss, and the row would print a deletion that
    // may have been an update.
    const paths = [`${REPO2}/beta.ts`, `${REPO2}/bench.ts`];
    const verbs = fileChangeVerbs(
      { tool: "fileChange", summary: `delete ${REPO2}/be` }, paths);
    expect(verbs.get(paths[0])).toBe("unknown");
    expect(verbs.get(paths[1])).toBe("unknown");
  });

  it("falls back on the tool name where the engine sends no kind at all", () => {
    // Claude's summary is the bare file_path. Edit is an update by definition;
    // Write cannot tell a create from an overwrite and must not claim either.
    expect(fileChangeVerbs({ tool: "Edit", summary: `${REPO2}/a.ts` },
      [`${REPO2}/a.ts`]).get(`${REPO2}/a.ts`)).toBe("update");
    expect(fileChangeVerbs({ tool: "MultiEdit", summary: `${REPO2}/a.ts` },
      [`${REPO2}/a.ts`]).get(`${REPO2}/a.ts`)).toBe("update");
    expect(fileChangeVerbs({ tool: "Write", summary: `${REPO2}/a.ts` },
      [`${REPO2}/a.ts`]).get(`${REPO2}/a.ts`)).toBe("unknown");
  });

  it("will not read ACP's lowercase edit as an update — it also creates files", () => {
    // Grok/ACP names the tool from the ACP kind, and that kind covers creating
    // the file. Normalising it together with Claude's "Edit" printed "Updated"
    // over a file the agent had just made.
    expect(fileChangeVerbs({ tool: "edit", summary: "Edit file" },
      [`${REPO2}/made.ts`]).get(`${REPO2}/made.ts`)).toBe("unknown");
  });
});

describe("fileChangeVerbWord", () => {
  it("names the one kind a patch took, in the tense the row is in", () => {
    expect(fileChangeVerbWord(["update", "update"], "ok")).toBe("Updated");
    expect(fileChangeVerbWord(["update"], "running")).toBe("Updating");
    expect(fileChangeVerbWord(["add"], "ok")).toBe("Added");
    expect(fileChangeVerbWord(["delete"], "ok")).toBe("Deleted");
    expect(fileChangeVerbWord(["delete"], "running")).toBe("Deleting");
  });

  it("drops the past tense for a patch that did not happen", () => {
    // "Deleted src/old.ts declined" says the file both went and stayed, in four
    // words. The bare form is the only one that is true beside a refusal, and a
    // failure and a decline are refusals alike.
    expect(fileChangeVerbWord(["delete"], "refused")).toBe("Delete");
    expect(fileChangeVerbWord(["update"], "refused")).toBe("Update");
    expect(fileChangeVerbWord(["add"], "refused")).toBe("Add");
    expect(fileChangeVerbWord(["unknown"], "refused")).toBe("Change");
  });

  it("prints the update rather than implying it", () => {
    // The repo drops words that NEVER vary. This one varies across
    // add/update/delete, so marking only the rare kinds would leave a delete
    // recognisable by the ABSENCE of a mark.
    expect(fileChangeVerbWord(["update"], "ok")).not.toBe("");
  });

  it("goes neutral for a mixed patch or a kind it could not read", () => {
    expect(fileChangeVerbWord(["update", "delete"], "ok")).toBe("Changed");
    expect(fileChangeVerbWord(["unknown"], "ok")).toBe("Changed");
    expect(fileChangeVerbWord([], "ok")).toBe("Changed");
    expect(fileChangeVerbWord(["add", "unknown"], "running")).toBe("Changing");
  });
});

describe("fileChangeTargetText", () => {
  it("leaves a one-file change to print its own filename", () => {
    expect(fileChangeTargetText(["update"])).toBe("");
    expect(fileChangeTargetText([])).toBe("");
  });

  it("counts the files while the verb beside it still says what happened", () => {
    expect(fileChangeTargetText(["delete", "delete", "delete"])).toBe("3 files");
    expect(fileChangeTargetText(["update", "update"])).toBe("2 files");
  });

  it("spells the breakdown out when the verb has to go neutral", () => {
    // "Changed 2 files" is the one shape in which a deletion is invisible
    // without hovering, and it is the shape in the screenshot this came from.
    expect(fileChangeTargetText(["update", "delete"])).toBe("1 deleted, 1 updated");
    expect(fileChangeTargetText(["add", "add", "delete"])).toBe("1 deleted, 2 added");
  });

  it("puts the destructive count first, because the tail is what is cut", () => {
    // This slot ellipsises at the tail and gets about eleven characters at the
    // 320px pane. The half that survives has to be the half that cannot be
    // undone.
    expect(fileChangeTargetText(["update", "update", "delete"]))
      .toMatch(/^1 deleted/u);
  });

  it("counts files rather than listing part of them when a kind is unreadable", () => {
    // A breakdown that omits the unknowns prints counts that do not add up to
    // the number of files it is describing.
    expect(fileChangeTargetText(["delete", "unknown"])).toBe("2 files");
  });
});

describe("summaryRelistsDiffPaths", () => {
  it("needs at least two paths on BOTH sides", () => {
    expect(summaryRelistsDiffPaths("update /a.ts, update /b.ts", ["/a.ts"])).toBe(false);
    expect(summaryRelistsDiffPaths("update /a.ts", ["/a.ts", "/b.ts"])).toBe(false);
    expect(summaryRelistsDiffPaths("four files", ["/a.ts", "/b.ts"])).toBe(false);
  });

  it("matches on any whole path, because the engine truncates the last one", () => {
    expect(summaryRelistsDiffPaths("update /a.ts, update /b.t", ["/a.ts", "/b.ts"]))
      .toBe(true);
  });
});

describe("diffPathsByToolRow", () => {
  const tool = (id: string): ConvItemType =>
    ({ kind: "tool", id, tool: "fileChange", summary: "update", input: {} });
  const diff = (id: string, path: string): ConvItemType =>
    ({ kind: "diff", id, callId: "t1", path });
  const row = (key: string, item: ConvItemType): TranscriptRowType =>
    ({ kind: "item", key, origin: "live", item });

  it("attributes the diff rows drawn directly under a call to that call", () => {
    const rows = [
      row("a", tool("t1")),
      row("b", diff("d1", "/x.ts")),
      row("c", diff("d2", "/y.ts")),
      row("d", tool("t2")),
    ];
    const map = diffPathsByToolRow(rows);
    expect(map.get("a")).toEqual(["/x.ts", "/y.ts"]);
    expect(map.has("d")).toBe(false);
  });

  it("stops at anything that is not a diff row", () => {
    const rows = [
      row("a", tool("t1")),
      row("b", { kind: "text", id: "x", text: "hi", streaming: false }),
      row("c", diff("d1", "/x.ts")),
    ];
    expect(diffPathsByToolRow(rows).has("a")).toBe(false);
  });
});

describe("commandRunFailureLabel", () => {
  it("says nothing when every command passed", () => {
    expect(commandRunFailureLabel(0, 3)).toBe("");
    expect(commandRunFailureLabel(0, 1)).toBe("");
  });

  it("says which, and how many, when they did not", () => {
    expect(commandRunFailureLabel(1, 1)).toBe("failed");
    expect(commandRunFailureLabel(1, 3)).toBe("1 failed");
    expect(commandRunFailureLabel(3, 3)).toBe("3 failed");
  });
});

describe("groupFinishedTools", () => {
  const tool = (id: string, ok?: boolean) => ({
    item: {
      kind: "tool" as const, id, tool: "Read", summary: id,
      input: {},
      ...(ok === undefined ? {} : { result: { ok, summary: "" } }),
    },
  });
  const text = (id: string) => ({
    item: { kind: "text" as const, id, text: id, streaming: false },
  });
  const diff = (id: string) => ({
    item: { kind: "diff" as const, id, path: `/repo/${id}.ts`, callId: "a" },
  });

  it("groups a stretch of finished, successful calls", () => {
    const runs = groupFinishedTools([tool("a", true), tool("b", true), tool("c", true)]);
    expect(runs).toHaveLength(1);
    expect(runs[0].grouped).toBe(true);
    expect(runs[0].members.map(row => row.item.id)).toEqual(["a", "b", "c"]);
  });

  it("keeps a diff inside the stretch that produced it", () => {
    // A diff has no result of its own — the call beside it carries the verdict
    // for both — so reading "no passing result" as "do not fold" would split
    // every edit away from the change it made.
    const runs = groupFinishedTools([tool("a", true), diff("d"), tool("b", true)]);
    expect(runs).toHaveLength(1);
    expect(runs[0].grouped).toBe(true);
    expect(runs[0].members.map(row => row.item.id)).toEqual(["a", "d", "b"]);
  });

  it("keeps a diff that ends the stretch inside it", () => {
    // The trailing case of the same rule: the last thing an edit turn emits is
    // usually the diff, and a stretch that stopped one row short of it would
    // strand every turn's final change outside the run that made it.
    const runs = groupFinishedTools([tool("a", true), tool("b", true), diff("d")]);
    expect(runs).toHaveLength(1);
    expect(runs[0].grouped).toBe(true);
    expect(runs[0].members.map(row => row.item.id)).toEqual(["a", "b", "d"]);
  });

  it("never folds a call that is still running", () => {
    const runs = groupFinishedTools([tool("a", true), tool("b"), tool("c", true)]);
    expect(runs.map(run => run.grouped)).toEqual([false, false, false]);
    expect(runs.map(run => run.members.map(row => row.item.id)))
      .toEqual([["a"], ["b"], ["c"]]);
  });

  it("never folds a failure, and does not fold its neighbours into one line", () => {
    const runs = groupFinishedTools([tool("a", true), tool("b", false), tool("c", true)]);
    expect(runs.map(run => run.grouped)).toEqual([false, false, false]);
  });

  it("splits a stretch around the one bad call, keeping the good runs", () => {
    const runs = groupFinishedTools([
      tool("a", true), tool("b", true), tool("x", false), tool("c", true), tool("d", true),
    ]);
    expect(runs.map(run => run.grouped)).toEqual([true, false, true]);
    expect(runs[0].members.map(row => row.item.id)).toEqual(["a", "b"]);
    expect(runs[1].members.map(row => row.item.id)).toEqual(["x"]);
    expect(runs[2].members.map(row => row.item.id)).toEqual(["c", "d"]);
  });

  it("passes everything that is not a tool straight through", () => {
    const runs = groupFinishedTools([text("t1"), tool("a", true), tool("b", true)]);
    expect(runs.map(run => run.grouped)).toEqual([false, true]);
    expect(runs[0].members.map(row => row.item.id)).toEqual(["t1"]);
  });

  it("does not group a lone finished call — one row is not a stretch", () => {
    const runs = groupFinishedTools([tool("a", true)]);
    expect(runs.map(run => run.grouped)).toEqual([false]);
  });

  it("is total on an empty list", () => {
    expect(groupFinishedTools([])).toEqual([]);
  });
});

describe("Conversation — a streaming arrival is painted in, never popped", () => {
  it("paints what was already shown on the commit that brings more", () => {
    // renderToStaticMarkup runs no effects, so the animation frame cannot fire
    // here at all: the arrival commit sets the reveal's speed and the frames
    // after it paint the new text in. Nothing of the arrival lands whole.
    const settled = "a".repeat(200);
    const grown = settled + "b".repeat(400);
    const frames = press(
      [step => pressProps({ items: [
        { kind: "text", id: "answer", text: step === 0 ? settled : grown,
          streaming: true }] })],
      [() => undefined],
      tree => itemCard(tree, "l:answer"));
    const painted = (frame: Frame): number => {
      const markdown = only(frame.card, el => isComponent(el));
      return (markdown.props.text as string).length;
    };
    expect(painted(frames[0][0])).toBe(settled.length);
    expect(painted(frames[1][0])).toBe(settled.length);
  });
});
