import { describe, expect, it } from "vitest";
import type { EngineEvent, SessionKey } from "../shared/engine";
import { activeApprovalsFor } from "./app-session-projection";
import { steeringQueueEventEndsTurn, steeringQueueTerminalFailure } from "./steering-queue";
import { eventEndsTurn, eventFailsTurn } from "./turn-events";

const EDGES: [EngineEvent, { ends: boolean; fails: boolean }][] = [
  [{ kind: "session.closed" }, { ends: true, fails: true }],
  [{ kind: "status.changed", status: "idle" }, { ends: true, fails: false }],
  [{ kind: "status.changed", status: "failed" }, { ends: true, fails: true }],
  [{ kind: "error", message: "gone", retryable: false }, { ends: true, fails: true }],
  [{ kind: "error", message: "again", retryable: true }, { ends: false, fails: false }],
  [{ kind: "status.changed", status: "working" }, { ends: false, fails: false }],
  [{ kind: "status.changed", status: "waiting" }, { ends: false, fails: false }],
  [{ kind: "message.delta", text: "hi" }, { ends: false, fails: false }],
];

describe("turn-ending events", () => {
  it("classify a turn's end, and a bad end, one way", () => {
    for (const [event, expected] of EDGES) {
      expect({ ends: eventEndsTurn(event), fails: eventFailsTurn(event) }, JSON.stringify(event))
        .toEqual(expected);
    }
    // The steering queue's names are the same predicates, not copies.
    expect(steeringQueueEventEndsTurn).toBe(eventEndsTurn);
    expect(steeringQueueTerminalFailure).toBe(eventFailsTurn);
  });

  it("close an unanswered approval exactly when the turn ends", () => {
    const key = "codex#s" as SessionKey;
    const approval: EngineEvent = { kind: "approval.requested", approvalId: "a1", tool: "shell",
      summary: "Run it", input: {} };
    for (const [event, { ends }] of EDGES) {
      const active = activeApprovalsFor(key, [approval, event], true, {});
      expect(active.has("a1"), JSON.stringify(event)).toBe(!ends);
    }
  });
});
