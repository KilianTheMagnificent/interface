import { describe, expect, it } from "vitest";
import type { EngineEvent } from "../shared/engine";
import {
  diffStatLabel, dirtyFilesLabel, formatDiffStat, lineDelta,
  sessionDiffFacts, sessionDiffPaths, sessionDiffStat,
} from "./diff-stats";

const edit = (before: string, after: string): EngineEvent =>
  ({ kind: "diff.produced", callId: "d1", path: "/repo/a.ts", before, after });

describe("lineDelta", () => {
  it("counts only the lines that actually moved inside a shared context", () => {
    // The exact shape of a Claude Edit: old_string/new_string carry context
    // lines the model quoted so the match is unambiguous. Counting the whole
    // fragment would report 4/4 for a one-line change.
    expect(lineDelta("a\nb\nc\nd", "a\nB\nc\nd")).toEqual({ added: 1, removed: 1 });
  });

  it("reports a pure insertion and a pure deletion without inventing the other", () => {
    expect(lineDelta("a\nc", "a\nb\nc")).toEqual({ added: 1, removed: 0 });
    expect(lineDelta("a\nb\nc", "a\nc")).toEqual({ added: 0, removed: 1 });
  });

  it("treats a Write as every line added and none removed", () => {
    expect(lineDelta("", "one\ntwo\nthree")).toEqual({ added: 3, removed: 0 });
  });

  it("does not count a trailing newline as an extra line", () => {
    expect(lineDelta("", "one\n")).toEqual({ added: 1, removed: 0 });
    expect(lineDelta("a\n", "a\nb\n")).toEqual({ added: 1, removed: 0 });
  });

  it("is zero for a diff whose two sides are identical", () => {
    expect(lineDelta("a\nb", "a\nb")).toEqual({ added: 0, removed: 0 });
    expect(lineDelta("", "")).toEqual({ added: 0, removed: 0 });
  });

  it("keeps the interleaved case honest rather than calling the block replaced", () => {
    // Two separate one-line edits with untouched lines between them. A
    // prefix/suffix-only measure would call all five lines changed.
    expect(lineDelta("1\n2\n3\n4\n5", "1\nX\n3\nY\n5")).toEqual({ added: 2, removed: 2 });
  });
});

describe("sessionDiffStat", () => {
  it("is null when the session produced no diff at all", () => {
    expect(sessionDiffStat([])).toBeNull();
    expect(sessionDiffStat([{ kind: "message.delta", text: "hello" }])).toBeNull();
  });

  it("sums every diff the session streamed", () => {
    expect(sessionDiffStat([
      edit("a\nb", "a\nB"),
      { kind: "message.delta", text: "working" },
      edit("", "one\ntwo"),
    ])).toEqual({ added: 3, removed: 1 });
  });

  it("reports zero — not null — for a session whose only diff changed nothing", () => {
    expect(sessionDiffStat([edit("a", "a")])).toEqual({ added: 0, removed: 0 });
  });
});

describe("a declined edit changes nothing", () => {
  // Claude's diff comes from the Edit's input, in the same breath as the call,
  // so it exists before the owner approves it. A refused call wrote nothing.
  const call = (callId: string, path: string): EngineEvent[] => [
    { kind: "tool.call", callId, tool: "Edit", summary: path, input: {} },
    { kind: "diff.produced", callId, path, before: "foo\nx", after: "bar\ny\nz" },
  ];
  const refused = (callId: string): EngineEvent =>
    ({ kind: "tool.result", callId, ok: false, summary: "The user doesn't want to proceed with this tool use." });

  it("leaves a session whose only edit was declined with no stat and no paths", () => {
    const log = [{ kind: "user.message", text: "rename foo" } as EngineEvent,
      ...call("c1", "/repo/a.ts"), refused("c1"),
      { kind: "message.delta", text: "OK, I won't change it." } as EngineEvent];
    expect(sessionDiffStat(log)).toBeNull();
    expect(sessionDiffPaths(log)).toEqual([]);
    expect(sessionDiffFacts(log)).toEqual({ stat: null, paths: [] });
  });

  it("counts the edits that applied and the ones still running, not the refused one", () => {
    const log = [
      ...call("c1", "/repo/a.ts"), refused("c1"),
      ...call("c2", "/repo/b.ts"),
      { kind: "tool.result", callId: "c2", ok: true, summary: "edited" } as EngineEvent,
      ...call("c3", "/repo/c.ts"),
    ];
    expect(sessionDiffStat(log)).toEqual({ added: 6, removed: 4 });
    expect(sessionDiffPaths(log)).toEqual(["/repo/b.ts", "/repo/c.ts"]);
    expect(sessionDiffFacts(log)).toEqual({ stat: sessionDiffStat(log), paths: sessionDiffPaths(log) });
  });
});

describe("formatDiffStat", () => {
  it("prints the pair with a real minus sign", () => {
    expect(formatDiffStat({ added: 12, removed: 4 })).toBe("+12 −4");
  });

  it("says nothing at all when the counts are unknown", () => {
    expect(formatDiffStat(null)).toBe("");
    expect(formatDiffStat(undefined)).toBe("");
  });

  it("refuses to claim +0 −0", () => {
    expect(formatDiffStat({ added: 0, removed: 0 })).toBe("");
  });

  it("still prints a one-sided change", () => {
    expect(formatDiffStat({ added: 0, removed: 7 })).toBe("+0 −7");
    expect(formatDiffStat({ added: 7, removed: 0 })).toBe("+7 −0");
  });
});

describe("sessionDiffPaths", () => {
  it("counts distinct paths this window produced, not events", () => {
    expect(sessionDiffPaths([])).toEqual([]);
    expect(sessionDiffPaths([
      { kind: "diff.produced", callId: "d1", path: "/repo/a.ts", before: "a", after: "b" },
      { kind: "message.delta", text: "working" },
      { kind: "diff.produced", callId: "d1", path: "/repo/b.ts", before: "", after: "x" },
      { kind: "diff.produced", callId: "d1", path: "/repo/a.ts", before: "b", after: "c" },
    ])).toEqual(["/repo/a.ts", "/repo/b.ts"]);
  });
});

describe("sessionDiffFacts", () => {
  it("is sessionDiffStat and sessionDiffPaths from one pass", () => {
    const logs: EngineEvent[][] = [[], [{ kind: "message.delta", text: "hi" }], [
      { kind: "diff.produced", callId: "d1", path: "/repo/a.ts", before: "a\n", after: "b\nc\n" },
      { kind: "message.delta", text: "working" },
      { kind: "diff.produced", callId: "d2", path: "/repo/b.ts", before: "", after: "x" },
      { kind: "diff.produced", callId: "d3", path: "/repo/a.ts", before: "b\nc\n", after: "c\n" },
    ]];
    for (const log of logs) {
      expect(sessionDiffFacts(log)).toEqual({ stat: sessionDiffStat(log), paths: sessionDiffPaths(log) });
    }
  });
});

describe("dirtyFilesLabel", () => {
  it("prints the file count and the line pair", () => {
    expect(dirtyFilesLabel(3, { added: 12, removed: 4 })).toBe("3 files changed +12 −4");
    expect(dirtyFilesLabel(1, { added: 2, removed: 1 })).toBe("1 file changed +2 −1");
  });

  it("is empty when the counts are unknown or a net-zero", () => {
    expect(dirtyFilesLabel(3, null)).toBe("");
    expect(dirtyFilesLabel(3, undefined)).toBe("");
    expect(dirtyFilesLabel(3, { added: 0, removed: 0 })).toBe("");
  });
});

describe("diffStatLabel", () => {
  it("spells the same fact for a screen reader", () => {
    expect(diffStatLabel({ added: 12, removed: 4 })).toBe("12 lines added, 4 removed");
    expect(diffStatLabel({ added: 1, removed: 0 })).toBe("1 line added, 0 removed");
  });

  it("is empty wherever the visible text is", () => {
    expect(diffStatLabel(null)).toBe("");
    expect(diffStatLabel({ added: 0, removed: 0 })).toBe("");
  });
});
