import { describe, expect, it } from "vitest";
import fixture from "../fixtures/terminal-fast-mode-v8.json";
import { TERMINAL_FAST_MODE_KEY, type TerminalFastModeV1 } from "./ipc";
import { RUST_HOST_PROTOCOL_VERSION } from "../main/rust-host-client";

describe("terminal Fast mode v8 contract", () => {
  it("matches the Rust projection and native key without submitting text", () => {
    const mode: TerminalFastModeV1 = { version: 1, model: "gpt-6-astra", enabled: true, ready: true };
    expect(RUST_HOST_PROTOCOL_VERSION).toBeGreaterThanOrEqual(fixture.protocolVersion);
    expect(mode).toEqual(fixture.mode);
    expect(TERMINAL_FAST_MODE_KEY).toBe(fixture.toggleKey);
    expect(TERMINAL_FAST_MODE_KEY).not.toMatch(/[\r\n]/u);
  });
});
