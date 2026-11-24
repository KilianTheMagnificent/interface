import { createHash } from "node:crypto";
import { describe, expect, it } from "vitest";
import type { ThreadTurn } from "../shared/threads";
import fixture from "../fixtures/threads-v34.json";
import { COMPACTED_NOTICE } from "./live-events";
import type { TranscriptRow } from "./transcript-merge";
import {
  isCliWrittenPrompt, matchThreadTurns, normalizeThreadPrompt, sha256Hex, threadPromptDigest,
  threadPromptRows, threadRetryKey, threadTurnChangeText, threadTurnEnds, threadTurnRowKeys,
  transcriptWasCompacted, turnRestoreForOrdinal,
} from "./thread-turns";

/* ==========================================================================
   WHICH TRANSCRIPT PROMPT IS WHICH RUST TURN. Every control the thread view
   draws on a user turn (its diff, Edit from here) and at a turn's end (Fork
   from here) hangs on this map, so a wrong entry is an edit or a fork at the
   wrong moment, and a missing entry is only a missing button.
   ========================================================================== */

const turn = (ordinal: number, prompt: string, over: Partial<ThreadTurn> = {}): ThreadTurn => ({
  ordinal, sentAt: 1_000 + ordinal, promptDigest: threadPromptDigest(prompt),
  hasStart: true, hasEnd: true, state: "done", ...over,
});
const user = (key: string, text: string): TranscriptRow =>
  ({ kind: "item", key, origin: "history", item: { kind: "user", id: key, text } });
const reply = (key: string): TranscriptRow =>
  ({ kind: "item", key, origin: "history", item: { kind: "text", id: key, text: "ok", streaming: false } });

describe("threadPromptDigest", () => {
  it("is SHA-256 of the UTF-8 bytes, hex, like the host's", () => {
    for (const text of ["", "abc", "Fix the flaky login test", "naïve — ünïcode ✓ 🧵",
      "x".repeat(55), "y".repeat(56), "z".repeat(64), "w".repeat(1000)]) {
      expect(sha256Hex(text)).toBe(createHash("sha256").update(text, "utf8").digest("hex"));
    }
  });

  it("matches the shared fixture's recorded digest for the same prompt", () => {
    const codex = fixture.view.threads.find(thread => thread.key.startsWith("codex#019a3c5e"));
    expect(codex?.turns[0]?.promptDigest).toBe(threadPromptDigest("Fix the flaky login test"));
    expect(threadPromptDigest("Fix the flaky login test")).toBe("4b886b8719e00bff");
  });

  it("folds CRLF to LF and trims before hashing, as Rust does", () => {
    expect(normalizeThreadPrompt("  one\r\ntwo\r\n\t")).toBe("one\ntwo");
    expect(threadPromptDigest("one\r\ntwo")).toBe(threadPromptDigest("one\ntwo"));
    expect(threadPromptDigest("\n\t Fix the flaky login test  \n"))
      .toBe("4b886b8719e00bff");
    // Interior whitespace is the prompt's own and is kept.
    expect(threadPromptDigest("Fix  the flaky login test")).not.toBe("4b886b8719e00bff");
    // A lone CR is not a CRLF pair and is not folded.
    expect(normalizeThreadPrompt("a\rb")).toBe("a\rb");
  });

  it("trims what Rust's str::trim trims, not what JavaScript's does", () => {
    expect(normalizeThreadPrompt("\u0085ok\u0085")).toBe("ok");
    expect(normalizeThreadPrompt("﻿ok")).toBe("﻿ok");
  });
});

describe("matchThreadTurns", () => {
  it("maps each prompt to the turn with its digest", () => {
    const rows = [user("h:u1", "first"), reply("h:t1"), user("h:u2", "second")];
    const turns = [turn(0, "first"), turn(1, "second")];
    const map = matchThreadTurns(threadPromptRows(rows), turns, true);
    expect(map.get("h:u1")?.ordinal).toBe(0);
    expect(map.get("h:u2")?.ordinal).toBe(1);
  });

  it("pairs the k-th repeat of a prompt with the k-th turn carrying it", () => {
    const rows = [user("a", "continue"), user("b", "other"), user("c", "continue"),
      user("d", "continue")];
    const turns = [turn(0, "continue"), turn(1, "other"), turn(2, "continue"),
      turn(3, "continue")];
    const map = matchThreadTurns(threadPromptRows(rows), turns, true);
    expect([...map].map(([key, value]) => [key, value.ordinal]))
      .toEqual([["a", 0], ["c", 2], ["d", 3], ["b", 1]]);
  });

  it("gives an unmatched row no turn at all", () => {
    const rows = [user("a", "typed in a terminal"), user("b", "sent here")];
    const map = matchThreadTurns(threadPromptRows(rows), [turn(0, "sent here")], true);
    expect(map.has("a")).toBe(false);
    expect(map.get("b")?.ordinal).toBe(0);
    expect(matchThreadTurns(threadPromptRows(rows), [], true).size).toBe(0);
  });

  it("aligns from the newest end when the transcript is missing its start", () => {
    // Turns 0..2 all said "yes"; only the newest two rows are loaded. Read from
    // the start, the first loaded row would claim turn 0 — the wrong message.
    const rows = [user("x", "yes"), user("y", "yes")];
    const turns = [turn(0, "yes"), turn(1, "yes"), turn(2, "yes")];
    const partial = matchThreadTurns(threadPromptRows(rows), turns, false);
    expect(partial.get("x")?.ordinal).toBe(1);
    expect(partial.get("y")?.ordinal).toBe(2);
  });

  it("aligns from the newest end when Rust kept only the newest turns", () => {
    const rows = [user("a", "go"), user("b", "go"), user("c", "go")];
    const kept = [turn(1, "go"), turn(2, "go")];
    const map = matchThreadTurns(threadPromptRows(rows), kept, true);
    expect(map.has("a")).toBe(false);
    expect(map.get("b")?.ordinal).toBe(1);
    expect(map.get("c")?.ordinal).toBe(2);
  });

  it("ignores rows that are not the owner's top-level prompts", () => {
    const rows: TranscriptRow[] = [reply("t"), { kind: "mark", key: "m" }, user("u", "hi")];
    expect(threadPromptRows(rows)).toEqual([{ key: "u", text: "hi" }]);
  });

  it("never pairs rows out of turn order: both rows of a crossing get no control", () => {
    // Review, 6 Oct 2026. The provider stored the first "go" differently, so
    // read from the start the second "go" claims turn 0 — above "fix", which
    // is turn 1. Either row could be the wrong one, so neither gets a control.
    const rows = [user("a", "fix"), user("b", "go"), user("c", "done")];
    const turns = [turn(0, "go"), turn(1, "fix"), turn(2, "go"), turn(3, "done")];
    const map = matchThreadTurns(threadPromptRows(rows), turns, true);
    expect(map.has("a")).toBe(false);
    expect(map.has("b")).toBe(false);
    expect(map.get("c")?.ordinal).toBe(3);
  });

  it("keeps only rows whose turn is above every matched turn before them and below every one after", () => {
    const rows = [user("a", "one"), user("b", "late"), user("c", "two"), user("d", "three")];
    // "late" was recorded as turn 5 and crosses "two" and "three". Which side
    // is wrong is not knowable here, so all three go; "one" crosses nothing.
    const turns = [turn(0, "one"), turn(1, "two"), turn(2, "three"), turn(5, "late")];
    const map = matchThreadTurns(threadPromptRows(rows), turns, true);
    expect([...map].map(([key, value]) => [key, value.ordinal]).sort())
      .toEqual([["a", 0]]);
    // In order, nothing is dropped.
    const ordered = matchThreadTurns(threadPromptRows([user("a", "one"), user("c", "two"),
      user("d", "three"), user("b", "late")]), turns, true);
    expect(ordered.size).toBe(4);
  });

  it("gives Claude's interrupt markers and ! bash echoes no control, and never counts them", () => {
    const rows = [
      user("u0", "first"), reply("r0"),
      user("m1", "[Request interrupted by user for tool use]"),
      user("b1", "<bash-input>ls</bash-input>"),
      user("b2", "<bash-stdout>Cargo.toml</bash-stdout><bash-stderr></bash-stderr>"),
      user("u1", "second"), reply("r1"),
      user("m2", "[Request interrupted by user]"),
      user("u2", "third prompt mentions [Request interrupted by user]"), reply("r2"),
    ];
    // Even a turn whose prompt digest is a marker's (the owner typed it) gets
    // no control on the marker row: the CLI writes exactly that text itself.
    const turns = [turn(0, "first"), turn(1, "second"),
      turn(2, "third prompt mentions [Request interrupted by user]"),
      turn(3, "[Request interrupted by user]")];
    expect(threadPromptRows(rows).map(row => row.key)).toEqual(["u0", "u1", "u2"]);
    const map = matchThreadTurns(threadPromptRows(rows), turns, true);
    expect([...map].map(([key, value]) => [key, value.ordinal]))
      .toEqual([["u0", 0], ["u1", 1], ["u2", 2]]);
    expect(isCliWrittenPrompt("  [Request interrupted by user]\n")).toBe(true);
    expect(isCliWrittenPrompt("<bash-input>ls</bash-input> and then")).toBe(false);
    expect(isCliWrittenPrompt("<bash-input>unclosed")).toBe(false);
  });
});

describe("threadTurnEnds", () => {
  it("puts each finished turn's end on the last row before the next prompt", () => {
    const rows = [user("u0", "a"), reply("r0a"), reply("r0b"), user("u1", "b"), reply("r1"),
      user("u2", "c")];
    const turns = [turn(0, "a"), turn(1, "b"), turn(2, "c")];
    const ends = threadTurnEnds(rows, matchThreadTurns(threadPromptRows(rows), turns, true));
    expect([...ends].map(([key, value]) => [key, value.ordinal])).toEqual([["r0b", 0], ["r1", 1]]);
  });

  it("lets a turn run past an interrupt marker or a bash echo to its last answer", () => {
    const rows = [user("u0", "a"), reply("r0"), user("m", "[Request interrupted by user]"),
      reply("r0late"), user("b", "<bash-input>pwd</bash-input>"), user("u1", "b"), reply("r1")];
    const turns = [turn(0, "a"), turn(1, "b")];
    const ends = threadTurnEnds(rows, matchThreadTurns(threadPromptRows(rows), turns, true));
    expect([...ends].map(([key, value]) => [key, value.ordinal])).toEqual([["r0late", 0], ["r1", 1]]);
  });

  it("offers no end for a running turn or an unmatched prompt", () => {
    const rows = [user("u0", "a"), reply("r0"), user("u1", "unknown"), reply("r1"),
      user("u2", "b"), reply("r2")];
    const turns = [turn(0, "a"), turn(1, "b", { state: "running", hasEnd: false })];
    const ends = threadTurnEnds(rows, matchThreadTurns(threadPromptRows(rows), turns, true));
    expect([...ends.keys()]).toEqual(["r0"]);
  });
});

describe("transcriptWasCompacted", () => {
  it("recognises the live log's compaction notice", () => {
    const notice: TranscriptRow = { kind: "item", key: "l:n", origin: "live",
      item: { kind: "notice", id: "n", tone: "warning",
        text: COMPACTED_NOTICE.kind === "notice" ? COMPACTED_NOTICE.message : "" } };
    expect(transcriptWasCompacted([notice, user("u", "a")])).toBe(true);
    expect(transcriptWasCompacted([user("u", "a")])).toBe(false);
  });
});

describe("threadTurnChangeText", () => {
  it("states what Rust measured and stays silent about the rest", () => {
    expect(threadTurnChangeText({ added: 41, removed: 12, filesChanged: 3 })).toBe("+41 −12 · 3 files");
    expect(threadTurnChangeText({ added: 1, removed: 0, filesChanged: 1 })).toBe("+1 −0 · 1 file");
    expect(threadTurnChangeText({ added: 0, removed: 0, filesChanged: 0 })).toBe("");
    expect(threadTurnChangeText({})).toBe("");
  });
});

describe("the diff panel's Undo, found by turn", () => {
  const point = { label: "Undo · 3 files", description: "Restores 3 files." };

  it("finds the restore point on the row the pane matched the turn to", () => {
    const rows = [user("h:u0", "a"), reply("h:r0"), user("l:u1", "b"), reply("l:r1")];
    const rowKeys = threadTurnRowKeys(matchThreadTurns(threadPromptRows(rows),
      [turn(0, "a"), turn(1, "b")], true));
    expect([...rowKeys]).toEqual([[0, "h:u0"], [1, "l:u1"]]);
    expect(turnRestoreForOrdinal(1, rowKeys, new Map([["l:u1", point]])))
      .toEqual({ turnKey: "l:u1", point });
  });

  it("follows the send's echo as it settles into its live row", () => {
    const points = new Map([["l:u1", point]]);
    // The pane's join while the send was still its local echo: the shell has
    // already moved the point to the settled row, so the stale join offers nothing.
    expect(turnRestoreForOrdinal(1, new Map([[1, "e:0"]]), points)).toBeNull();
    // The pane's next report, once the echo settled: the same Undo again.
    expect(turnRestoreForOrdinal(1, new Map([[1, "l:u1"]]), points)?.turnKey).toBe("l:u1");
  });

  it("offers nothing where the transcript would offer nothing", () => {
    const rowKeys = new Map([[0, "h:u0"]]);
    // No row for the turn (a prompt typed in the terminal), no point on the
    // row (nothing captured), and a pane that never reported.
    expect(turnRestoreForOrdinal(1, rowKeys, new Map([["h:u0", point]]))).toBeNull();
    expect(turnRestoreForOrdinal(0, rowKeys, new Map())).toBeNull();
    expect(turnRestoreForOrdinal(0, undefined, new Map([["h:u0", point]]))).toBeNull();
    expect(turnRestoreForOrdinal(0, rowKeys, undefined)).toBeNull();
  });

  it("never pairs a turn with a row the join dropped as a crossing", () => {
    const rows = [user("u0", "b"), reply("r0"), user("u1", "a"), reply("r1")];
    const rowKeys = threadTurnRowKeys(matchThreadTurns(threadPromptRows(rows),
      [turn(0, "a"), turn(1, "b")], true));
    expect(rowKeys.size).toBeLessThan(2);
  });
});

describe("threadRetryKey", () => {
  const retryOf = (rows: TranscriptRow[], turns: ThreadTurn[]): string | null =>
    threadRetryKey(rows, matchThreadTurns(threadPromptRows(rows), turns, true), turns);

  it("offers Continue on the newest turn when Rust recorded it failed", () => {
    const rows = [user("u0", "a"), reply("r0"), user("u1", "Fix the login test")];
    expect(retryOf(rows, [turn(0, "a"), turn(1, "Fix the login test", { state: "failed" })])).toBe("u1");
  });

  it("offers nothing for a running or finished newest turn", () => {
    const rows = [user("u0", "a"), reply("r0"), user("u1", "b")];
    for (const state of ["running", "done"] as const) {
      expect(retryOf(rows, [turn(0, "a"), turn(1, "b", { state })])).toBeNull();
    }
  });

  it("offers Continue for an interrupted newest turn without replaying an older turn", () => {
    expect(retryOf([user("u0", "a")], [turn(0, "a", { state: "interrupted" })])).toBe("u0");
  });

  it("never offers an older failure once a later turn was sent", () => {
    // Retrying turn 0 now would replay it after turn 1, out of order.
    const rows = [user("u0", "a"), reply("r0"), user("u1", "b"), reply("r1")];
    expect(retryOf(rows, [turn(0, "a", { state: "failed" }), turn(1, "b")])).toBeNull();
  });

  it("offers nothing when the newest turn has no row on screen", () => {
    // A send recorded by Rust whose prompt the transcript does not show yet.
    const rows = [user("u0", "a"), reply("r0")];
    expect(retryOf(rows, [turn(0, "a"), turn(1, "b", { state: "failed" })])).toBeNull();
  });

  it("never offers a provider command", () => {
    const rows = [user("u0", "/compact")];
    expect(retryOf(rows, [turn(0, "/compact", { state: "failed" })])).toBeNull();
  });
});
