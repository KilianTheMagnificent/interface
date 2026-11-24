import { describe, expect, it } from "vitest";
import fixture from "../fixtures/terminal-kimi-v54.json";
import type { CreateTerminalRequest, TerminalLaunchProfile } from "./ipc";
import { RUST_HOST_PROTOCOL_VERSION } from "../main/rust-host-client";

const profile: TerminalLaunchProfile = { id: "kimi", engine: "acp:kimi", label: "Kimi Code", available: true };
const request: CreateTerminalRequest = {
  id: "3f2b8c1d-7e4a-4b9f-8d26-5a1c0e9b4f73", cwd: "/tmp/project",
  size: { columns: 100, rows: 30 }, title: "Kimi Code", launch: { version: 1, profileId: "kimi" },
};
describe("Kimi Code terminal contract", () => {
  it("matches the Rust fixture and negotiated host protocol", () => {
    expect(RUST_HOST_PROTOCOL_VERSION).toBeGreaterThanOrEqual(fixture.protocolVersion);
    expect(JSON.parse(JSON.stringify(profile))).toEqual(fixture.profile);
    expect(JSON.parse(JSON.stringify(request))).toEqual(fixture.request);
  });
});
