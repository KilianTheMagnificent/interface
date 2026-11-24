import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { EngineEvent } from "../shared/engine";
import { HISTORY_LIMIT } from "../shared/ipc";
import { conversationItems } from "./conversation-reducer";
import { historyPageItems } from "./history-page-projection";
import {
  HistoryPagingController, IDLE_HISTORY, LOADING_HISTORY, frozenHistoryView, historyBeforeLive,
} from "./history-paging";
import { mergeTranscript } from "./transcript-merge";

const app = readFileSync(new URL("./App.tsx", import.meta.url), "utf8");

/* A send the HOST starts (a scheduled or watch prompt, a resume at reset, a
   team notice, an outside agent's send) can reach the window before it has
   read that thread's history: at launch the startup catch-up pass sends a
   schedule that came due while Interface was closed. Hands-free repro (review,
   15 Nov): the open thread then showed only the new turn, with every earlier
   message and visual card gone until a reload. */

const SCHEDULED = '[Interface] Scheduled prompt "restart2":\nappend restart2.txt fired after restart';

/** The provider's own page, read after the live turn had started. */
const PAGE: EngineEvent[] = [
  { kind: "user.message", text: "visual Sales chart" },
  { kind: "tool.call", callId: "call-1", tool: "mcp__iface_team__show_visual", summary: "Sales chart", input: {} },
  { kind: "tool.result", callId: "call-1", ok: true, summary: "v1-shown" },
  { kind: "visual.shown", id: "v1794781554560-8dbcd671", title: "Sales chart", bytes: 1370 },
  { kind: "message.delta", text: "Here is the chart." },
  { kind: "status.changed", status: "idle" },
  { kind: "user.message", text: "append ui.txt scheduled from the dialog" },
  { kind: "message.delta", text: "Appended a line to ui.txt." },
  { kind: "status.changed", status: "idle" },
  // The live turn, already in the provider's store when the page was read.
  { kind: "user.message", text: SCHEDULED },
  { kind: "message.delta", text: "Appended a line to restart2.txt." },
] as EngineEvent[];

/** What the window received live, before it read anything. */
const LIVE: EngineEvent[] = [
  { kind: "user.message", text: SCHEDULED },
  { kind: "message.delta", text: "Appended a line to restart2.txt." },
  { kind: "status.changed", status: "idle" },
] as EngineEvent[];

const texts = (rows: ReturnType<typeof mergeTranscript>) => rows.flatMap(row =>
  row.kind === "item" && (row.item.kind === "user" || row.item.kind === "text")
    ? [`${row.origin}:${row.item.kind}:${row.item.text}`] : []);

describe("a session that went live before its history was read", () => {
  it("still reads its history once and shows it in order, the live turn once", () => {
    const paging = new HistoryPagingController();
    const key = "claude#e3f7915b";
    // The first live event arrives with no read made (or one in flight): the
    // window freezes the session's history, as it always has.
    expect(paging.freeze(key)).toBe(true);
    expect(paging.begin(key)).toBeNull();
    // The pane opens: one late read is allowed, and it shows as loading, not
    // as an empty past (which is what it showed before).
    expect(paging.canReadLate(key)).toBe(true);
    expect(frozenHistoryView(undefined, paging.canReadLate(key))).toBe(LOADING_HISTORY);
    expect(frozenHistoryView(undefined)).toBe(IDLE_HISTORY);
    const generation = paging.beginLate(key);
    expect(generation).not.toBeNull();
    expect(paging.canReadLate(key)).toBe(false);
    expect(paging.beginLate(key)).toBeNull();
    expect(paging.isCurrent(key, generation ?? -1)).toBe(true);
    expect(paging.settle(key, generation ?? -1, HISTORY_LIMIT)).toBe(true);
    // Still frozen for everything else: no "Load older" over a live stream.
    expect(paging.nextLimit(key)).toBeNull();
    expect(paging.begin(key)).toBeNull();

    const history = historyPageItems(historyBeforeLive(PAGE, LIVE));
    const rows = mergeTranscript(history, conversationItems(LIVE));
    expect(texts(rows)).toEqual([
      "history:user:visual Sales chart",
      "history:text:Here is the chart.",
      "history:user:append ui.txt scheduled from the dialog",
      "history:text:Appended a line to ui.txt.",
      `live:user:${SCHEDULED}`,
      "live:text:Appended a line to restart2.txt.",
    ]);
    expect(rows.filter(row => row.kind === "item" && row.item.kind === "visual")).toHaveLength(1);
  });

  it("cuts at the right copy when the live prompt was also sent before", () => {
    const page = [...PAGE.slice(0, 6),
      { kind: "user.message", text: SCHEDULED }, { kind: "message.delta", text: "First run." }, { kind: "status.changed", status: "idle" },
      ...PAGE.slice(9)] as EngineEvent[];
    const history = historyPageItems(historyBeforeLive(page, LIVE));
    expect(texts(mergeTranscript(history, conversationItems(LIVE)))).toEqual([
      "history:user:visual Sales chart",
      "history:text:Here is the chart.",
      `history:user:${SCHEDULED}`,
      "history:text:First run.",
      `live:user:${SCHEDULED}`,
      "live:text:Appended a line to restart2.txt.",
    ]);
  });

  it("keeps the prompt of a turn the live stream joined midway, and only it", () => {
    const joined = [{ kind: "message.delta", text: "Appended a line to restart2.txt." }] as EngineEvent[];
    expect(historyBeforeLive(PAGE, joined)).toEqual(PAGE.slice(0, 10));
  });

  it("keeps the whole page when the live stream drew nothing or the page predates it", () => {
    expect(historyBeforeLive(PAGE, [{ kind: "status.changed", status: "idle" }] as EngineEvent[])).toEqual(PAGE);
    expect(historyBeforeLive(PAGE.slice(0, 9), LIVE)).toEqual(PAGE.slice(0, 9));
  });
});

describe("an ordinary live session is not read again", () => {
  it("allows no late read once its history was read, or when it started here", () => {
    const paging = new HistoryPagingController();
    const read = "claude#read";
    const generation = paging.begin(read);
    expect(paging.settle(read, generation ?? -1, HISTORY_LIMIT)).toBe(true);
    paging.freeze(read);
    expect(paging.canReadLate(read)).toBe(false);
    expect(paging.beginLate(read)).toBeNull();

    const born = "claude#born";
    paging.markStartedHere(born);
    expect(paging.canReadLate(born)).toBe(false);
    expect(paging.beginLate(born)).toBeNull();

    // A deleted key starts over.
    const gone = "claude#gone";
    paging.freeze(gone);
    paging.drop(gone);
    expect(paging.canReadLate(gone)).toBe(false);
  });

  it("App reads late only through the controller's one late read, cut to what predates the stream", () => {
    const loader = app.slice(app.indexOf("const loadHistory = useCallback"),
      app.indexOf("/* ---- session lifecycle"));
    expect(loader).toContain("historyPaging.current.beginLate(key)");
    expect(loader).toContain("historyBeforeLive(page.events, eventsRef.current[key] ?? NO_EVENTS)");
    // A late read keeps the frozen session's paging: no "Load older".
    expect(loader).toMatch(/canLoadOlder: !late &&/u);
  });
});
