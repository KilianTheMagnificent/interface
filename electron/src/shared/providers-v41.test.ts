import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { encodeRendererPayload } from "../main/ipc-payload";
import { CHANNELS, type CreateTerminalRequest } from "./ipc";
import type { EngineEvent, EngineInfo } from "./engine";
import { conversationItems } from "../renderer/conversation-reducer";

const fixture = JSON.parse(readFileSync(
  new URL("../fixtures/providers-v41.json", import.meta.url), "utf8",
)) as {
  signedOutInfo: EngineInfo;
  signedOutError: EngineEvent;
  plainError: EngineEvent;
  signInTerminal: CreateTerminalRequest;
  refresh: { refresh: boolean };
};

describe("providers v41", () => {
  it("asks Rust to find the provider CLIs again only on an explicit refresh", () => {
    for (const channel of [CHANNELS.probeEngines, CHANNELS.listTerminalLaunchProfiles]) {
      expect(encodeRendererPayload(channel, [])).toEqual({});
      expect(encodeRendererPayload(channel, [undefined])).toEqual({});
      expect(encodeRendererPayload(channel, [{ refresh: false }])).toEqual({});
      expect(encodeRendererPayload(channel, [fixture.refresh])).toEqual({ refresh: true });
      expect(() => encodeRendererPayload(channel, [{ reload: true }])).toThrow();
      expect(() => encodeRendererPayload(channel, [{ refresh: "yes" }])).toThrow();
    }
  });

  it("sends a sign-in terminal as Rust reads it", () => {
    expect(encodeRendererPayload(CHANNELS.createTerminal, [fixture.signInTerminal]))
      .toEqual(fixture.signInTerminal);
    expect(fixture.signedOutInfo.auth).toBe("signedOut");
  });

  it("offers Sign in on a turn that failed because the CLI is signed out, and only there", () => {
    expect(fixture.signedOutError).toMatchObject({ kind: "error", cause: "signedOut" });
    const items = conversationItems([fixture.signedOutError, fixture.plainError]);
    const notices = items.filter(item => item.kind === "notice");
    expect(notices).toHaveLength(2);
    expect(notices[0]).toMatchObject({ tone: "error", signIn: true,
      text: "Not logged in · Please run /login" });
    expect(notices[1]).not.toHaveProperty("signIn");
  });
});
