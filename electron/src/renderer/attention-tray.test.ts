import { describe, expect, it } from "vitest";
import type { AttentionItem } from "../shared/harness";
import {
  ATTENTION_EVENT_CAP, EMPTY_ATTENTION, attentionPlace, attentionRowMeta, attentionRows,
  attentionTrayLabel, attentionUnreadCount, attentionWord, clearEvents, dismissEvent,
  harnessAttentionEvents, markOpened, recordEvent, trackNeeds, unreadCount,
  type AttentionRow, type LiveNeed,
} from "./attention-tray";

const need = (id: string, title = id): LiveNeed => ({
  id: `session:${id}`, target: { kind: "session", key: `claude#${id}` }, title, where: "interface",
});
const event = (id: string, at: number, kind: AttentionRow["kind"] = "finished"): Omit<AttentionRow, "live"> => ({
  id: `done:${id}:${at.toString()}`, kind, target: { kind: "session", key: `claude#${id}` },
  title: id, where: "ledger", at,
});

describe("the attention tray's live needs", () => {
  it("remembers when each need began and forgets the ones that ended", () => {
    const a = trackNeeds(EMPTY_ATTENTION, [need("a")], 1_000);
    expect(a.since).toEqual({ "session:a": 1_000 });
    // Still waiting a minute later: the first-seen time stands.
    const b = trackNeeds(a, [need("a"), need("b")], 61_000);
    expect(b.since).toEqual({ "session:a": 1_000, "session:b": 61_000 });
    // Answered: gone, without touching the other.
    const c = trackNeeds(b, [need("b")], 90_000);
    expect(c.since).toEqual({ "session:b": 61_000 });
  });

  it("returns the same state when nothing changed, so an effect can run every render", () => {
    const a = trackNeeds(EMPTY_ATTENTION, [need("a")], 1_000);
    expect(trackNeeds(a, [need("a")], 5_000)).toBe(a);
    expect(trackNeeds(EMPTY_ATTENTION, [], 5_000)).toBe(EMPTY_ATTENTION);
  });

  it("draws live rows from the needs and the times it remembered, newest first", () => {
    const state = trackNeeds(trackNeeds(EMPTY_ATTENTION, [need("a")], 1_000), [need("a"), need("b")], 2_000);
    const rows = attentionRows(state, [need("a"), need("b")]);
    expect(rows.map(row => [row.id, row.kind, row.at, row.live])).toEqual([
      ["session:b", "needs-input", 2_000, true],
      ["session:a", "needs-input", 1_000, true],
    ]);
  });
});

describe("the attention tray's events", () => {
  it("prepends, replaces a repeat, and keeps a bounded list", () => {
    let state = recordEvent(EMPTY_ATTENTION, event("a", 1_000));
    state = recordEvent(state, event("b", 2_000, "failed"));
    expect(state.events.map(row => row.id)).toEqual(["done:b:2000", "done:a:1000"]);
    expect(state.events[0].live).toBe(false);
    state = recordEvent(state, event("a", 1_000));
    expect(state.events.map(row => row.id)).toEqual(["done:a:1000", "done:b:2000"]);
    for (let i = 0; i < ATTENTION_EVENT_CAP + 5; i++) state = recordEvent(state, event("x", 10_000 + i));
    expect(state.events).toHaveLength(ATTENTION_EVENT_CAP);
    expect(state.events[0].id).toBe(`done:x:${(10_000 + ATTENTION_EVENT_CAP + 4).toString()}`);
  });

  it("dismisses one, clears all, and is a no-op when there is nothing to do", () => {
    const state = recordEvent(recordEvent(EMPTY_ATTENTION, event("a", 1_000)), event("b", 2_000));
    expect(dismissEvent(state, "done:a:1000").events.map(row => row.id)).toEqual(["done:b:2000"]);
    expect(dismissEvent(state, "nope")).toBe(state);
    expect(clearEvents(state).events).toEqual([]);
    expect(clearEvents(EMPTY_ATTENTION)).toBe(EMPTY_ATTENTION);
  });

  it("interleaves events with live needs by time, and leaves live rows to clear themselves", () => {
    let state = recordEvent(EMPTY_ATTENTION, event("done", 1_500));
    state = trackNeeds(state, [need("a")], 1_000);
    state = trackNeeds(state, [need("a"), need("b")], 2_000);
    const rows = attentionRows(state, [need("a"), need("b")]);
    expect(rows.map(row => row.id)).toEqual(["session:b", "done:done:1500", "session:a"]);
    // The need for "a" is answered: its row is simply not built. The event stays.
    expect(attentionRows(state, [need("b")]).map(row => row.id)).toEqual(["session:b", "done:done:1500"]);
  });
});

describe("the attention tray's words", () => {
  it("counts unread against the last open, and clears the dot on open", () => {
    let state = recordEvent(EMPTY_ATTENTION, event("a", 1_000));
    state = recordEvent(state, event("b", 3_000));
    const rows = attentionRows(state, []);
    expect(unreadCount(rows, 0)).toBe(2);
    state = markOpened(state, 2_000);
    expect(unreadCount(rows, state.openedAt)).toBe(1);
    expect(markOpened(state, 2_000)).toBe(state);
  });

  it("uses the shared status words and names the bell by count and novelty", () => {
    expect(attentionWord("needs-input")).toBe("Needs attention");
    expect(attentionWord("failed")).toBe("Failed");
    expect(attentionWord("finished")).toBe("Finished");
    expect(attentionTrayLabel(0, 0)).toBe("Attention: nothing needs you");
    expect(attentionTrayLabel(1, 0)).toBe("Attention: 1 item");
    expect(attentionTrayLabel(3, 2)).toBe("Attention: 3 items, 2 new");
  });

  it("spells the row's meta from the parts it knows, with no dash for the rest", () => {
    const now = Date.parse("2026-09-05T12:04:00.000Z");
    const row: AttentionRow = { ...event("a", now - 4 * 60_000), live: false };
    expect(attentionRowMeta(row, now)).toBe("Finished · ledger · 4m");
    expect(attentionRowMeta({ ...row, where: "", at: 0 }, now)).toBe("Finished");
    expect(attentionPlace("/Users/me/Desktop/Projects/Interface-Rust")).toBe("Interface-Rust");
    expect(attentionPlace(null)).toBe("");
    // The shared basename rule: trailing separators drop, a backslash is one
    // too, and a path with no segment names nothing.
    expect(attentionPlace("/Users/me/work/ledger/")).toBe("ledger");
    expect(attentionPlace("C:\\work\\ledger")).toBe("ledger");
    expect(attentionPlace("/")).toBe("");
  });
});

describe("the attention tray's harness history", () => {
  const item = (id: string, at: number, over: Partial<AttentionItem> = {}): AttentionItem => ({
    id, kind: "finished", target: { kind: "session", key: `claude#${id}` }, title: id,
    place: "ledger", at, provenance: "provider", seen: false, resolved: false, ...over,
  });

  it("shows unresolved history newest first, capped, with interrupted turns as failed", () => {
    const attention = [
      item("old", 1), item("done", 2, { resolved: true }), item("asks", 3, { kind: "needs-input" }),
      item("cut", 4, { kind: "interrupted", target: { kind: "terminal", terminalId: "t1", generation: 2 } }),
      item("new", 5),
    ];
    expect(harnessAttentionEvents({ attention })).toEqual([
      { id: "new", kind: "finished", target: { kind: "session", key: "claude#new" }, title: "new",
        where: "ledger", at: 5, live: false },
      { id: "cut", kind: "failed", target: { kind: "terminal", terminalId: "t1", generation: 2 },
        title: "cut", where: "ledger", at: 4, live: false },
      { id: "old", kind: "finished", target: { kind: "session", key: "claude#old" }, title: "old",
        where: "ledger", at: 1, live: false },
    ]);
    const many = Array.from({ length: ATTENTION_EVENT_CAP + 5 }, (_, i) => item(`e${i.toString()}`, i));
    const rows = harnessAttentionEvents({ attention: many });
    expect(rows).toHaveLength(ATTENTION_EVENT_CAP);
    expect(rows[0].id).toBe(`e${(ATTENTION_EVENT_CAP + 4).toString()}`);
  });

  it("counts the harness's unseen items plus new live rows once there is a harness read", () => {
    const live: AttentionRow = { id: "session:a", kind: "needs-input",
      target: { kind: "session", key: "claude#a" }, title: "a", where: "", at: 20, live: true };
    const local: AttentionRow = { ...live, id: "done:b", kind: "finished", live: false, at: 30 };
    const rows = [live, local];
    expect(attentionUnreadCount(null, rows, 10)).toBe(2);
    expect(attentionUnreadCount(null, rows, 25)).toBe(1);
    const view = { state: { version: 1 as const, revision: 1, tasks: [], drafts: [], branches: [],
      attention: [item("x", 1), item("y", 2, { seen: true }), item("z", 3, { resolved: true })] } };
    // One unseen harness item, plus the live row newer than the last open.
    expect(attentionUnreadCount(view, rows, 10)).toBe(2);
    expect(attentionUnreadCount(view, rows, 25)).toBe(1);
  });
});
