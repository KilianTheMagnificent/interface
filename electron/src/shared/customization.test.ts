import { describe, expect, it } from "vitest";
import fixture from "../fixtures/customization-background-v49.json";
import type { CustomizationCommand, ProviderCustomization, IfaceApi } from "./ipc";
import { CHANNELS } from "./ipc";
import { encodeRendererPayload } from "../main/ipc-payload";

describe("provider inventory and local runner wire", () => {
  it("encodes the same v49 commands consumed by Rust", () => {
    for (const request of fixture.customizationRequests) {
      const typed = request as CustomizationCommand;
      expect(encodeRendererPayload(CHANNELS.customizationControl, [typed])).toEqual(request);
    }
    for (const request of fixture.backgroundRequests) {
      const typed = request as Parameters<IfaceApi["backgroundControl"]>[0];
      expect(encodeRendererPayload(CHANNELS.backgroundControl, [typed])).toEqual(request);
    }
    const response = fixture.customizationResponse as ProviderCustomization;
    expect(response.loaded).toBe("unknown");
    expect(response.health?.state).toBe("ready");
    expect(Object.keys(response)).toEqual(["provider", "userRoot", "loaded", "inventory", "health"]);
  });
});
