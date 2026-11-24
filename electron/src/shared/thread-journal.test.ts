import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { encodeRendererPayload } from "../main/ipc-payload";
import { handsfreeRefusal } from "../main/handsfree";
import { CHANNELS } from "./ipc";
import {
  isVisualId, journalHitsOf, searchesMessages, threadVisualOf, type ThreadJournalCommand,
  type ThreadJournalResponse,
} from "./thread-journal";

const fixture = JSON.parse(readFileSync(
  new URL("../fixtures/thread-journal-v56.json", import.meta.url), "utf8",
)) as {
  channel: string;
  protocol: number;
  requests: ThreadJournalCommand[];
  lines: unknown[];
  responses: ThreadJournalResponse[];
};

describe("thread journal v56", () => {
  it("sends each command to Rust unchanged on its own channel", () => {
    expect(fixture.channel).toBe(CHANNELS.threadJournal);
    expect(fixture.protocol).toBe(56);
    expect(fixture.requests).toHaveLength(4);
    for (const request of fixture.requests) {
      expect(encodeRendererPayload(CHANNELS.threadJournal, [request])).toEqual(request);
    }
  });

  it("reads hits from each answer and round-trips the fixture's shapes", () => {
    expect(fixture.responses.map(journalHitsOf).map(hits => hits.length)).toEqual([0, 0, 2, 0]);
    const [first] = journalHitsOf(fixture.responses[2]);
    expect(first).toEqual({
      key: "claude#11111111-1111-4111-8111-111111111111", seq: 1, turn: 0, role: "prompt",
      excerpt: "Fix the flaky test", at: 1731630000000,
    });
    expect(JSON.parse(JSON.stringify(fixture.responses))).toEqual(fixture.responses);
    expect(fixture.lines).toHaveLength(19);
    expect(journalHitsOf({ hits: [{ key: "", excerpt: "x", seq: 1, role: "prompt" }, "x", { key: "a#b" }] }))
      .toEqual([]);
    expect(journalHitsOf(null)).toEqual([]);
  });

  it("searches messages from three characters", () => {
    expect(searchesMessages("ab")).toBe(false);
    expect(searchesMessages("  ab  ")).toBe(false);
    expect(searchesMessages("abc")).toBe(true);
  });

  it("hands-free sessions may read the journal", () => {
    expect(handsfreeRefusal(CHANNELS.threadJournal, { kind: "search", query: "x" })).toBeUndefined();
  });
});

const visuals = JSON.parse(readFileSync(
  new URL("../fixtures/visual-replies-v61.json", import.meta.url), "utf8",
)) as {
  channel: string;
  protocol: number;
  events: Array<{ kind: string; id: string; title: string; bytes: number }>;
  requests: ThreadJournalCommand[];
  refused: Array<{ kind: string; key?: string; id?: string }>;
  lines: Array<{ entry: { kind: string; id: string } }>;
  responses: ThreadJournalResponse[];
};

describe("visual replies v61", () => {
  it("asks for a visual on the journal channel unchanged", () => {
    expect(visuals.channel).toBe(CHANNELS.threadJournal);
    expect(visuals.protocol).toBe(61);
    for (const request of visuals.requests) {
      expect(encodeRendererPayload(CHANNELS.threadJournal, [request])).toEqual(request);
      expect(request.kind === "visual" && isVisualId(request.id)).toBe(true);
    }
    for (const refused of visuals.refused.filter(request => typeof request.id === "string")) {
      expect(isVisualId(refused.id) && refused.key !== "" && Object.keys(refused).length === 3)
        .toBe(false);
    }
  });

  it("reads the page only from a matching answer and round-trips the fixture's shapes", () => {
    const [first, empty] = visuals.responses;
    const id = visuals.requests[0]?.kind === "visual" ? visuals.requests[0].id : "";
    expect(threadVisualOf(first, id)).toContain("<svg");
    expect(threadVisualOf(first, "v0000000000000-00000000")).toBeNull();
    expect(threadVisualOf(empty, id)).toBeNull();
    expect(threadVisualOf(null, id)).toBeNull();
    expect(threadVisualOf({ visual: { id, html: 7 } }, id)).toBeNull();
    expect(visuals.events.every(event => event.kind === "visual.shown" && isVisualId(event.id))).toBe(true);
    expect(visuals.lines.map(line => line.entry.kind)).toEqual(["visual"]);
    expect(JSON.parse(JSON.stringify(visuals))).toEqual(visuals);
  });
});
