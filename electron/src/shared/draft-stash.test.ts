import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { encodeRendererPayload } from "../main/ipc-payload";
import { handsfreeRefusal } from "../main/handsfree";
import { CHANNELS, type PromptContextValidation } from "./ipc";
import type { DraftStashCommand, DraftStashView } from "./draft-stash";

const fixture = JSON.parse(readFileSync(
  new URL("../fixtures/composer-references-v63.json", import.meta.url), "utf8",
)) as {
  channel: string;
  protocol: number;
  requests: DraftStashCommand[];
  refusedRequests: unknown[];
  view: DraftStashView;
  poppedView: DraftStashView;
  threadReference: PromptContextValidation;
};

const roundTrip = <T>(value: T): T => JSON.parse(JSON.stringify(value)) as T;

describe("composer references v63", () => {
  it("sends each stash command to Rust unchanged on its own channel", () => {
    expect(fixture.channel).toBe(CHANNELS.draftStash);
    expect(fixture.protocol).toBe(63);
    expect(fixture.requests).toHaveLength(5);
    for (const request of fixture.requests) {
      expect(encodeRendererPayload(CHANNELS.draftStash, [request])).toEqual(request);
    }
    expect(fixture.refusedRequests).toHaveLength(6);
    expect(handsfreeRefusal(CHANNELS.draftStash, fixture.requests[1])).toBeUndefined();
  });

  it("round-trips the list, the popped draft and a validated thread reference", () => {
    for (const shape of [fixture.view, fixture.poppedView, fixture.threadReference]) {
      expect(roundTrip(shape)).toEqual(shape);
    }
    expect(fixture.view.stashes.map(stash => stash.id)).toEqual(["stash-2", "stash-1"]);
    expect(fixture.poppedView.popped?.draft.items.map(item => item.kind)).toEqual(["file", "chat"]);
    const reference = fixture.threadReference;
    expect(reference.ok).toBe(true);
    expect(reference.items[0]).toMatchObject({ kind: "chat", value: "codex#thread-b" });
    expect(reference.items[0]?.safeContent).toContain("Use read_thread with this key for more.");
  });
});
