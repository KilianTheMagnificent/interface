import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  BUILD_CHANNEL_ARG_PREFIX, buildChannelArgument, productTitle, resolveBuildChannel,
} from "./build-channel";

describe("build channel", () => {
  it("treats the Vite supervisor and the Interface Dev name as dev", () => {
    expect(resolveBuildChannel({
      appName: "Interface",
      viteDevServerUrl: "http://127.0.0.1:5199",
    })).toBe("dev");
    expect(resolveBuildChannel({ appName: "Interface Dev" })).toBe("dev");
  });

  it("keeps the packaged product on the release channel", () => {
    expect(resolveBuildChannel({ appName: "Interface" })).toBe("release");
  });

  it("names the window and the argv flag from the channel", () => {
    expect(productTitle("dev")).toBe("Interface Dev");
    expect(productTitle("release")).toBe("Interface");
    expect(buildChannelArgument("dev")).toBe(`${BUILD_CHANNEL_ARG_PREFIX}dev`);
  });

  it("stamps the same argv flag in main and preload", () => {
    const main = readFileSync(fileURLToPath(new URL("./main.ts", import.meta.url)), "utf8");
    const preload = readFileSync(fileURLToPath(new URL("./preload.cts", import.meta.url)), "utf8");
    expect(main).toContain("buildChannelArgument(buildChannel)");
    expect(main).toMatch(
      /allowPackagedDevServer: appName === "Interface Dev"\s*&& process\.env\.INTERFACE_DEV_SUPERVISED === "1"/u,
    );
    expect(main).toContain("viteDevServerUrl: location.devServerUrl");
    const locationIndex = main.indexOf("const location = resolveRendererLocation");
    const channelIndex = main.indexOf("const buildChannel = resolveBuildChannel");
    const loadIndex = main.indexOf(
      "win.loadURL(location.devServerUrl ?? location.packagedEntryUrl)",
    );
    expect(locationIndex).toBeGreaterThanOrEqual(0);
    expect(locationIndex).toBeLessThan(channelIndex);
    expect(channelIndex).toBeLessThan(loadIndex);
    expect(loadIndex).toBeGreaterThanOrEqual(0);
    expect(preload).toContain(BUILD_CHANNEL_ARG_PREFIX);
    expect(preload).toContain("exposeInMainWorld(\"ifaceBuild\"");
  });
});
