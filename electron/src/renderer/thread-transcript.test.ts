import { beforeEach, describe, expect, it } from "vitest";
import { collapseSuccessfulToolRuns } from "./conversation-display";
import type { ConvItem } from "./conversation-reducer";
import {
  forgetThreadTurnTimings, rememberedThreadTurnTiming, rememberThreadTurnTiming,
  THREAD_CHANGES_ROWS, THREAD_TIMING_LIMIT, threadChangesLabel, threadChangesView, threadDisplayPlan,
  threadDrawnKeys, THREAD_RUN_MATCH_MS, THREAD_STAMP_GAP_MS, threadFileLetter, threadRunFor,
  threadStampKeys, threadStampParts, threadSteerKeys,
  threadTranscriptPlan, threadTurnDurationMs, threadTurnEndedAt, threadTurnTiming, threadWorkChips,
  threadWorkLead, threadWorkSentence, threadWorkSummary, type ThreadTurnMatch, type ThreadWorkCounts,
} from "./thread-transcript";
import type { TranscriptRow } from "./transcript-merge";

/* ==========================================================================
   THE THREADS LAYOUT'S TURNS (owner, 2026-10-06 redesign): what folds behind
   a finished turn's work line, what that line says, the files it changed, and
   how long it ran — from the merged rows and Rust's own stamps, never a guess.
   ========================================================================== */

const row = (item: ConvItem): TranscriptRow => ({ kind: "item", key: `l:${item.id}`, origin: "live", item });
const rows = (...items: ConvItem[]): TranscriptRow[] => items.map(row);
const user = (id: string, text = `prompt ${id}`): ConvItem => ({ kind: "user", id, text });
const text = (id: string, body = `answer ${id}`): ConvItem => ({ kind: "text", id, text: body, streaming: false });
const tool = (id: string, name: string, input: unknown = {},
  state: "ok" | "failed" | "running" = "ok"): ConvItem => ({ kind: "tool", id, tool: name, summary: "",
  input, ...(state === "running" ? {} : { result: { ok: state === "ok",
    summary: state === "ok" ? "done" : "exit 1" } }) });
const diff = (id: string, callId: string, path: string, added?: number, removed?: number): ConvItem =>
  ({ kind: "diff", id, callId, path, ...(added === undefined ? {} : { added }),
    ...(removed === undefined ? {} : { removed }) });
const none: ThreadWorkCounts = { read: 0, search: 0, list: 0, edit: 0, command: 0, subagent: 0, other: 0,
  steer: 0 };

describe("a Threads transcript's turns", () => {
  it("folds everything between the prompt and the final answer, and nothing else", () => {
    const plan = threadTranscriptPlan(rows(
      text("pre"),                                   // before any prompt: no turn
      user("u0"),
      text("t1", "I'll look."),
      tool("r1", "Read", { file_path: "/p/a.ts" }),
      text("t2", "Fixed it."),
      tool("after", "Bash", { command: "ls" }),     // trailing: after the answer
      user("u1"),
    ));
    expect(plan.turns.map(turn => turn.userKey)).toEqual(["l:u0", "l:u1"]);
    const [first, second] = plan.turns;
    expect(first).toMatchObject({ answerKey: "l:t2", answerText: "Fixed it.", endKey: "l:after",
      folded: 2, replied: true });
    expect([...plan.foldedIn]).toEqual([["l:t1", "l:u0"], ["l:r1", "l:u0"]]);
    expect(second).toMatchObject({ answerKey: null, endKey: null, folded: 0, replied: false });
    expect(plan.byUser.get("l:u1")).toBe(second);
  });

  it("never folds a failure, an unfinished call, a waiting approval, a CLI row or the seam", () => {
    const items: TranscriptRow[] = [
      row(user("u0")),
      row(tool("bad", "Bash", { command: "npm test" }, "failed")),
      row(tool("open", "Read", {}, "running")),
      row({ kind: "notice", id: "err", text: "Provider error", tone: "error" }),
      row({ kind: "notice", id: "info", text: "Retrying", tone: "info" }),
      row({ kind: "approval", id: "ask", tool: "Bash", summary: "rm", input: {} }),
      row({ kind: "approval", id: "done-ask", tool: "Bash", summary: "ls", input: {} }),
      row({ kind: "subagent", id: "sa", subagentType: "Explore", description: "", status: "failed",
        children: [], toolCount: 0, subagentCount: 0 }),
      row({ kind: "compact", id: "c", trigger: "auto", ok: false }),
      row(user("stop", "[Request interrupted by user]")),
      { kind: "mark", key: "seam" },
      row(text("answer")),
    ];
    const plan = threadTranscriptPlan(items, new Set(["ask"]));
    // One prompt: the interrupt marker opens no turn.
    expect(plan.turns).toHaveLength(1);
    expect([...plan.foldedIn.keys()]).toEqual(["l:info", "l:done-ask"]);
    expect([...plan.failed].sort()).toEqual(["l:bad", "l:c", "l:err", "l:sa"]);
  });

  it("does not call a refusal a failure", () => {
    const declined: ConvItem = { kind: "tool", id: "no", tool: "apply_patch", summary: "", input: {},
      result: { ok: false, summary: "declined" } };
    const plan = threadTranscriptPlan(rows(user("u"), declined, text("a")));
    expect(plan.failed.size).toBe(0);
    expect(plan.foldedIn.has("l:no")).toBe(false);
  });

  it("folds all of a turn's work when it has no final message", () => {
    const plan = threadTranscriptPlan(rows(user("u"), tool("r", "Read"), tool("e", "Edit")));
    expect(plan.turns[0]).toMatchObject({ answerKey: null, endKey: "l:e", folded: 2 });
  });

  it("counts the folded work by kind: distinct files read, files edited, commands run", () => {
    const plan = threadTranscriptPlan(rows(
      user("u"),
      tool("r1", "Read", { file_path: "/p/a.ts" }),
      tool("r2", "Read", { file_path: "/p/a.ts" }),   // the same file again
      tool("r3", "Read", {}),                          // names no file: one call
      tool("g", "Grep", { pattern: "x" }),
      tool("e1", "Edit", { file_path: "/p/a.ts" }),
      diff("d1", "e1", "/p/a.ts", 2, 1),
      tool("e2", "apply_patch", {}),
      diff("d2", "e2", "/p/b.ts", 1, 0),
      tool("b", "Bash", { command: "npm test" }),
      tool("m", "mcp__linear__get_issue", {}),
      { kind: "subagent", id: "sa", subagentType: "Explore", description: "", status: "done",
        children: [], toolCount: 2, subagentCount: 0 },
      text("a"),
    ));
    expect(plan.turns[0].work).toEqual({ read: 2, search: 1, list: 0, edit: 2, command: 1,
      subagent: 1, other: 1, steer: 0 });
  });

  it("says the work in one line, leaving out every kind nobody did", () => {
    expect(threadWorkSummary({ ...none, read: 3, edit: 2, command: 1 }))
      .toBe("read 3 files, edited 2 files, ran 1 command");
    expect(threadWorkSummary({ ...none, read: 1, search: 1, list: 2, command: 3, subagent: 2, other: 1 }))
      .toBe("read 1 file, searched once, listed 2 folders, ran 3 commands, used 2 subagents, called 1 tool");
    expect(threadWorkSummary({ ...none, search: 4 })).toBe("searched 4 times");
    expect(threadWorkSummary(none)).toBe("");
  });

  it("draws one chip per kind of work done, in the sentence's order, and none for nothing", () => {
    expect(threadWorkChips({ ...none, read: 3, edit: 2, command: 1 })).toEqual([
      { kind: "read", count: 3, label: "read 3" },
      { kind: "edit", count: 2, label: "edited 2" },
      { kind: "command", count: 1, label: "ran 1" },
    ]);
    expect(threadWorkChips({ read: 1, search: 2, list: 1, edit: 1, command: 4, subagent: 2, other: 1,
      steer: 1 }).map(chip => chip.label))
      .toEqual(["read 1", "searched 2", "listed 1", "edited 1", "ran 4", "2 subagents", "1 tool",
        "steered 1"]);
    expect(threadWorkChips({ ...none, subagent: 1, other: 3 }).map(chip => chip.label))
      .toEqual(["1 subagent", "3 tools"]);
    expect(threadWorkChips(none)).toEqual([]);
  });

  it("keeps the whole sentence for the line's accessible name", () => {
    expect(threadWorkSentence("Worked for 41s", { ...none, read: 3, edit: 2 }))
      .toBe("Worked for 41s · read 3 files, edited 2 files");
    expect(threadWorkSentence("Worked", none)).toBe("Worked");
  });

  it("says a steer in the sentence and on its own chip", () => {
    expect(threadWorkSummary({ ...none, read: 1, steer: 1 })).toBe("read 1 file, steered once");
    expect(threadWorkSummary({ ...none, steer: 3 })).toBe("steered 3 times");
    expect(threadWorkChips({ ...none, steer: 2 })).toEqual([{ kind: "steer", count: 2, label: "steered 2" }]);
  });

  it("leads with how long the turn worked, or only that it did", () => {
    expect(threadWorkLead(41_000)).toBe("Worked for 41s");
    expect(threadWorkLead(72_000)).toBe("Worked for 1m 12s");
    expect(threadWorkLead(undefined)).toBe("Worked");
    expect(threadWorkLead(162)).toBe("Worked for <1s");
  });
});

describe("a steer is part of the run it arrived in (owner, 2026-10-07)", () => {
  const turn = (ordinal: number, state: "running" | "done" = "done") => ({ ordinal, state });
  const match = (entries: [string, ReturnType<typeof turn>][], newest: number): ThreadTurnMatch =>
    ({ matched: new Map(entries), newest });

  it("folds a message with no Rust turn into the matched turn before it", () => {
    const transcript = rows(user("u0"), tool("r", "Read", { file_path: "/p/a.ts" }), user("steer"),
      tool("e", "Edit", { file_path: "/p/a.ts" }), text("a"), user("u1"), text("b"));
    const turns = match([["l:u0", turn(0)], ["l:u1", turn(1)]], 1);
    expect([...threadSteerKeys(transcript, turns)]).toEqual(["l:steer"]);
    const plan = threadTranscriptPlan(transcript, undefined, turns);
    expect(plan.turns.map(entry => entry.userKey)).toEqual(["l:u0", "l:u1"]);
    // Counted once, with the rest of the run: no second "Worked" for its half.
    expect(plan.turns[0].work).toMatchObject({ read: 1, edit: 1, steer: 1 });
    expect(plan.turns[0].folded).toBe(3);
    expect(plan.foldedIn.get("l:steer")).toBe("l:u0");
    expect(plan.steers.has("l:steer")).toBe(true);
  });

  it("keeps today's turns when Rust recorded none of the prompts", () => {
    const transcript = rows(user("u0"), text("a"), user("u1"), text("b"));
    expect(threadSteerKeys(transcript, match([], -1)).size).toBe(0);
    expect(threadSteerKeys(transcript, undefined).size).toBe(0);
    expect(threadTranscriptPlan(transcript, undefined, match([], -1)).turns).toHaveLength(2);
    expect(threadTranscriptPlan(transcript).turns).toHaveLength(2);
  });

  it("leaves an unmatched row its own turn where a Rust turn is free for it", () => {
    // Rust has turn 1 but no row matched it: u1's prompt may be stored differently.
    const between = rows(user("u0"), text("a"), user("u1"), text("b"), user("u2"), text("c"));
    expect(threadSteerKeys(between, match([["l:u0", turn(0)], ["l:u2", turn(2)]], 2)).size).toBe(0);
    // After the newest matched turn, with a newer Rust turn unmatched, the same.
    const after = rows(user("u0"), text("a"), user("u1"), text("b"));
    expect(threadSteerKeys(after, match([["l:u0", turn(0)]], 1)).size).toBe(0);
    // A row after an unmatched turn has no matched turn to belong to.
    const chain = rows(user("u0"), text("a"), user("x"), text("b"), user("y"), text("c"));
    expect(threadSteerKeys(chain, match([["l:u0", turn(0)]], 2)).size).toBe(0);
  });

  it("takes a fresh send Rust has not recorded yet for a turn, and a mid-run one for a steer", () => {
    const sent = rows(user("u0"), text("a"), user("next"));
    // The run is over and nothing has answered the new message: it is a send in flight.
    expect(threadSteerKeys(sent, match([["l:u0", turn(0)]], 0)).size).toBe(0);
    // While the run goes, the same row is a steer.
    expect([...threadSteerKeys(sent, match([["l:u0", turn(0, "running")]], 0))]).toEqual(["l:next"]);
    // Once anything follows it, the run took it in.
    expect([...threadSteerKeys([...sent, row(text("b"))], match([["l:u0", turn(0)]], 0))])
      .toEqual(["l:next"]);
  });

  it("never takes a row the CLI wrote for a steer", () => {
    const transcript = rows(user("u0"), user("stop", "[Request interrupted by user]"), text("a"));
    expect(threadSteerKeys(transcript, match([["l:u0", turn(0)]], 0)).size).toBe(0);
  });
});

describe("a turn's timing outlives the next send", () => {
  beforeEach(() => { forgetThreadTurnTimings(); });
  const first = { ordinal: 0, sentAt: 1_000, promptDigest: "a", hasStart: true, hasEnd: true,
    state: "done" as const };
  const second = { ...first, ordinal: 1, sentAt: 50_000, promptDigest: "b" };

  it("remembers Rust's stamps while they are the turn's, and reads them back after", () => {
    expect(threadTurnTiming("s", first, { turns: [first], lastRun: { startedAt: 1_010, endedAt: 35_010 } }))
      .toEqual({ durationMs: 34_000, endedAt: 35_010 });
    // Turn 2 is sent: Rust's one run is now turn 2's.
    const thread = { turns: [first, second], lastRun: { startedAt: 50_005 } };
    expect(threadRunFor(first, thread)).toBeUndefined();
    expect(threadTurnTiming("s", first, thread)).toEqual({ durationMs: 34_000, endedAt: 35_010 });
    // Another thread, or the same ordinal sent at another time, reads nothing.
    expect(threadTurnTiming("other", first, thread)).toEqual({});
    expect(threadTurnTiming("s", { ...first, sentAt: 2_000 }, thread)).toEqual({});
    expect(threadTurnTiming(undefined, first, thread)).toEqual({});
  });

  it("lets Rust's stamps replace this window's measure, never the other way", () => {
    rememberThreadTurnTiming("s", first, { durationMs: 33_000, endedAt: 34_000 }, "window");
    expect(rememberedThreadTurnTiming("s", first)).toEqual({ durationMs: 33_000, endedAt: 34_000 });
    rememberThreadTurnTiming("s", first, { durationMs: 34_000, endedAt: 35_010 }, "rust");
    rememberThreadTurnTiming("s", first, { durationMs: 1, endedAt: 2 }, "window");
    expect(rememberedThreadTurnTiming("s", first)).toEqual({ durationMs: 34_000, endedAt: 35_010 });
  });

  it("says nothing for a turn still running", () => {
    rememberThreadTurnTiming("s", first, { durationMs: 1_000, endedAt: 2_000 }, "window");
    expect(threadTurnTiming("s", { ...first, state: "running" }, { turns: [first] })).toEqual({});
  });

  it("holds a bounded number, dropping the least recently read first", () => {
    for (let ordinal = 0; ordinal <= THREAD_TIMING_LIMIT; ordinal += 1) {
      rememberThreadTurnTiming("s", { ordinal, sentAt: ordinal }, { durationMs: ordinal, endedAt: ordinal }, "window");
      // Ordinal 1 is read each time, so it stays while 0 ages out.
      if (ordinal >= 1) rememberedThreadTurnTiming("s", { ordinal: 1, sentAt: 1 });
    }
    expect(rememberedThreadTurnTiming("s", { ordinal: 0, sentAt: 0 })).toBeUndefined();
    expect(rememberedThreadTurnTiming("s", { ordinal: 1, sentAt: 1 })).toEqual({ durationMs: 1, endedAt: 1 });
    expect(rememberedThreadTurnTiming("s", { ordinal: THREAD_TIMING_LIMIT, sentAt: THREAD_TIMING_LIMIT }))
      .toBeDefined();
  });
});

describe("a turn's duration, from Rust's own stamps", () => {
  const turn = { sentAt: 1_000, state: "done" as const };
  it("is the run that began when the turn was sent, until it ended", () => {
    const run = { startedAt: 1_000, endedAt: 42_000 };
    expect(threadTurnDurationMs(turn, run)).toBe(41_000);
    expect(threadTurnEndedAt(turn, run)).toBe(42_000);
    // Rust stamps the send and the run's start from two places, ms apart.
    expect(threadTurnDurationMs({ ...turn, sentAt: 1_047 }, run)).toBe(41_000);
  });
  it("belongs only to the thread's newest turn", () => {
    const first = { ordinal: 0, sentAt: 1_000, promptDigest: "a", hasStart: true, hasEnd: true,
      state: "done" as const };
    const second = { ...first, ordinal: 1, sentAt: 5_000, promptDigest: "b" };
    const lastRun = { startedAt: 5_010, endedAt: 9_000 };
    expect(threadRunFor(second, { turns: [first, second], lastRun })).toBe(lastRun);
    expect(threadRunFor(first, { turns: [first, second], lastRun })).toBeUndefined();
    expect(threadRunFor(undefined, { turns: [first], lastRun })).toBeUndefined();
  });
  it("is unknown for any other turn, a run still going, or a turn still running", () => {
    expect(threadTurnDurationMs(turn, { startedAt: 1_000 + THREAD_RUN_MATCH_MS + 1, endedAt: 99_000 }))
      .toBeUndefined();
    expect(threadTurnDurationMs(turn, { startedAt: 1_000 })).toBeUndefined();
    expect(threadTurnDurationMs({ ...turn, state: "running" }, { startedAt: 1_000, endedAt: 9_000 }))
      .toBeUndefined();
    expect(threadTurnDurationMs(undefined, { startedAt: 1_000, endedAt: 9_000 })).toBeUndefined();
    expect(threadTurnEndedAt(turn, undefined)).toBeUndefined();
  });
});

describe("a turn's own end, kept by Rust (protocol 36)", () => {
  beforeEach(() => { forgetThreadTurnTimings(); });
  const first = { ordinal: 0, sentAt: 1_000, promptDigest: "a", hasStart: true, hasEnd: true,
    state: "done" as const, endedAt: 35_000 };
  const second = { ...first, ordinal: 1, sentAt: 50_000, promptDigest: "b", endedAt: 60_000 };

  it("times an earlier turn from its send to its own end, after the next send and a restart", () => {
    const thread = { turns: [first, second], lastRun: { startedAt: 50_005, endedAt: 60_000 } };
    expect(threadRunFor(first, thread)).toBeUndefined();
    // Nothing remembered in this window (a restart): Rust's own stamps answer.
    expect(threadTurnTiming("s", first, thread)).toEqual({ durationMs: 34_000, endedAt: 35_000 });
    expect(threadTurnDurationMs(first, undefined)).toBe(34_000);
    expect(threadTurnEndedAt(first, undefined)).toBe(35_000);
  });

  it("keeps the newest turn's run start while that run is the turn's", () => {
    expect(threadTurnDurationMs(second, { startedAt: 50_005, endedAt: 60_000 })).toBe(9_995);
    // A run still going (a newer send's) leaves the turn's own end to answer.
    expect(threadTurnDurationMs(second, { startedAt: 50_010 })).toBe(10_000);
  });

  it("says nothing for a turn from before protocol 36 nobody measured, or one still running", () => {
    const older = { ordinal: 0, sentAt: 1_000, promptDigest: "a", hasStart: true, hasEnd: true,
      state: "done" as const };
    expect(threadTurnTiming("s", older, { turns: [older, second] })).toEqual({});
    expect(threadTurnDurationMs({ ...first, state: "running" }, undefined)).toBeUndefined();
  });
});

describe("the files a turn changed", () => {
  it("lists each file once, with its kind and lines, from the turn's edits", () => {
    const plan = threadTranscriptPlan(rows(
      user("u"),
      { kind: "tool", id: "p", tool: "apply_patch", summary: "add /p/new.ts, update /p/a.ts", input: {},
        result: { ok: true, summary: "ok" } },
      diff("d1", "p", "/p/new.ts", 5, 0),
      diff("d2", "p", "/p/a.ts", 2, 1),
      tool("e", "Edit", { file_path: "/p/a.ts" }),
      diff("d3", "e", "/p/a.ts", 1, 1),
      // A refused patch changed nothing.
      tool("x", "apply_patch", {}, "failed"),
      diff("d4", "x", "/p/refused.ts", 9, 9),
      // An edit with no diff row still names its file, with no stat.
      tool("w", "Write", { file_path: "/p/c.ts" }),
      // A whole-file write's diff row holds only the new text: no stat.
      tool("w2", "Write", { file_path: "/p/session.ts" }),
      diff("d6", "w2", "/p/session.ts", 1, 0),
      // A subagent's edits are the turn's own, and its writes have no stat either.
      { kind: "subagent", id: "sa", subagentType: "Explore", description: "", status: "done",
        toolCount: 2, subagentCount: 0, children: [diff("d5", "inner", "/p/d.ts"),
          tool("iw", "Write", { file_path: "/p/e.ts" }), diff("d7", "iw", "/p/e.ts", 3, 0)] },
      text("a"),
    ));
    const turn = plan.turns[0];
    expect(turn.files).toEqual([
      { path: "/p/new.ts", verb: "add", stat: { added: 5, removed: 0 } },
      { path: "/p/a.ts", verb: "update", stat: { added: 3, removed: 2 } },
      { path: "/p/c.ts", verb: "unknown", stat: null },
      { path: "/p/session.ts", verb: "unknown", stat: null },
      { path: "/p/d.ts", verb: "unknown", stat: null },
      { path: "/p/e.ts", verb: "unknown", stat: null },
    ]);
    expect(turn.stat).toEqual({ added: 8, removed: 2 });
  });

  it("skips a subagent's refused edit the way it skips the turn's own", () => {
    const plan = threadTranscriptPlan(rows(
      user("u"),
      { kind: "subagent", id: "sa", subagentType: "builder", description: "", status: "done",
        toolCount: 2, subagentCount: 0, children: [
          tool("no", "Edit", { file_path: "/p/a.ts" }, "failed"), diff("d1", "no", "/p/a.ts", 4, 1),
          tool("go", "Edit", { file_path: "/p/b.ts" }, "running"), diff("d2", "go", "/p/b.ts", 2, 0)] },
      text("a"),
    ));
    expect(plan.turns[0].files).toEqual([{ path: "/p/b.ts", verb: "unknown", stat: { added: 2, removed: 0 } }]);
    expect(plan.turns[0].stat).toEqual({ added: 2, removed: 0 });
  });

  // 7 Oct 2026: a Write that replaced an 8-line file with one line read
  // "src/lib/session.ts +1 −0" under Rust's "+1 −8". The row keeps its path
  // and drops the count it cannot know.
  it("never prints a whole-file write's new lines as the file's change", () => {
    const plan = threadTranscriptPlan(rows(
      user("u"),
      tool("w", "Write", { file_path: "/p/src/lib/session.ts" }),
      diff("d", "w", "/p/src/lib/session.ts", 1, 0),
      text("a"),
    ));
    const turn = plan.turns[0];
    expect(turn.files).toEqual([{ path: "/p/src/lib/session.ts", verb: "unknown", stat: null }]);
    expect(turn.stat).toBeNull();
    expect(threadChangesView(turn, { filesChanged: 1, added: 1, removed: 8 }, "/p")).toEqual({
      files: 1, stat: { added: 1, removed: 8 },
      rows: [{ path: "/p/src/lib/session.ts", shown: "src/lib/session.ts", letter: "", verb: "unknown",
        stat: null }],
      more: 0,
    });
    // Without Rust's measure the card says what it knows: one file, no lines.
    expect(threadChangesView(turn, undefined, "/p")).toMatchObject({ files: 1, stat: null });
  });

  it("prefers Rust's measure of the turn, and falls back to the edit rows", () => {
    const files = [{ path: "/p/a.ts", verb: "update" as const, stat: { added: 2, removed: 1 } }];
    const turn = { files, stat: { added: 2, removed: 1 } };
    expect(threadChangesView(turn, { filesChanged: 3, added: 41, removed: 12 }, "/p")).toEqual({
      files: 3, stat: { added: 41, removed: 12 },
      rows: [{ path: "/p/a.ts", shown: "a.ts", letter: "M", verb: "update", stat: { added: 2, removed: 1 } }],
      more: 2,
    });
    expect(threadChangesView(turn, undefined, "/p")).toMatchObject({ files: 1,
      stat: { added: 2, removed: 1 }, more: 0 });
    // Rust counted files but not lines: the lines come from the rows.
    expect(threadChangesView(turn, { filesChanged: 1 }, "/p")?.stat).toEqual({ added: 2, removed: 1 });    // The rows name every file Rust counted but add up to other lines (a
    // whole-file write reports only its new lines): paths stay, counts go.
    expect(threadChangesView(turn, { filesChanged: 1, added: 1, removed: 4 }, "/p")?.rows[0]?.stat)
      .toBeNull();
    expect(threadChangesView(turn, { filesChanged: 1, added: 2, removed: 1 }, "/p")?.rows[0]?.stat)
      .toEqual({ added: 2, removed: 1 });
  });

  it("draws no card for a turn that changed nothing, and no '+ N more' under an empty list", () => {
    expect(threadChangesView({ files: [], stat: null }, undefined)).toBeNull();
    expect(threadChangesView({ files: [], stat: null }, { filesChanged: 0 })).toBeNull();
    expect(threadChangesView({ files: [], stat: null }, { filesChanged: 4 }))
      .toEqual({ files: 4, stat: null, rows: [], more: 0 });
  });

  it("lists six files and counts the rest", () => {
    const files = Array.from({ length: 9 }, (_, index) =>
      ({ path: `/p/f${index}.ts`, verb: "unknown" as const, stat: null }));
    const view = threadChangesView({ files, stat: null }, undefined);
    expect(view?.rows).toHaveLength(THREAD_CHANGES_ROWS);
    expect(view?.more).toBe(3);
  });

  it("prints a status letter only for a kind it knows, and never a zero stat", () => {
    expect(["add", "delete", "update", "unknown"].map(verb =>
      threadFileLetter(verb as "add"))).toEqual(["A", "D", "M", ""]);
    expect(threadChangesLabel({ files: 2, stat: { added: 10, removed: 1 } })).toBe("2 files changed +10 −1");
    expect(threadChangesLabel({ files: 1, stat: null })).toBe("1 file changed");
    expect(threadChangesLabel({ files: 1, stat: { added: 0, removed: 0 } })).toBe("1 file changed");
  });
});

describe("the plan over what the pane draws", () => {
  const transcript = rows(
    user("u0"),
    tool("r1", "Read", { file_path: "/p/a.ts" }),
    tool("r2", "Read", { file_path: "/p/b.ts" }),
    tool("bad", "Edit", { file_path: "/p/a.ts" }, "failed"),
    diff("d", "bad", "/p/a.ts", 1, 1),
    text("answer"),
    user("u1"),
    tool("only", "Bash", { command: "ls" }),
  );
  const plan = threadTranscriptPlan(transcript);
  const display = collapseSuccessfulToolRuns(transcript, 1);

  it("folds a run of finished calls whole, and never a failed call's merged change", () => {
    const view = threadDisplayPlan(display, plan);
    const folded = [...view.foldedBy];
    expect(folded).toHaveLength(2);
    expect(folded.map(([, turn]) => turn)).toEqual(["l:u0", "l:u1"]);
    expect(view.failed.size).toBe(1);
    // The answer carries turn 0's end block; turn 1 has no answer, so its last row does.
    expect([...view.endOf].map(([, turn]) => turn)).toEqual(["l:u0", "l:u1"]);
    expect(view.endOf.get("l:answer")).toBe("l:u0");
  });

  it("counts a closed work line as one drawn line for the jump capsule", () => {
    const view = threadDisplayPlan(display, plan);
    const closed = threadDrawnKeys(display, view.foldedBy, () => true);
    const open = threadDrawnKeys(display, view.foldedBy, () => false);
    expect(open).toHaveLength(display.length);
    expect(closed.length).toBeLessThanOrEqual(open.length);
    expect(closed.flat().sort()).toEqual(open.flat().sort());
  });
});

describe("the stamp over a prompt (the TN Code look, owner 2026-11-16)", () => {
  const hour = THREAD_STAMP_GAP_MS;
  const at = new Date(2026, 10, 16, 16, 12).getTime();

  it("stamps the first timed prompt, then one an hour or more after the last stamp", () => {
    const turns: [string, { sentAt?: number }][] = [
      ["untimed", {}], ["first", { sentAt: at }], ["soon", { sentAt: at + 5 * 60_000 }],
      ["just-under", { sentAt: at + hour - 1 }], ["an-hour", { sentAt: at + hour }],
      ["after", { sentAt: at + hour + 60_000 }], ["zero", { sentAt: 0 }],
      ["later", { sentAt: at + 2 * hour }],
    ];
    expect([...threadStampKeys(turns)]).toEqual(["first", "an-hour", "later"]);
    expect(threadStampKeys([]).size).toBe(0);
  });

  it("says the day in a word and then the time", () => {
    const now = new Date(2026, 10, 16, 18, 0).getTime();
    const time = new Date(at).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
    expect(threadStampParts(at, now)).toEqual({ day: "Today", time });
    // Just before midnight the day before is Yesterday, however few hours ago.
    const lateYesterday = new Date(2026, 10, 15, 23, 50).getTime();
    expect(threadStampParts(lateYesterday, new Date(2026, 10, 16, 0, 10).getTime()).day).toBe("Yesterday");
    const threeDays = new Date(2026, 10, 13, 9, 0).getTime();
    expect(threadStampParts(threeDays, now).day)
      .toBe(new Date(threeDays).toLocaleDateString([], { weekday: "long" }));
    const older = new Date(2026, 9, 2, 9, 0).getTime();
    expect(threadStampParts(older, now).day)
      .toBe(new Date(older).toLocaleDateString([], { month: "short", day: "numeric" }));
  });
});
