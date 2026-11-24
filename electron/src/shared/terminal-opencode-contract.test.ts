import { describe, expect, it } from "vitest";
import fixture from "../fixtures/terminal-opencode-v9.json";
import type { CreateTerminalRequest, TerminalLaunchProfile } from "./ipc";
import { RUST_HOST_PROTOCOL_VERSION } from "../main/rust-host-client";

const profile: TerminalLaunchProfile = { id: "opencode", engine: "acp:opencode", label: "OpenCode", available: true };
const request: CreateTerminalRequest = {
  id: "2f56fa6c-e114-4e88-bb31-6878c8cc3702", cwd: "/tmp/project",
  size: { columns: 100, rows: 30 }, title: "OpenCode", launch: { version: 1, profileId: "opencode" },
};
describe("OpenCode terminal contract", () => {
  it("matches the Rust fixture and negotiated host protocol", () => {
    expect(RUST_HOST_PROTOCOL_VERSION).toBeGreaterThanOrEqual(fixture.protocolVersion);
    expect(JSON.parse(JSON.stringify(profile))).toEqual(fixture.profile);
    expect(JSON.parse(JSON.stringify(request))).toEqual(fixture.request);
  });
});
