import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { encodeRendererPayload } from "../main/ipc-payload";
import { handsfreeRefusal } from "../main/handsfree";
import { CHANNELS } from "./ipc";
import { noticesOf, storeNoticeText, type DiagnosticsCommand } from "./diagnostics";

const fixture = JSON.parse(readFileSync(
  new URL("../fixtures/diagnostics-v39.json", import.meta.url), "utf8",
)) as {
  channel: string;
  requests: DiagnosticsCommand[];
  responses: unknown[];
};

describe("diagnostics v39", () => {
  it("sends each command to Rust unchanged on its own channel", () => {
    expect(fixture.channel).toBe(CHANNELS.diagnostics);
    for (const request of fixture.requests) {
      expect(encodeRendererPayload(CHANNELS.diagnostics, [request])).toEqual(request);
    }
  });

  it("reads notices from an answer and words them the way the banner shows them", () => {
    expect(fixture.responses.map(noticesOf)).toEqual([
      [], [], [{ id: "threads-1", what: "your Threads list" }], [],
    ]);
    expect(noticesOf({ notices: [{ id: "", what: "x" }, { id: 3 }, "x"] })).toEqual([]);
    expect(storeNoticeText({ id: "threads-1", what: "your Threads list" }))
      .toBe("Interface couldn't read your Threads list and set the file aside.");
    expect(storeNoticeText({ id: "sessions-2", what: "your saved session names and archive marks" },
      "Interface Dev"))
      .toBe("Interface Dev couldn't read your saved session names and archive marks and set the file aside.");
  });

  it("hands-free and packaged-smoke sessions may read diagnostics but never open Finder", () => {
    expect(handsfreeRefusal(CHANNELS.diagnostics, { kind: "report" })).toBeUndefined();
    expect(handsfreeRefusal(CHANNELS.diagnostics, { kind: "notices" })).toBeUndefined();
    expect(handsfreeRefusal(CHANNELS.diagnostics, { kind: "reveal", target: "dataFolder" }))
      .toBe("reveal a file or folder in Finder");
  });
});
