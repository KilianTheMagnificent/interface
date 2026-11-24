import { describe, expect, it } from "vitest";
import type { ConvItem } from "./conversation-reducer";
import {
  THREAD_PEEK_CHARS, THREAD_PEEK_LINES, THREAD_STEER_MAX, THREAD_WAITING_LINE, threadLiveAnnouncement,
  threadLiveTurn, threadPeekLines, threadStepLabel, type ThreadTrailRow, type ThreadTrailStep,
} from "./thread-live-turn";
import { THREAD_THINKING } from "./thread-live";
import type { TranscriptRow } from "./transcript-merge";

/* ==========================================================================
   A RUNNING THREADS TURN, AS IT DRAWS ITSELF (owner, 2026-10-07): the header's
   words, the trail's steps and their states, the waiting step, steers, the
   running edit's peek, and that a step's lines exist only once it finished.
   ========================================================================== */

const ROOT = "/Users/ann/Developer/atlas-web";
const row = (item: ConvItem): TranscriptRow => ({ kind: "item", key: `l:${item.id}`, origin: "live", item });
const rows = (...items: ConvItem[]): TranscriptRow[] => items.map(row);
type ToolItem = Extract<ConvItem, { kind: "tool" }>;
const call = (id: string, tool: string, input: Record<string, unknown>, result?: { ok: boolean; summary: string },
  summary = ""): ToolItem => ({ kind: "tool", id, tool, summary, input, ...(result === undefined ? {} : { result }) });
const ok = { ok: true, summary: "done" };
const text = (id: string, body: string, streaming = false): ConvItem => ({ kind: "text", id, text: body, streaming });
const steps = (segments: ReturnType<typeof threadLiveTurn>["segments"]): ThreadTrailRow[] =>
  segments.flatMap(segment => segment.kind === "trail" ? segment.rows : []);
const step = (model: ReturnType<typeof threadLiveTurn>, key: string): ThreadTrailStep => {
  const found = steps(model.segments).find(entry => entry.key === key);
  if (found === undefined || found.kind !== "step") throw new Error(`No step ${key}`);
  return found;
};

describe("a live turn's header", () => {
  it("thinks before any work, then says the newest running step", () => {
    expect(threadLiveTurn([]).header).toEqual({ state: "thinking", words: null,
      line: THREAD_THINKING, short: THREAD_THINKING });
    expect(threadLiveTurn([]).started).toBe(false);
    const model = threadLiveTurn(rows(
      call("r", "Read", { file_path: `${ROOT}/README.md` }, ok, `${ROOT}/README.md`),
      call("e", "Edit", { file_path: `${ROOT}/src/routes/login.ts`, new_string: "x" }, undefined,
        `${ROOT}/src/routes/login.ts`),
    ), { root: ROOT });
    expect(model.header).toMatchObject({ state: "working", line: "Editing src/routes/login.ts",
      short: "Editing login.ts" });
    expect(model.runningKey).toBe("l:e");
    expect(model.started).toBe(true);
  });

  it("thinks again between steps, and an empty streaming block is not output yet", () => {
    const model = threadLiveTurn(rows(text("t", "", true)));
    expect(model.started).toBe(false);
    expect(model.segments).toEqual([]);
    expect(threadLiveTurn(rows(call("r", "Read", {}, ok))).header.state).toBe("thinking");
  });

  it("waits on the owner while one of their approvals is open", () => {
    const model = threadLiveTurn(rows(
      call("b", "Bash", { command: "npm test" }, undefined, "npm test"),
      { kind: "approval", id: "ap", tool: "Bash", summary: "npm test", input: { command: "npm test" } },
    ), { waiting: new Set(["ap"]) });
    expect(model.header).toEqual({ state: "waiting", words: null, line: THREAD_WAITING_LINE,
      short: THREAD_WAITING_LINE });
    expect(model.waitingId).toBe("ap");
  });

  it("announces states, never steps or ticks", () => {
    expect(["thinking", "working", "waiting", "done"].map(state =>
      threadLiveAnnouncement(state as "done"))).toEqual(["Working", "Working", "Waiting for your approval",
      "Finished"]);
  });
});

describe("a live turn's trail", () => {
  it("breaks the trail around the prose, in transcript order", () => {
    const model = threadLiveTurn(rows(
      call("r1", "Read", { file_path: "/p/a.ts" }, ok),
      text("t1", "Looking."),
      call("r2", "Read", { file_path: "/p/b.ts" }, ok),
      call("g", "Grep", { pattern: "retry" }, ok),
    ));
    expect(model.segments.map(segment => segment.kind === "trail"
      ? segment.rows.map(entry => entry.key) : segment.key))
      .toEqual([["l:r1"], "l:t1", ["l:r2", "l:g"]]);
  });

  it("gives each step its state, and a refusal is not a failure", () => {
    const model = threadLiveTurn(rows(
      call("a", "Bash", { command: "ls" }, ok),
      call("b", "Bash", { command: "false" }, { ok: false, summary: "exit 1" }),
      call("c", "apply_patch", {}, { ok: false, summary: "declined" }),
      call("d", "Bash", { command: "sleep 9" }),
    ));
    expect(["l:a", "l:b", "l:c", "l:d"].map(key => step(model, key).state))
      .toEqual(["done", "failed", "done", "running"]);
    expect(step(model, "l:b").meta).toBe("failed");
    expect(step(model, "l:c").meta).toBe("declined");
  });

  it("prints a step's lines only once its call has a result, never while it runs", () => {
    const diff: ConvItem = { kind: "diff", id: "d", path: "/p/a.ts", callId: "e", added: 3, removed: 1 };
    const running = threadLiveTurn(rows(call("e", "Edit", { file_path: "/p/a.ts", new_string: "x" }), diff));
    expect(step(running, "l:e")).toMatchObject({ stat: null, keys: ["l:e", "l:d"] });
    // The diff is the call's; it is not a step of its own.
    expect(steps(running.segments)).toHaveLength(1);
    const done = threadLiveTurn(rows(call("e", "Edit", { file_path: "/p/a.ts" }, ok), diff));
    expect(step(done, "l:e").stat).toEqual({ added: 3, removed: 1 });
    const failed = threadLiveTurn(rows(call("e", "Edit", { file_path: "/p/a.ts" }, { ok: false, summary: "x" }), diff));
    expect(step(failed, "l:e").stat).toBeNull();
    // Unknown is never zero: a diff with no counts gives no stat.
    const uncounted = threadLiveTurn(rows(call("e", "Edit", { file_path: "/p/a.ts" }, ok),
      { kind: "diff", id: "d", path: "/p/a.ts", callId: "e" }));
    expect(step(uncounted, "l:e").stat).toBeNull();
  });

  it("keeps the absolute path on the title and shows it project-relative", () => {
    const model = threadLiveTurn(rows(call("r", "Read", { file_path: `${ROOT}/src/a.ts` }, ok, `${ROOT}/src/a.ts`)),
      { root: ROOT });
    expect(step(model, "l:r")).toMatchObject({ title: `${ROOT}/src/a.ts`,
      words: { verb: "Read", target: "src/a.ts" } });
    expect(threadStepLabel(step(model, "l:r"))).toBe("Read src/a.ts, done");
  });

  it("draws a change whose call is elsewhere as a finished step of its own", () => {
    const model = threadLiveTurn(rows({ kind: "diff", id: "d", path: `${ROOT}/a.ts`, callId: "gone",
      added: 2, removed: 0 }), { root: ROOT });
    expect(step(model, "l:d")).toMatchObject({ state: "done", words: { verb: "Edited", target: "a.ts" },
      stat: { added: 2, removed: 0 } });
  });

  it("delegates to a subagent in its own words", () => {
    const model = threadLiveTurn(rows({ kind: "subagent", id: "sa", subagentType: "Explore",
      description: "Find callers of verify", status: "running", children: [], toolCount: 0, subagentCount: 0 }));
    expect(step(model, "l:sa")).toMatchObject({ state: "running", meta: "Explore",
      words: { verb: "Delegating", target: "Find callers of verify" } });
    expect(model.header.line).toBe("Delegating Find callers of verify");
  });

  it("draws a steer as the owner's own row, first line only, bounded", () => {
    const long = `${"x".repeat(THREAD_STEER_MAX * 2)}\nsecond line`;
    const model = threadLiveTurn(rows({ kind: "user", id: "s", text: `\n${long}` }, { kind: "user", id: "other",
      text: "not a steer" }), { steers: new Set(["l:s"]) });
    const [steer] = steps(model.segments);
    expect(steer).toMatchObject({ kind: "steer", key: "l:s" });
    expect(steer.kind === "steer" && steer.text.length).toBe(THREAD_STEER_MAX);
    // A user row that is not a steer is drawn as it is.
    expect(model.segments[model.segments.length - 1]).toMatchObject({ kind: "row", key: "l:other" });
  });

  it("draws only what it is told to, for the finish's collapse", () => {
    const model = threadLiveTurn(rows(call("r", "Read", {}, ok), text("t", "Answer.")),
      { only: key => key === "l:r" });
    expect(model.segments.map(segment => segment.key)).toEqual(["trail:l:r"]);
  });
});

describe("a waiting step", () => {
  const bash = call("b", "Bash", { command: "npm test" }, undefined, "npm test");
  const ask: ConvItem = { kind: "approval", id: "ap", tool: "Bash", summary: "npm test",
    input: { command: "npm test" } };

  it("is the running call the approval is about, which then hides the request", () => {
    const model = threadLiveTurn(rows(bash, ask), { waiting: new Set(["ap"]) });
    expect(steps(model.segments)).toHaveLength(1);
    expect(step(model, "l:b")).toMatchObject({ state: "waiting", meta: "needs your OK", approvalId: "ap",
      keys: ["l:b", "l:ap"], words: { verb: "Run", target: "npm test" } });
    expect(threadStepLabel(step(model, "l:b"))).toBe("Run npm test, needs your OK, waiting");
  });

  it("is the request itself when no running call matches it", () => {
    const other = call("x", "Bash", { command: "ls" }, undefined, "ls");
    const model = threadLiveTurn(rows(other, ask), { waiting: new Set(["ap"]) });
    expect(step(model, "l:x").state).toBe("running");
    expect(step(model, "l:ap")).toMatchObject({ state: "waiting", meta: "needs your OK" });
  });

  it("reads as a quiet done row once answered, and as checking while the policy decides", () => {
    const allowed = threadLiveTurn(rows(bash, { ...ask, resolved: "allow" } as ConvItem));
    expect(step(allowed, "l:ap")).toMatchObject({ state: "done", prefix: "Allowed" });
    expect(threadStepLabel(step(allowed, "l:ap"))).toBe("Allowed Run npm test, done");
    const denied = threadLiveTurn(rows(ask), { verdict: () => "deny" });
    expect(step(denied, "l:ap").prefix).toBe("Denied");
    const checking = threadLiveTurn(rows(ask), { checking: new Set(["ap"]) });
    expect(step(checking, "l:ap")).toMatchObject({ state: "running", meta: "checking" });
    // A request the policy is checking is not the step running.
    expect(checking.header.state).toBe("thinking");
  });
});

describe("a running edit's peek", () => {
  it("shows Edit's new lines, MultiEdit's in order, and Write's opening, blank lines skipped", () => {
    expect(threadPeekLines(call("e", "Edit", { new_string: "a\n\n  b\r\nc" }))).toEqual(["a", "  b", "c"]);
    expect(threadPeekLines(call("m", "MultiEdit", { edits: [{ new_string: "one\ntwo" },
      { new_string: "three\nfour\nfive" }] }))).toEqual(["one", "two", "three", "four"]);
    expect(threadPeekLines(call("w", "Write", { content: "1\n2\n3\n4\n5\n6" }))).toEqual(["1", "2", "3", "4"]);
    expect(threadPeekLines(call("r", "Read", { file_path: "/a" }))).toEqual([]);
    expect(threadPeekLines(call("b", "Bash", { command: "ls" }))).toEqual([]);
  });

  it("peeks at what an edit adds, never the lines it keeps", () => {
    const file = "import a;\nexport function f() {\n  return 1;\n}\n";
    expect(threadPeekLines(call("ap", "Edit", { old_string: file,
      new_string: `${file}import { limit } from "./limit";\n` })))
      .toEqual(['import { limit } from "./limit";']);
    expect(threadPeekLines(call("mid", "Edit", { old_string: "a\nb\nc", new_string: "a\nB1\nB2\nc" })))
      .toEqual(["B1", "B2"]);
    expect(threadPeekLines(call("del", "Edit", { old_string: "a\nb\nc", new_string: "a\nc" }))).toEqual([]);
    expect(threadPeekLines(call("me", "MultiEdit", { edits: [{ old_string: "x\ny", new_string: "x\ny\nz" }] })))
      .toEqual(["z"]);
  });

  it("is bounded however large the input", () => {
    const huge = `${"y".repeat(THREAD_PEEK_CHARS * 3)}\n`.repeat(10_000);
    const lines = threadPeekLines(call("w", "Write", { content: huge }));
    expect(lines).toHaveLength(THREAD_PEEK_LINES);
    expect(lines.every(line => line.length === THREAD_PEEK_CHARS && line.endsWith("…"))).toBe(true);
  });

  it("is drawn only while the edit runs, and remembered per call", () => {
    const edit = call("e", "Edit", { new_string: "x" });
    expect(threadPeekLines(edit)).toBe(threadPeekLines(edit));
    expect(step(threadLiveTurn(rows(edit)), "l:e").peek).toEqual(["x"]);
    expect(step(threadLiveTurn(rows({ ...edit, result: ok })), "l:e").peek).toEqual([]);
  });
});
