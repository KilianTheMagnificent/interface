import { describe, expect, it } from "vitest";
import {
  conversationProjectionFor, createConversationProjection, onlyStreamedTextSince,
  releaseConversationProjection, retainConversationItems,
} from "./conversation-projection";
import { conversationItems, liveItemOrdinal, type ConvItem } from "./conversation-reducer";
import { echoPosition, mergeTranscript, newPendingEcho, settledEchoRowKey } from "./transcript-merge";
import type { EngineEvent } from "../shared/engine";
import { historyPageItems, retainHistoryPage } from "./history-page-projection";
import { appendLiveEvents, COMPACTED_NOTICE } from "./live-events";

it("reuses deserialized history prefixes and supports an array that grows in place", () => {
  const project = createConversationProjection();
  const events: EngineEvent[] = [{ kind: "user.message", text: "hello" }];
  const first = project(events);
  events.push({ kind: "message.delta", text: "answer" });
  expect(project(events)).toEqual(conversationItems(events));
  expect(project(events)[0]).toBe(first[0]);
  const page = { events, messages: 2, olderMessages: 0 };
  expect(retainHistoryPage(page, structuredClone(page))).toBe(page);
  const next = structuredClone(page);
  next.events[1] = { kind: "message.delta", text: "answer grows" };
  const retained = retainHistoryPage(page, next);
  expect(retained.events[0]).toBe(events[0]);
  expect(project(retained.events)).toEqual(conversationItems(next.events));
});

it("matches the reference across tail replacement, compaction, tools and out-of-order nesting", () => {
  const project = createConversationProjection();
  let events: EngineEvent[] = [];
  const check = (next: EngineEvent[]): void => {
    const old = project(events);
    const frozen = JSON.stringify(old);
    expect(project(next)).toEqual(conversationItems(next));
    expect(JSON.stringify(old)).toBe(frozen);
    events = next;
  };
  const stream: EngineEvent[] = [
    { kind: "user.message", text: "question" },
    { kind: "message.delta", text: "first" },
    { kind: "message.delta", text: " second" },
    { kind: "tool.call", callId: "spawn", tool: "Agent", summary: "child", input: {} },
    { kind: "message.delta", agentKey: "spawn", text: "early child" },
    { kind: "subagent.started", agentKey: "spawn", subagentType: "reviewer", description: "review" },
    { kind: "tool.result", callId: "spawn", ok: true, summary: "done" },
    { kind: "subagent.finished", agentKey: "spawn", ok: true, summary: "finished" },
    { kind: "session.closed" },
  ];
  for (const event of stream) {
    check([...events, event]);
    if (event.kind === "message.delta") check([...events.slice(0, -1), { ...event, text: event.text + " tail" }]);
  }
  check(events.slice(-3));
  check([{ kind: "message.delta", text: "new history" }]);
  check([...events, { kind: "commands.available", commands: [] }, { kind: "message.delta", text: " more" }]);
  check([]);
});

it("preserves unchanged rows while the full reducer remains authoritative", () => {
  const project = createConversationProjection();
  const events: EngineEvent[] = [
    { kind: "user.message", text: "hello" },
    { kind: "message.delta", text: "hi" },
  ];
  const before = project(events);
  const nextEvents: EngineEvent[] = [...events, { kind: "message.delta", text: " there" }];
  const after = project(nextEvents);
  expect(after).toEqual(conversationItems(nextEvents));
  expect(after[0]).toBe(before[0]);
  expect(after[1]).not.toBe(before[1]);
  expect(project(nextEvents)).toBe(after);
  expect(project([])).toEqual([]);
});

it("preserves nested identities, respects ordering, and never mutates old rows", () => {
  const child: ConvItem = { kind: "text", id: "child", text: "settled", streaming: false };
  const agent: ConvItem = { kind: "subagent", id: "agent", subagentType: "reviewer", description: "Review", status: "running", children: [child], toolCount: 0, subagentCount: 0 };
  const after = retainConversationItems([agent], [{ ...agent, children: [{ ...child }, { ...child, id: "new" }] }]);
  expect(after[0]).not.toBe(agent);
  expect(after[0].kind === "subagent" && after[0].children[0]).toBe(child);
  expect(agent.children).toEqual([child]);
});

it("shares one projection per session, idempotent for the log it just saw", () => {
  const key = "codex#shared-projection";
  const events: EngineEvent[] = [
    { kind: "user.message", text: "question" },
    { kind: "tool.call", callId: "spawn", tool: "Task", summary: "child", input: {} },
    { kind: "subagent.started", agentKey: "spawn", subagentType: "reviewer", description: "review" },
    { kind: "message.delta", agentKey: "spawn", text: "child working" },
  ];
  const project = conversationProjectionFor(key);
  expect(conversationProjectionFor(key)).toBe(project);
  // The subagent column calls first, the pane second, in one frame: the second
  // call gets the very same list back rather than a second reduce.
  const first = project(events);
  expect(conversationProjectionFor(key)(events)).toBe(first);
  expect(first).toEqual(conversationItems(events));
  // A card the next frame did not touch keeps its object.
  const next: EngineEvent[] = [...events, { kind: "message.delta", text: "main thread" }];
  const after = conversationProjectionFor(key)(next);
  expect(after).toEqual(conversationItems(next));
  expect(after[0]).toBe(first[0]);
  expect(after[1]).toBe(first[1]);
  releaseConversationProjection(key);
  expect(conversationProjectionFor(key)).not.toBe(project);
  releaseConversationProjection(key);
});

it("keeps at most 32 shared projections, dropping the least recently used", () => {
  const keys = Array.from({ length: 33 }, (_, index) => `codex#bounded-${index}`);
  const first = conversationProjectionFor(keys[0]);
  const second = conversationProjectionFor(keys[1]);
  for (const key of keys.slice(2, 32)) conversationProjectionFor(key);
  expect(conversationProjectionFor(keys[0])).toBe(first); // used again: now the newest
  conversationProjectionFor(keys[32]);
  expect(conversationProjectionFor(keys[0])).toBe(first);
  expect(conversationProjectionFor(keys[1])).not.toBe(second);
  for (const key of keys) releaseConversationProjection(key);
});

it("tells streamed text apart from every other change a covered pane must draw", () => {
  const user: EngineEvent = { kind: "user.message", text: "hello" };
  const delta: EngineEvent = { kind: "message.delta", text: "hi" };
  const held: EngineEvent[] = [user, delta];
  expect(onlyStreamedTextSince(held, held)).toBe(true);
  // Appended deltas, including a subagent's, and the tail delta merged longer.
  expect(onlyStreamedTextSince(held, [user, delta, { kind: "message.delta", text: " more" }])).toBe(true);
  expect(onlyStreamedTextSince(held, [user, { kind: "message.delta", text: "hi there" }])).toBe(true);
  expect(onlyStreamedTextSince(held,
    [user, delta, { kind: "message.delta", agentKey: "spawn", text: "child" }])).toBe(true);
  expect(onlyStreamedTextSince([], [delta])).toBe(true);
  // Anything else lands: a tool, an approval, a status, a replaced or dropped event.
  expect(onlyStreamedTextSince(held, [...held,
    { kind: "tool.call", callId: "c", tool: "Read", summary: "a", input: {} }])).toBe(false);
  expect(onlyStreamedTextSince(held, [...held,
    { kind: "approval.requested", approvalId: "a", tool: "Bash", summary: "rm", input: {} }])).toBe(false);
  expect(onlyStreamedTextSince(held, [...held, { kind: "status.changed", status: "idle" }])).toBe(false);
  expect(onlyStreamedTextSince(held, [{ kind: "user.message", text: "hello" }, delta])).toBe(false);
  expect(onlyStreamedTextSince(held, [user])).toBe(false);
  expect(onlyStreamedTextSince([delta, user], [delta, { kind: "user.message", text: "edited" }]))
    .toBe(false);
});

describe("generated row ids across compaction and older history pages", () => {
  const limits = { sessionBytes: 1 << 30, totalBytes: 1 << 30, textBytes: 1 << 20, events: 40 };
  const turn = (n: number): EngineEvent[] => [
    { kind: "user.message", text: `question ${n}` },
    { kind: "message.delta", text: `answer ${n}` },
    { kind: "notice", tone: "info", message: `note ${n}` },
  ];
  const append = (log: EngineEvent[], events: EngineEvent[]): EngineEvent[] =>
    appendLiveEvents({ a: log }, events.map(evt => ({ key: "a", evt })), limits).a;
  const rowKeys = (items: ConvItem[]): Map<string, string> => new Map(
    mergeTranscript([], items).flatMap(row => row.kind === "item" && row.item.kind !== "tool"
      ? [[row.key, JSON.stringify({ ...row.item, id: undefined })] as const] : []));

  it("keeps every surviving row's key and item when compaction drops the log's head", () => {
    let log: EngineEvent[] = [];
    for (let n = 0; n < 13; n++) log = append(log, turn(n));
    expect(log[0]).not.toBe(COMPACTED_NOTICE);
    const project = createConversationProjection();
    const before = project(log);
    // An uncompacted log keeps the index ids it always had.
    expect(before.slice(0, 3).map(item => item.id)).toEqual(["user-0", "text-1", "notice-2"]);
    const keysBefore = rowKeys(before);

    const compacted = append(log, turn(13));
    expect(compacted[0]).toBe(COMPACTED_NOTICE);
    const after = project(compacted);
    expect(after).toEqual(conversationItems(compacted));
    let survivors = 0;
    for (const [key, item] of rowKeys(after)) {
      if (!keysBefore.has(key)) continue;
      survivors++;
      expect(item).toBe(keysBefore.get(key));
    }
    expect(survivors).toBeGreaterThan(20);
    // Items the compaction kept are the very objects drawn before it.
    const beforeById = new Map(before.map(item => [item.id, item]));
    expect(after.filter(item => beforeById.get(item.id) === item).length).toBe(survivors);
    expect(new Set(after.map(item => item.id)).size).toBe(after.length);
  });

  it("keeps a turn's settled row key on that turn after a compaction", () => {
    let log: EngineEvent[] = [];
    for (let n = 0; n < 12; n++) log = append(log, turn(n));
    const echo = newPendingEcho({ id: "7", text: "question 11", seen: 0, at: 33, historyKnown: true });
    const settled = settledEchoRowKey([], conversationItems(log), echo);
    expect(settled).toBe("l:user-33");
    const compacted = append(log, [...turn(12), ...turn(13)]);
    expect(compacted[0]).toBe(COMPACTED_NOTICE);
    const row = mergeTranscript([], conversationItems(compacted))
      .find(candidate => candidate.key === settled);
    expect(row?.kind === "item" ? row.item : null)
      .toMatchObject({ kind: "user", text: "question 11" });
    expect(settledEchoRowKey([], conversationItems(compacted), echo)).toBe(settled);
  });

  it("keeps a sent message's echo under the row it followed when compaction drops the head", () => {
    let log: EngineEvent[] = [];
    for (let n = 0; n < 12; n++) log = append(log, turn(n));
    const project = createConversationProjection();
    const drawn = project(log);
    const echo = newPendingEcho({ id: "e", text: "follow up", seen: 0,
      at: echoPosition(drawn), historyKnown: true });
    // Ordinals equal positions until the first compaction: the old count.
    expect(echo.at).toBe(drawn.length);
    const placed = (items: ConvItem[]): string[] => mergeTranscript([], items, [echo])
      .flatMap(row => row.kind !== "item" ? []
        : row.origin === "echo" ? ["ECHO"]
        : row.item.kind === "text" ? [row.item.text]
        : row.item.kind === "notice" ? [row.item.text] : []);
    // The reply streams in through the incremental path.
    log = append(log, [{ kind: "message.delta", text: "reply" }]);
    log = append(log, [{ kind: "message.delta", text: " continues" }]);
    expect(placed(project(log)).slice(-3)).toEqual(["note 11", "ECHO", "reply continues"]);
    log = append(log, [{ kind: "notice", tone: "info", message: "reply note" }, ...turn(12)]);
    expect(log[0]).toBe(COMPACTED_NOTICE);
    const after = project(log);
    expect(after).toEqual(conversationItems(log));
    expect(after.map((item, index) => liveItemOrdinal(item, index)))
      .toEqual(conversationItems(log).map((item, index) => liveItemOrdinal(item, index)));
    const order = placed(after);
    // A count would now point five rows further down, below the reply.
    expect(order.slice(order.indexOf("ECHO") - 1, order.indexOf("ECHO") + 3))
      .toEqual(["note 11", "ECHO", "reply continues", "reply note"]);
  });

  it("keeps the ids and items of the rows already drawn when an older page lands", () => {
    const events = Array.from({ length: 10 }, (_, n) => turn(n)).flat();
    const tail = structuredClone(events.slice(-9));
    const first = historyPageItems(tail);
    const older = historyPageItems(structuredClone(events), first);
    expect(older).toHaveLength(30);
    expect(older.slice(-9)).toEqual(first);
    older.slice(-9).forEach((item, index) => { expect(item).toBe(first[index]); });
    expect(new Set(older.map(item => item.id)).size).toBe(older.length);
  });
});
