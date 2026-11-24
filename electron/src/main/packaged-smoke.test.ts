import { afterEach, describe, expect, it, vi } from "vitest";
import { chmodSync, mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
vi.mock("electron", () => ({ app: { getPath: () => "/private/tmp/profile" }, BrowserWindow: {}, dialog: {}, Menu: {}, shell: {} }));
import { guardPackagedSmoke, packagedSmokeHostCommand, smokeProfile } from "./packaged-smoke.js";
const roots: string[] = [];
afterEach(() => { for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true }); });
function fixture() {
  const profile = realpathSync(mkdtempSync(join(tmpdir(), "packaged-smoke-test-")));
  roots.push(profile);
  writeFileSync(join(profile, "host.sb"), "(version 1)(allow default)", { mode: 0o600 });
  const data = join(profile, "data"), home = join(profile, "home");
  mkdirSync(data, { mode: 0o700 }); mkdirSync(home, { mode: 0o700 });
  return { profile, data, home, argv: [`--user-data-dir=${profile}`], env: {
    INTERFACE_PACKAGED_SMOKE_PROFILE: profile, INTERFACE_HOST_SMOKE_DATA_ROOT: data,
    INTERFACE_HOST_SMOKE_HOME: home,
  } };
}
describe("packaged smoke isolation", () => {
  it("leaves ordinary launches unchanged", () => { expect(smokeProfile({}, [])).toBeUndefined(); });
  it("accepts only a complete matching private profile", () => {
    const f = fixture(); expect(smokeProfile(f.env, f.argv)).toBe(f.profile);
    expect(() => smokeProfile(f.env, [])).toThrow(/match/u);
    expect(() => smokeProfile({ INTERFACE_HOST_SMOKE_HOME: f.home }, [])).toThrow(/together/u);
  });
  it("refuses shared directories and symlink roots before launch", () => {
    const f = fixture(); chmodSync(f.home, 0o755);
    expect(() => smokeProfile(f.env, f.argv)).toThrow(/private/u);
    rmSync(f.home, { recursive: true }); symlinkSync(f.data, f.home);
    expect(() => smokeProfile(f.env, f.argv)).toThrow(/private/u);
  });
  it("wraps only the isolated host and refuses a shared policy file", () => {
    expect(packagedSmokeHostCommand(false, "/app/host")).toEqual({ command: "/app/host" });
    expect(packagedSmokeHostCommand(true, "/app/host")).toEqual({
      command: "/usr/bin/sandbox-exec", args: ["-f", "/private/tmp/profile/host.sb", "/app/host"],
    });
    const f = fixture(); chmodSync(join(f.profile, "host.sb"), 0o644);
    expect(() => smokeProfile(f.env, f.argv)).toThrow(/private host sandbox/u);
  });
  it("refuses effects which leave the session while allowing ordinary Agent reads", () => {
    expect(() => guardPackagedSmoke(true, "iface:agentControl", { kind: "read" })).not.toThrow();
    expect(() => guardPackagedSmoke(true, "iface:openTerminal", {})).toThrow(/cannot/u);
    expect(() => guardPackagedSmoke(false, "iface:openTerminal", {})).not.toThrow();
  });
});
