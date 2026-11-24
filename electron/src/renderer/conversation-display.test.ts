import { describe, expect, it } from "vitest";
import {
  collapseSuccessfulToolRuns, displayRowKeys, groupSubagentGrids, retainDisplayRows,
  type ConversationDisplayRow,
} from "./conversation-display";
import { expandImportedRows } from "./transcript-import";
import { mergeTranscript, type TranscriptRow } from "./transcript-merge";
import { toolRunMinimum } from "./transcript-density";
import type { ConvItem } from "./conversation-reducer";
import { createConversationProjection } from "./conversation-projection";
import type { EngineEvent } from "../shared/engine";

const ok = { ok: true, summary: "fine" };
const tool = (key: string, name: string, result?: { ok: boolean; summary: string }): TranscriptRow => ({
  kind: "item", key, origin: "live",
  item: { kind: "tool", id: key, tool: name, summary: key, input: {},
    ...(result === undefined ? {} : { result }) },
});
const diff = (key: string, callId: string, path: string): TranscriptRow => ({
  kind: "item", key, origin: "live",
  item: { kind: "diff", id: key, callId, path, added: 1, removed: 0 },
});
const shape = (display: readonly ConversationDisplayRow[]) => display.map(row =>
  [row.kind, row.kind === "tool-run" ? row.tool : "", displayRowKeys(row)]);

describe("collapseSuccessfulToolRuns over a stretch that keeps breaking", () => {
  // One work stretch with a running call, a failure, a refusal and an orphan
  // diff inside it: every break stands up on its own, a failed or refused
  // call keeps the diff it produced, and the finished work either side folds.
  const rows: TranscriptRow[] = [
    tool("r1", "Read", ok), tool("r2", "Read", ok),
    tool("run", "Bash"),
    tool("e1", "apply_patch", { ok: false, summary: "boom" }), diff("d-e1", "e1", "/repo/e1.ts"),
    tool("g1", "Grep", ok),
    tool("dec", "apply_patch", { ok: false, summary: "declined" }), diff("d-dec", "dec", "/repo/dec.ts"),
    diff("d-other", "elsewhere", "/repo/other.ts"),
    tool("r3", "Read", ok), tool("b1", "Bash", ok),
    { kind: "item", key: "t", origin: "live",
      item: { kind: "text", id: "t", text: "done", streaming: false } },
    tool("r4", "Read", ok),
    tool("e2", "Edit", ok), diff("d-e2", "e2", "/repo/e2.ts"),
    tool("e3", "Edit", ok), diff("d-e3", "e3", "/repo/e3.ts"),
  ];

  it("folds only the finished partitions, at every step of the dial", () => {
    const tail = [
      ["file-change", "", ["dec", "d-dec"]],
      ["tool-run", "work", ["d-other", "r3", "b1"]],
      ["row", "", ["t"]],
      ["tool-run", "work", ["r4", "e2", "d-e2", "e3", "d-e3"]],
    ];
    expect(shape(collapseSuccessfulToolRuns(rows, 0))).toEqual([
      ["tool-run", "work", ["r1", "r2"]],
      ["row", "", ["run"]],
      ["file-change", "", ["e1", "d-e1"]],
      ["tool-run", "work", ["g1"]],
      ...tail,
    ]);
    expect(shape(collapseSuccessfulToolRuns(rows, 1))).toEqual([
      ["tool-run", "work", ["r1", "r2"]],
      ["row", "", ["run"]],
      ["file-change", "", ["e1", "d-e1"]],
      ["row", "", ["g1"]],
      ...tail,
    ]);
    expect(shape(collapseSuccessfulToolRuns(rows, 3))).toEqual([
      ["row", "", ["r1"]],
      ["row", "", ["r2"]],
      ["row", "", ["run"]],
      ["file-change", "", ["e1", "d-e1"]],
      ["row", "", ["g1"]],
      ...tail,
    ]);
  });
});

describe("retainDisplayRows", () => {
  const rows: TranscriptRow[] = [
    tool("r1", "Read", ok), tool("r2", "Read", ok),
    { kind: "item", key: "t", origin: "live",
      item: { kind: "text", id: "t", text: "done", streaming: false } },
  ];

  it("keeps a row whose members are the same, and the list when nothing changed", () => {
    const first = collapseSuccessfulToolRuns(rows, 1);
    const second = collapseSuccessfulToolRuns([...rows], 1);
    expect(second[0]).not.toBe(first[0]);
    const retained = retainDisplayRows(first, second);
    expect(retained).toBe(first);
  });

  it("replaces a run whose call changed, gained a member, or folded differently", () => {
    const first = collapseSuccessfulToolRuns(rows, 1);
    const changed = retainDisplayRows(first, collapseSuccessfulToolRuns(
      [tool("r1", "Read", { ok: true, summary: "other" }), ...rows.slice(1)], 1));
    expect(changed[0]).not.toBe(first[0]);
    expect(changed[1]).toBe(first[1]);
    const grown = retainDisplayRows(first, collapseSuccessfulToolRuns(
      [...rows.slice(0, 2), tool("r3", "Read", ok), rows[2]], 1));
    expect(grown[0]).not.toBe(first[0]);
    expect(grown[0]).toMatchObject({ kind: "tool-run", key: "tool-run:r1" });
    const detailed = retainDisplayRows(first,
      collapseSuccessfulToolRuns(rows, toolRunMinimum("detailed")));
    expect(detailed.some(row => row === first[0])).toBe(false);
    expect(detailed.at(-1)).toBe(first.at(-1));
  });

  it("returns exactly what the pass produced, only with older objects", () => {
    const first = collapseSuccessfulToolRuns(rows, 1);
    const next = collapseSuccessfulToolRuns([tool("r0", "Bash", ok), ...rows], 1);
    expect(retainDisplayRows(first, next)).toEqual(next);
  });
});

describe("a streamed token keeps every settled display row", () => {
  const stream: EngineEvent[] = [
    { kind: "user.message", text: "go" },
    { kind: "tool.call", callId: "r1", tool: "Read", summary: "a", input: {} },
    { kind: "tool.result", callId: "r1", ok: true, summary: "read" },
    { kind: "tool.call", callId: "b1", tool: "Bash", summary: "ls", input: {} },
    { kind: "tool.result", callId: "b1", ok: true, summary: "ok" },
    { kind: "tool.call", callId: "e1", tool: "apply_patch", summary: "patch", input: {} },
    { kind: "diff.produced", callId: "e1", path: "/repo/a.ts", before: "a", after: "b" },
    { kind: "tool.result", callId: "e1", ok: true, summary: "applied" },
    { kind: "message.delta", text: "Between." },
    { kind: "diff.produced", callId: "ghost", path: "/repo/x.ts", before: "", after: "x" },
    { kind: "diff.produced", callId: "ghost", path: "/repo/y.ts", before: "", after: "y" },
    { kind: "tool.call", callId: "s1", tool: "Agent", summary: "one", input: {} },
    { kind: "subagent.started", agentKey: "s1", subagentType: "reviewer", description: "one" },
    { kind: "tool.call", callId: "s2", tool: "Agent", summary: "two", input: {} },
    { kind: "subagent.started", agentKey: "s2", subagentType: "reviewer", description: "two" },
    { kind: "message.delta", text: "Here" },
  ];

  const pipeline = (items: readonly ConvItem[], previous: readonly ConversationDisplayRow[],
    minimum: number): readonly ConversationDisplayRow[] => retainDisplayRows(previous,
    groupSubagentGrids(collapseSuccessfulToolRuns(
      expandImportedRows(mergeTranscript([], items)), minimum)));

  for (const density of ["balanced", "detailed"] as const) {
    it(`reuses the object for every row the delta did not touch (${density})`, () => {
      const project = createConversationProjection();
      const minimum = toolRunMinimum(density);
      const before = pipeline(project(stream), [], minimum);
      const after = pipeline(project([...stream, { kind: "message.delta", text: " is more" }]),
        before, minimum);
      expect(after).toHaveLength(before.length);
      const kinds = new Set(before.map(row => row.kind));
      expect(kinds).toContain("subagent-grid");
      expect(kinds).toContain(density === "balanced" ? "tool-run" : "file-change");
      if (density === "detailed") expect(kinds).toContain("edited-files");
      // The streaming message is the one row that changed.
      after.slice(0, -1).forEach((row, index) => { expect(row).toBe(before[index]); });
      expect(after.at(-1)).not.toBe(before.at(-1));
      expect(after.at(-1)).toMatchObject({ kind: "row",
        row: { item: { kind: "text", text: "Here is more" } } });
    });
  }
});
