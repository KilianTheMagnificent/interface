import { describe, expect, it } from "vitest";
import type { EngineEvent } from "../shared/engine";
import { appReducer, initialState } from "./store";
import { conversationItems } from "./conversation-reducer";
import { eventLineDelta, lineDelta } from "./diff-stats";
import {
  appendLiveEvents, compactLiveLog, COMPACTED_NOTICE, liveEventOrdinal, liveHistoryBytes,
  LIVE_COMPACTION_LOW_WATER, LIVE_HISTORY_LIMITS,
} from "./live-events";

describe("display history byte retention", () => {
  const limits = { sessionBytes: 4096, totalBytes: 8192, textBytes: 1024, events: 100 };
  it("bounds an uninterrupted Unicode stream within one batch and preserves its live tail", () => {
    const items = Array.from({ length: 4096 }, () => ({ key: "a",
      evt: { kind: "message.delta", text: "😀界".repeat(100) } as EngineEvent }));
    const log = appendLiveEvents({}, items, limits).a;
    expect(log[0]).toMatchObject({ kind: "notice", tone: "warning" });
    const tail = log.at(-1) as Extract<EngineEvent, { kind: "message.delta" }>;
    expect(new TextDecoder().decode(new TextEncoder().encode(tail.text))).toBe(tail.text);
    expect(tail.text.endsWith("😀界")).toBe(true);
    expect(tail.text.length * 2).toBeLessThanOrEqual(limits.textBytes);
    expect(liveHistoryBytes(log)).toBeLessThanOrEqual(limits.sessionBytes);
  });
  it("bounds interleaved agents and aggregate disposable history", () => {
    const items = Array.from({ length: 1000 }, (_, i) => ({ key: `session-${i % 16}`,
      evt: { kind: "message.delta", agentKey: `agent-${i % 3}`, text: "x".repeat(400) } as EngineEvent }));
    const logs = appendLiveEvents({}, items, limits);
    expect(Object.values(logs).reduce((sum, log) => sum + liveHistoryBytes(log), 0))
      .toBeLessThanOrEqual(limits.totalBytes);
  });
  it("keeps unresolved approval input and current lifecycle facts across compaction", () => {
    const approval: EngineEvent = { kind: "approval.requested", approvalId: "approval", tool: "test",
      summary: "Review exact input", input: { text: "x".repeat(10_000) } };
    const status: EngineEvent = { kind: "status.changed", status: "waiting" };
    const items = [approval, status, ...Array.from({ length: 200 }, () =>
      ({ kind: "notice", tone: "info", message: "x".repeat(200) } as EngineEvent))]
      .map(evt => ({ key: "a", evt }));
    const log = appendLiveEvents({}, items, limits).a;
    expect(log).toContain(approval);
    expect(log).toContain(status);
    const closed = appendLiveEvents({ a: log }, [{ key: "a", evt: { kind: "session.closed" } }], limits).a;
    expect(closed).not.toContain(approval);
    expect(closed.at(-1)).toMatchObject({ kind: "session.closed" });
  });
  it("does not re-compact a compacted log whose disposable history still fits", () => {
    // The unresolved approval is reserve, not display history: it alone holds
    // the log over the byte cap after compaction, so every later frame used to
    // run compaction again only to copy the same events.
    const approval: EngineEvent = { kind: "approval.requested", approvalId: "approval", tool: "test",
      summary: "Review exact input", input: { text: "x".repeat(10_000) } };
    const items = [approval, ...Array.from({ length: 5 }, () =>
      ({ kind: "notice", tone: "info", message: "small" } as EngineEvent))].map(evt => ({ key: "a", evt }));
    const log = appendLiveEvents({}, items, limits).a;
    // An uncompacted log over the caps only through its reserve still gets the notice.
    expect(log[0]).toBe(COMPACTED_NOTICE);
    expect(log).toHaveLength(7);
    expect(liveHistoryBytes(log)).toBeGreaterThan(limits.sessionBytes);
    expect(compactLiveLog(log, limits.sessionBytes, limits.events)).toBe(log);
    const more: EngineEvent = { kind: "notice", tone: "info", message: "more" };
    const next = appendLiveEvents({ a: log }, [{ key: "a", evt: more }], limits).a;
    expect(next).toEqual([...log, more]);
    expect(compactLiveLog(next, limits.sessionBytes, limits.events)).toBe(next);
  });
  it("leaves compaction's own output alone and still drops history that no longer fits", () => {
    for (const events of [5, 40, 100]) {
      for (const budget of [1024, 4096]) {
        const log = [
          { kind: "approval.requested", approvalId: "a", tool: "t", summary: "s", input: { text: "y".repeat(3000) } },
          { kind: "status.changed", status: "waiting" },
          ...Array.from({ length: 300 }, (_, i) => ({ kind: "notice", tone: "info", message: "z".repeat(i % 97) })),
        ] as EngineEvent[];
        const once = compactLiveLog(log, budget, events);
        expect(once).not.toBe(log);
        expect(compactLiveLog(once, budget, events)).toBe(once);
        const grown = [...once, { kind: "notice", tone: "info", message: "w".repeat(budget) } as EngineEvent];
        expect(compactLiveLog(grown, budget, events)).not.toBe(grown);
      }
    }
  });
  it("compacts a log at its event cap to the low-water mark, then appends without compacting", () => {
    const counted = { ...limits, sessionBytes: 1 << 30, totalBytes: 1 << 30 };
    const notice = (i: number): EngineEvent => ({ kind: "notice", tone: "info", message: `n${i}` });
    let log = appendLiveEvents({}, Array.from({ length: counted.events + 1 }, (_, i) => ({ key: "a", evt: notice(i) })),
      counted).a;
    // The notice takes one of the low-water slots, as it takes one of the caps.
    const low = Math.floor(counted.events * LIVE_COMPACTION_LOW_WATER);
    expect(log[0]).toBe(COMPACTED_NOTICE);
    expect(log).toHaveLength(low);
    expect(log.at(-1)).toMatchObject({ message: `n${counted.events}` });
    let compactions = 0;
    let appended = 0;
    for (let i = 0; i < 3 * (counted.events - low + 1); i++) {
      const next = appendLiveEvents({ a: log }, [{ key: "a", evt: notice(1000 + i) }], counted).a;
      // An append keeps every earlier event by reference; a compaction drops the head.
      if (next.length === log.length + 1 && log.every((event, index) => next[index] === event)) appended++;
      else {
        compactions++;
        expect(next[0]).toBe(COMPACTED_NOTICE);
        expect(next).toHaveLength(low);
      }
      log = next;
    }
    // Each compaction buys the headroom between the low-water mark and the cap.
    expect(compactions).toBe(3);
    expect(appended).toBe(3 * (counted.events - low));
  });
  it("keeps the cut to the low-water mark when the reserve holds a compacted log over its byte cap", () => {
    const approval: EngineEvent = { kind: "approval.requested", approvalId: "approval", tool: "test",
      summary: "Review exact input", input: { text: "x".repeat(10_000) } };
    const notice = (i: number): EngineEvent => ({ kind: "notice", tone: "info", message: `${i}`.padEnd(40, "z") });
    const capped = { ...limits, sessionBytes: 16_384, totalBytes: 1 << 30 };
    let log = appendLiveEvents({}, [approval, ...Array.from({ length: 200 }, (_, i) => notice(i))]
      .map(evt => ({ key: "a", evt })), capped).a;
    expect(log).toContain(approval);
    let compactions = 0;
    for (let i = 0; i < 200; i++) {
      const next = appendLiveEvents({ a: log }, [{ key: "a", evt: notice(1000 + i) }], capped).a;
      expect(liveHistoryBytes(next)).toBeGreaterThan(capped.sessionBytes);
      if (!log.every((event, index) => next[index] === event)) compactions++;
      expect(next).toContain(approval);
      log = next;
    }
    // Every one of these frames is over the byte cap through the reserve, yet
    // disposable history refills from 90% to 100% of the cap between cuts.
    const headroom = Math.floor((capped.sessionBytes * (1 - LIVE_COMPACTION_LOW_WATER))
      / liveHistoryBytes([notice(1000)]));
    expect(headroom).toBeGreaterThanOrEqual(9);
    expect(compactions).toBeGreaterThan(0);
    expect(compactions).toBeLessThanOrEqual(Math.ceil(200 / headroom));
  });
  it("drops accounting with deleted history and accepts fresh events without old omissions", () => {
    const loaded = appReducer(initialState, { type: "event", key: "a", evt: {
      kind: "message.delta", text: "x".repeat(LIVE_HISTORY_LIMITS.textBytes),
    } });
    const dropped = appReducer(loaded, { type: "drop", key: "a" });
    expect(dropped.events.a).toBeUndefined();
    const fresh = appReducer(dropped, { type: "event", key: "a", evt: { kind: "message.delta", text: "fresh" } });
    expect(fresh.events.a).toEqual([{ kind: "message.delta", text: "fresh" }]);
  });
});

describe("live event ordinals", () => {
  const limits = { sessionBytes: 1 << 30, totalBytes: 1 << 30, textBytes: 64, events: 10_000 };
  const ordinalsOf = (log: EngineEvent[]): number[] =>
    log.map((event, index) => liveEventOrdinal(event, index));
  it("are the indices of a log that was never compacted, and survive the omission notice", () => {
    const user: EngineEvent = { kind: "user.message", text: "go" };
    let log = appendLiveEvents({}, [{ key: "a", evt: user },
      { key: "a", evt: { kind: "message.delta", text: "short" } }], limits).a;
    expect(ordinalsOf(log)).toEqual([0, 1]);
    // A delta over the text limit is clipped, and the notice goes in front.
    log = appendLiveEvents({ a: log }, [{ key: "a", evt: { kind: "message.delta", text: "x".repeat(100) } },
      { key: "a", evt: { kind: "notice", tone: "info", message: "after" } }], limits).a;
    expect(log[0]).toBe(COMPACTED_NOTICE);
    expect(ordinalsOf(log)).toEqual([-1, 0, 1, 2]);
  });
  it("stay unique when one event object is appended twice", () => {
    const again: EngineEvent = { kind: "notice", tone: "info", message: "twice" };
    const log = appendLiveEvents({}, [{ key: "a", evt: again }, { key: "a", evt: again }], limits).a;
    expect(log).toEqual([again, again]);
    expect(ordinalsOf(log)).toEqual([0, 1]);
  });
});

describe("diffs in the live log", () => {
  it("keeps a diff's line count and path, not its file bodies", () => {
    const before = Array.from({ length: 400 }, (_, line) => `line ${line}`).join("\n");
    const after = `${before}\nadded one\nadded two`;
    const diff: EngineEvent = { kind: "diff.produced", callId: "write-1", path: "/repo/big.ts",
      before, after, agentKey: "agent-1" };
    const log = appendLiveEvents({}, [{ key: "a", evt: diff }]).a;
    const stored = log[0] as Extract<EngineEvent, { kind: "diff.produced" }>;
    expect(stored).toEqual({ ...diff, before: "", after: "" });
    expect(eventLineDelta(stored)).toEqual(lineDelta(before, after));
    expect(liveHistoryBytes(log)).toBeLessThan(liveHistoryBytes([diff]) / 10);
    expect(conversationItems(log)).toEqual(conversationItems([diff]));
  });
});
