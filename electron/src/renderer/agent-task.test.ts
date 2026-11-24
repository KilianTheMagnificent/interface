import { describe, expect, it } from "vitest";
import type { EngineEvent, SessionStatus } from "../shared/engine";
import { agentProgressFromItems, touchedFileStats } from "./agent-task";
import { conversationItems, latestPlanEntries } from "./conversation-reducer";
import { sessionDiffStat } from "./diff-stats";

/** The panel for a live event log: reduce it, then read the items. */
const agentProgress = (events: readonly EngineEvent[], status: SessionStatus) =>
  agentProgressFromItems(conversationItems([...events]), latestPlanEntries(events), status);
const call = (callId: string, tool: string, summary: string, agentKey?: string): EngineEvent =>
  ({ kind: "tool.call", callId, tool, summary, input: {}, ...(agentKey ? { agentKey } : {}) });
const result = (callId: string, ok: boolean): EngineEvent =>
  ({ kind: "tool.result", callId, ok, summary: "" });
const diff = (callId: string, path: string): EngineEvent =>
  ({ kind: "diff.produced", callId, path, before: "", after: "x" });

describe("agentProgress", () => {
  it("folds consecutive calls of one tool into a counted row", () => {
    const { steps } = agentProgress([
      call("1", "Read", "src/a.ts"), result("1", true),
      call("2", "Read", "src/b.ts"), result("2", true),
      call("3", "Read", "src/c.ts"), result("3", true),
      call("4", "Bash", "npm test"), result("4", true),
    ], "idle");
    expect(steps.map(s => [s.label, s.count, s.state])).toEqual([
      ["Read 3 files", 3, "ok"], ["npm test", 1, "ok"],
    ]);
  });

  it("marks a failed call and keeps a failure visible through a fold", () => {
    const { steps } = agentProgress([
      call("1", "Edit", "a.ts"), result("1", true),
      call("2", "Edit", "b.ts"), result("2", false),
    ], "idle");
    expect(steps).toHaveLength(1);
    expect(steps[0]).toMatchObject({ label: "Edited 2 files", state: "failed" });
  });

  it("shows an unresolved call as running on a live session and ended on a stopped one", () => {
    const events = [call("1", "Bash", "npm run build")];
    expect(agentProgress(events, "working").steps[0]?.state).toBe("running");
    expect(agentProgress(events, "idle").steps[0]?.state).toBe("ended");
  });

  it("collapses a subagent into one row named by its description", () => {
    const { steps } = agentProgress([
      call("t1", "Agent", "Task"),
      { kind: "subagent.started", agentKey: "t1", subagentType: "Explore",
        description: "Find the config loader" },
      call("t2", "Read", "x.ts", "t1"), result("t2", true),
      { kind: "subagent.finished", agentKey: "t1", ok: true, summary: "found it" },
      result("t1", true),
    ], "idle");
    expect(steps).toEqual([{ key: "t1", tool: "Agent", label: "Find the config loader",
      count: 1, state: "ok" }]);
  });

  it("lists touched files newest first without repeats, and the latest plan", () => {
    const out = agentProgress([
      { kind: "plan.updated", entries: [{ content: "old", priority: "medium", status: "pending" }] },
      diff("1", "a.ts"), diff("2", "b.ts"), diff("3", "a.ts"),
      { kind: "plan.updated", entries: [{ content: "new", priority: "medium", status: "in_progress" }] },
    ], "idle");
    expect(out.files).toEqual(["a.ts", "b.ts"]);
    expect(out.plan?.map(e => e.content)).toEqual(["new"]);
  });
});

describe("agentProgressFromItems", () => {
  // What a session the app did not start actually has: a HistoryView's items and
  // no events at all. Progress reads the same list the transcript does, so the
  // panel and the conversation under it can never describe different work.
  it("reads steps, files and per-file counts straight from history items", () => {
    const out = agentProgressFromItems([
      { kind: "tool", id: "c1", tool: "Bash", summary: "npm test", input: {},
        result: { ok: true, summary: "" } },
      { kind: "subagent", id: "t1", subagentType: "Explore",
        description: "Find the config loader", status: "done", children: [],
        toolCount: 0, subagentCount: 0 },
      { kind: "diff", id: "d1", path: "a.ts", callId: "c2", added: 3, removed: 1 },
      { kind: "diff", id: "d2", path: "b.ts", callId: "c3", added: 5, removed: 0 },
      { kind: "diff", id: "d3", path: "a.ts", callId: "c4", added: 2, removed: 2 },
    ], undefined, "idle");
    expect(out.steps).toEqual([
      { key: "c1", tool: "Bash", label: "npm test", count: 1, state: "ok" },
      { key: "t1", tool: "Agent", label: "Find the config loader", count: 1, state: "ok" },
    ]);
    // a.ts was touched last, so it heads the list — and both of its diffs are
    // summed, the way touchedFileStats sums a path's every event.
    expect(out.files).toEqual(["a.ts", "b.ts"]);
    expect(out.fileStats).toEqual({
      "a.ts": { added: 5, removed: 3 }, "b.ts": { added: 5, removed: 0 },
    });
  });

  it("does not list a file whose only edit was declined or failed", () => {
    const out = agentProgressFromItems([
      { kind: "tool", id: "c1", tool: "Edit", summary: "a.ts", input: {},
        result: { ok: false, summary: "declined" } },
      { kind: "diff", id: "d1", path: "a.ts", callId: "c1", added: 3, removed: 1 },
      { kind: "subagent", id: "t1", subagentType: "builder", description: "build",
        status: "done", toolCount: 1, subagentCount: 0, children: [
          { kind: "tool", id: "c2", tool: "Edit", summary: "b.ts", input: {},
            result: { ok: false, summary: "old_string not found" } },
          { kind: "diff", id: "d2", path: "b.ts", callId: "c2", added: 1, removed: 1 },
        ] },
      { kind: "diff", id: "d3", path: "c.ts", callId: "c3", added: 2, removed: 0 },
    ], undefined, "idle");
    expect(out.files).toEqual(["c.ts"]);
    expect(out.fileStats).toEqual({ "c.ts": { added: 2, removed: 0 } });
  });
});

describe("touchedFileStats", () => {
  const edit = (path: string, before: string, after: string): EngineEvent =>
    ({ kind: "diff.produced", callId: "c", path, before, after });

  it("sums every diff a path received, and knows nothing about untouched paths", () => {
    const stats = touchedFileStats([
      edit("a.ts", "one\ntwo\n", "one\ntwo\nthree\n"),
      { kind: "tool.call", callId: "1", tool: "Read", summary: "b.ts", input: {} },
      edit("b.ts", "keep\ndrop\n", "keep\n"),
      edit("a.ts", "one\ntwo\nthree\n", "ONE\ntwo\nthree\n"),
    ]);
    expect(stats).toEqual({
      "a.ts": { added: 2, removed: 1 },
      "b.ts": { added: 0, removed: 1 },
    });
    expect(stats["never-touched.ts"]).toBeUndefined();
  });

  it("agrees with the session total the tiles already print", () => {
    // The per-file rows and the one-line figure beside the same task are read
    // side by side, so they must be the same arithmetic and not two of it.
    const events = [edit("a.ts", "", "x\ny\n"), edit("b.ts", "gone\n", "")];
    const perFile = Object.values(touchedFileStats(events));
    expect({
      added: perFile.reduce((sum, s) => sum + s.added, 0),
      removed: perFile.reduce((sum, s) => sum + s.removed, 0),
    }).toEqual(sessionDiffStat(events));
  });

  it("returns nothing at all for a log with no diffs in it", () => {
    expect(touchedFileStats([])).toEqual({});
  });

  it("agrees with the session total when an edit was declined", () => {
    // The declined edit's diff arrived before the refusal; neither figure
    // may count it, or the per-file rows would claim a change nothing made.
    const events: EngineEvent[] = [
      edit("a.ts", "", "x\ny\n"),
      { kind: "tool.call", callId: "no", tool: "Edit", summary: "b.ts", input: {} },
      { kind: "diff.produced", callId: "no", path: "b.ts", before: "keep\n", after: "keep\nmore\n" },
      { kind: "tool.result", callId: "no", ok: false, summary: "declined" },
    ];
    const stats = touchedFileStats(events);
    expect(stats).toEqual({ "a.ts": { added: 2, removed: 0 } });
    const perFile = Object.values(stats);
    expect({
      added: perFile.reduce((sum, s) => sum + s.added, 0),
      removed: perFile.reduce((sum, s) => sum + s.removed, 0),
    }).toEqual(sessionDiffStat(events));
  });
});
