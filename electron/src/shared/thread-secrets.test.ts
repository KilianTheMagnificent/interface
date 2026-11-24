import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { encodeRendererPayload } from "../main/ipc-payload";
import { handsfreeRefusal } from "../main/handsfree";
import { CHANNELS } from "./ipc";
import type { ThreadSecretCommand, ThreadSecretsView } from "./thread-secrets";

const fixture = JSON.parse(readFileSync(
  new URL("../fixtures/quick-wins-v67.json", import.meta.url), "utf8",
)) as {
  protocolVersion: number;
  secretChannel: string;
  secretRequests: ThreadSecretCommand[];
  secretsView: ThreadSecretsView;
};

describe("the private secret card's channel (quick wins v67)", () => {
  it("sends each secret command to Rust unchanged on its own channel", () => {
    expect(fixture.protocolVersion).toBe(67);
    expect(fixture.secretChannel).toBe(CHANNELS.threadSecret);
    expect(fixture.secretRequests.map(request => request.kind)).toEqual(["list", "give", "decline", "forget"]);
    for (const request of fixture.secretRequests) {
      expect(encodeRendererPayload(CHANNELS.threadSecret, [request])).toEqual(request);
    }
    expect(handsfreeRefusal(CHANNELS.threadSecret, fixture.secretRequests[1])).toBeUndefined();
  });

  it("answers names only, never a value", () => {
    expect(JSON.parse(JSON.stringify(fixture.secretsView))).toEqual(fixture.secretsView);
    expect(Object.keys(fixture.secretsView.secrets[0] ?? {}).sort()).toEqual(["expiresAt", "key", "name"]);
    expect(JSON.stringify(fixture.secretsView)).not.toContain("sk_test");
  });
});
