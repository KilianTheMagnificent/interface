import { expect, it, vi } from "vitest";
import type { EngineEvent } from "../shared/engine";
import { createConversationProjection } from "./conversation-projection";
import { conversationItems } from "./conversation-reducer";
import { appendLiveEvents, COMPACTED_NOTICE, liveHistoryBytes } from "./live-events";

// Counts the projection's full reductions; the append path never calls it.
vi.mock("./conversation-reducer", async importOriginal => {
  const actual = await importOriginal<typeof import("./conversation-reducer")>();
  return { ...actual, conversationItems: vi.fn(actual.conversationItems) };
});

it("streams a session at its byte cap through the projection's append path between compactions", () => {
  const limits = { sessionBytes: 256 * 1024, totalBytes: 1 << 30, textBytes: 1 << 20, events: 10_000 };
  const history: EngineEvent[] = Array.from({ length: 700 }, (_, i) =>
    ({ kind: "notice", tone: "info", message: `${i} `.padEnd(180, "h") }));
  let log = appendLiveEvents({}, [...history, { kind: "user.message", text: "go" } as EngineEvent]
    .map(evt => ({ key: "a", evt })), limits).a;
  const project = createConversationProjection();
  let items = project(log);
  const reductions = vi.mocked(conversationItems);
  const frames = 600;
  let compactions = 0;
  let settledKept = 0;
  reductions.mockClear();
  for (let frame = 0; frame < frames; frame++) {
    const next = appendLiveEvents({ a: log },
      [{ key: "a", evt: { kind: "message.delta", text: "t".repeat(100) } }], limits).a;
    // Appending keeps every earlier event by reference (the streamed delta is
    // replaced by its longer merge); a compaction drops the log's head.
    const appended = log.slice(0, -1).every((event, index) => next[index] === event);
    if (!appended) compactions++;
    const before = items;
    items = project(next);
    // Between compactions the settled rows are the same objects, frame to frame.
    if (appended && items.slice(0, before.length - 1).every((item, index) => item === before[index])) settledKept++;
    log = next;
  }
  expect(log[0]).toBe(COMPACTED_NOTICE);
  expect(liveHistoryBytes(log)).toBeLessThanOrEqual(limits.sessionBytes);
  // 600 frames of 200 bytes cross the cap several times, and each crossing cuts
  // to 90%: one compaction per ~26 KiB streamed, not one per frame at the cap.
  expect(compactions).toBeGreaterThanOrEqual(3);
  expect(compactions).toBeLessThanOrEqual(frames / 100);
  // Only a compaction takes the full reducer; every other frame appends.
  expect(reductions.mock.calls.length).toBe(compactions);
  expect(settledKept).toBe(frames - compactions);
  reductions.mockClear();
  expect(items).toEqual(conversationItems(log));
});
