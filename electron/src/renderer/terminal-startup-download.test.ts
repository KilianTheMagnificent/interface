import { describe, expect, it } from "vitest";
import fixture from "../fixtures/terminal-startup-download-v31.json";
import type { TerminalStartupDownloadV1 } from "../shared/ipc";
import { RUST_HOST_PROTOCOL_VERSION } from "../main/rust-host-client";
import { STARTUP_DOWNLOAD_HELP, startupDownloadNotice } from "./terminal-startup-download";

describe("terminal startup download v31", () => {
  it("matches the Rust projection and its notice", () => {
    const download: TerminalStartupDownloadV1 = {
      version: 1,
      files: [".git/HEAD", "CLAUDE.md", "~/Desktop/Projects/AGENTS.md"],
      remaining: 5,
    };
    expect(RUST_HOST_PROTOCOL_VERSION).toBeGreaterThanOrEqual(fixture.protocolVersion);
    expect(download).toEqual(fixture.download);
    expect(startupDownloadNotice(download)).toBe(fixture.notice);
  });

  it("names every file when none are left over", () => {
    expect(startupDownloadNotice({ version: 1, files: ["CLAUDE.md"], remaining: 1 }))
      .toBe("Waiting for cloud download: CLAUDE.md");
    expect(startupDownloadNotice({ version: 1, files: [], remaining: 2 }))
      .toBe("Waiting for cloud download: 2 files");
  });

  it("says where the wait comes from and how to avoid it", () => {
    expect(STARTUP_DOWNLOAD_HELP).toMatch(/iCloud Drive/u);
    expect(STARTUP_DOWNLOAD_HELP).toMatch(/Keep Downloaded/u);
  });
});
