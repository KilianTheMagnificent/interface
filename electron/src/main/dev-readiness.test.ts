import { describe, expect, it } from "vitest";
import { devLaunchStaysHidden, reportDevReadiness } from "./dev-readiness.js";

describe("Dev readiness", () => {
  const revision = "a".repeat(40);
  it("waits for the rendered app and completed Rust read", async () => {
    const lines: string[] = [];
    let probes = 0;
    await reportDevReadiness({ launch: "launch", revision, active: () => true,
      probe: async () => ++probes > 1, wait: async () => {}, emit: line => lines.push(line) });
    expect(probes).toBe(2);
    expect(lines).toHaveLength(1);
    expect(lines[0]).toContain("INTERFACE_DEV_READY");
    expect(lines[0]).toContain(revision);
  });
  it("does not report readiness after shutdown or a failed bridge call", async () => {
    for (const fails of [true, false]) {
      const lines: string[] = [];
      let active = true;
      await reportDevReadiness({ launch: "launch", revision, active: () => active,
        probe: async () => { if (fails) throw new Error("host stopped"); active = false; return true; },
        wait: async () => {}, emit: line => lines.push(line) });
      expect(lines).toHaveLength(1);
      expect(lines[0]).toContain("INTERFACE_DEV_FAILED");
    }
  });
  it("has no production behavior without a launch identity", async () => {
    await reportDevReadiness({ launch: undefined, revision, active: () => true,
      probe: async () => { throw new Error("must not probe"); }, emit: () => { throw new Error("must not emit"); } });
  });
  it("keeps an agent's Dev refresh hidden unless Dev was in front", () => {
    const refresh = { INTERFACE_DEV_SUPERVISED: "1", INTERFACE_DEV_LAUNCH: "launch" };
    expect(devLaunchStaysHidden("Interface Dev", { ...refresh, INTERFACE_DEV_QUIET: "1" })).toBe(true);
    // An older branch's dev-restart sends no flag at all.
    expect(devLaunchStaysHidden("Interface Dev", refresh)).toBe(true);
    expect(devLaunchStaysHidden("Interface Dev", { ...refresh, INTERFACE_DEV_QUIET: "0" })).toBe(false);
    // `npm run dev` by hand, a Dock launch, and production.
    expect(devLaunchStaysHidden("Interface Dev", { INTERFACE_DEV_SUPERVISED: "1" })).toBe(false);
    expect(devLaunchStaysHidden("Interface Dev", { INTERFACE_DEV_LAUNCH: "launch" })).toBe(false);
    expect(devLaunchStaysHidden("Interface", refresh)).toBe(false);
  });
});
