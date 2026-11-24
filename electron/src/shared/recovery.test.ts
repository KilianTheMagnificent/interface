import { describe, expect, it } from "vitest";
import outputFixture from "../fixtures/output-reviews-v49.json";
import recoveryFixture from "../fixtures/recovery-v49.json";
import type { OutputPreviewRequest, OutputReviewResponse } from "./output-preview";
import type { RecoveryRequest, RecoveryResponse } from "./recovery";
import { encodeRendererPayload } from "../main/ipc-payload";
import { CHANNELS } from "./ipc";
import type { AgentRequest } from "./agents";

describe("protocol49 durable review and recovery wire", () => {
  it("shares review requests and immutable snapshots with Rust", () => {
    const requests = outputFixture.requests as OutputPreviewRequest[];
    const responses = outputFixture.responses as OutputReviewResponse[];
    expect(requests.map(item => item.action)).toEqual(["listReviews", "captureReview", "saveReview", "reserveFeedback", "settleFeedback", "deleteReview"]);
    expect(JSON.parse(JSON.stringify(responses))).toEqual(outputFixture.responses);
  });
  it("keeps uncertain delivery distinct from safe retry", () => {
    const requests = recoveryFixture.requests as RecoveryRequest[];
    const responses = recoveryFixture.responses as RecoveryResponse[];
    expect(requests.map(item => item.kind)).toEqual(["read", "prepare", "forget", "listPending", "retainDrafts", "acknowledgeLaunch"]);
    expect(responses[0].receipt?.delivery).toBe("unknown");
    expect(responses[0].receipt?.input.context[0].value).toBe("/project/sketch.png");
  });
  it("keeps launch recovery outside provider options and passes agent context unchanged", () => {
    expect(encodeRendererPayload(CHANNELS.createSession, [{ ...recoveryFixture.launch.options,
      engine: recoveryFixture.launch.engine, recovery: recoveryFixture.launch.recovery }]))
      .toEqual(recoveryFixture.launch);
    const command = recoveryFixture.agentTask as AgentRequest;
    expect(encodeRendererPayload(CHANNELS.agentControl, [command])).toEqual(recoveryFixture.agentTask);
    expect(encodeRendererPayload(CHANNELS.send, ["claude#one", "serialized", recoveryFixture.input]))
      .toEqual({ sessionKey: "claude#one", text: "serialized", recovery: recoveryFixture.input });
  });
});
