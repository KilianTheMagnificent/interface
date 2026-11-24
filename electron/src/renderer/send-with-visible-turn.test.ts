import { describe, expect, it } from "vitest";
import { launchPendingEcho } from "./transcript-merge";
import { sendWithVisibleTurn } from "./send-with-visible-turn";

describe("non-composer prompt turn boundaries", () => {
  it("shows the prompt and checkpoints before sending, then marks acceptance", async () => {
    const events: string[] = [];
    const echo = launchPendingEcho("Revise the output", []);
    await sendWithVisibleTurn(() => { events.push("capture turn"); return echo; }, {
      send: async (before, accepted) => { before(); events.push("send"); accepted(); },
      append: row => { events.push(row.text); }, checkpoint: key => { events.push(key); },
      delivered: () => { events.push("accepted"); }, fail: () => { events.push("failed"); },
    });
    expect(events).toEqual(["capture turn", "Revise the output", `e:${echo.id}`, "send", "accepted"]);
  });

  it.each([
    [false, "Attachment validation refused", false],
    [true, "Definitely unsent", true],
    [true, "Delivery outcome unknown", false],
  ])("keeps certainty when shown=%s and the result is %s", async (shown, message, failed) => {
    const echo = launchPendingEcho("Continue the task", []);
    const appended: string[] = [], failures: string[] = [];
    await expect(sendWithVisibleTurn(() => echo, {
      send: async before => { if (shown) before(); throw new Error(message); },
      append: row => { appended.push(row.id); }, checkpoint: () => {}, delivered: () => {},
      fail: (_id, error) => { failures.push(error); },
    })).rejects.toThrow(message);
    expect(appended).toEqual(shown ? [echo.id] : []);
    expect(failures).toEqual(failed ? [message] : []);
  });
});
