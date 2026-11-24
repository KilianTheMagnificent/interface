import { describe, expect, it } from "vitest";
import fixture from "../fixtures/voice-control-v21.json";
import { CHANNELS } from "../shared/ipc";
import { encodeRendererPayload } from "./ipc-payload";
import type { ControlRequest, ControlSnapshot } from "../shared/voice-control";

describe("control desk v1 across the desktop bridge", () => {
  it("encodes every owner command as the exact Rust and iPhone fixture", () => {
    for (const request of fixture.requests as ControlRequest[]) {
      expect(encodeRendererPayload(CHANNELS.voiceControl, [request])).toEqual(request);
    }
    const snapshot = fixture.snapshot as ControlSnapshot;
    expect(snapshot.audioConnected).toBe(false);
    expect(snapshot.runs[0]?.receipts[0]?.state).toBe("accepted");
    expect(snapshot.runs[0]?.stops).toEqual([]);
  });
  it("rejects malformed bridge invocation before transport", () => {
    expect(() => encodeRendererPayload(CHANNELS.voiceControl, [])).toThrow();
    expect(() => encodeRendererPayload(CHANNELS.voiceControl, ["allow everything"])).toThrow();
  });
});
