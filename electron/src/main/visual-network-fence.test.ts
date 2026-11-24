import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { DEAD_PROXY, fenceBypassRules, fenceWindowNetwork } from "./visual-network-fence";

const main = readFileSync(new URL("./main.ts", import.meta.url), "utf8");

describe("the app window's network fence (WebRTC out of a visual reply)", () => {
  it("turns off WebRTC's own UDP and sends every request, loopback too, to a proxy that leads nowhere", async () => {
    const calls: unknown[] = [];
    const contents = { setWebRTCIPHandlingPolicy: (policy: string) => { calls.push(["policy", policy]); } };
    const target = { setProxy: async (config: unknown) => { calls.push(["proxy", config]); } };
    await fenceWindowNetwork(contents as never, target as never);
    expect(calls).toEqual([
      ["policy", "disable_non_proxied_udp"],
      ["proxy", { mode: "fixed_servers", proxyRules: DEAD_PROXY, proxyBypassRules: "<-loopback>" }],
    ]);
    expect(DEAD_PROXY).toBe("http://127.0.0.1:1");
  });

  it("lets only the dev server through, by its own host and port", () => {
    expect(fenceBypassRules()).toBe("<-loopback>");
    expect(fenceBypassRules("http://localhost:5173/")).toBe("<-loopback>,localhost:5173");
  });

  it("fences the window's own session before its first document loads", () => {
    const fence = main.indexOf("fenceWindowNetwork(win.webContents, session.defaultSession, location.devServerUrl)");
    const awaited = main.indexOf("await fenceWindowNetwork(");
    const load = main.indexOf("win.loadURL(location.devServerUrl ?? location.packagedEntryUrl)");
    expect(fence).toBeGreaterThan(0);
    expect(awaited).toBeGreaterThan(0);
    expect(awaited).toBeLessThan(fence);
    expect(load).toBeGreaterThan(awaited);
  });

  it("refuses to register a visual frame unless the window's network fence applied", () => {
    const declared = main.indexOf("let networkFenceApplied = false;");
    const awaited = main.indexOf("await fenceWindowNetwork(");
    const marked = main.indexOf(".then(() => { networkFenceApplied = true; })");
    const load = main.indexOf("win.loadURL(location.devServerUrl ?? location.packagedEntryUrl)");
    const handler = main.slice(
      main.indexOf("ipcMain.handle(VISUAL_FRAME_CHANNEL"),
      main.indexOf("ipcMain.on(WORK_IN_FLIGHT_CHANNEL"),
    );
    expect(declared).toBeGreaterThan(0);
    expect(declared).toBeLessThan(awaited);
    expect(marked).toBeGreaterThan(awaited);
    expect(load).toBeGreaterThan(marked);
    expect(handler).toContain(
      'if (request.kind === "register") return networkFenceApplied ? visualFrames.register(request.html) : null;',
    );
  });
});
