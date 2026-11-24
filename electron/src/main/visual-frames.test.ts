import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { VISUAL_FRAME_RESPONSE_CSP } from "../shared/visual-frame.js";
import {
  installVisualFrameProtocol, isVisualFrameNavigation, rendererNavigationGuard, VisualFrames,
} from "./visual-frames.js";

const tokens = () => {
  let next = 0;
  return () => (next++).toString(16).padStart(32, "0");
};

describe("visual frames (T3 section 8)", () => {
  it("serves a registered document under the visual CSP and sandbox, and nothing else", async () => {
    const frames = new VisualFrames(8, 1024 * 1024, tokens());
    const url = frames.register("<p>chart</p>");
    expect(url).toBe(`iface-visual://frame/${"0".repeat(32)}`);
    const served = frames.respond(url);
    expect(served.status).toBe(200);
    expect(await served.text()).toBe("<p>chart</p>");
    expect(served.headers.get("content-security-policy")).toBe(VISUAL_FRAME_RESPONSE_CSP);
    expect(served.headers.get("content-type")).toBe("text/html; charset=utf-8");
    expect(served.headers.get("x-content-type-options")).toBe("nosniff");
    expect(served.headers.get("cache-control")).toBe("no-store");
    expect(frames.respond(url, "POST").status).toBe(404);
    expect(frames.respond(`iface-visual://frame/${"f".repeat(32)}`).status).toBe(404);
    expect(frames.respond(`${url}/../x`).status).toBe(404);
    frames.release(url);
    expect(frames.respond(url).status).toBe(404);
  });

  it("keeps a bounded number of documents and bytes, the oldest going first", () => {
    const frames = new VisualFrames(2, 1024 * 1024, tokens());
    const first = frames.register("a");
    const second = frames.register("b");
    const third = frames.register("c");
    expect(frames.size).toBe(2);
    expect(frames.respond(first).status).toBe(404);
    expect(frames.respond(second).status).toBe(200);
    expect(frames.respond(third).status).toBe(200);
    const small = new VisualFrames(10, 9, tokens());
    const old = small.register("abc");
    const kept = small.register("de");
    expect(small.respond(old).status).toBe(404);
    expect(small.respond(kept).status).toBe(200);
    // One document larger than the bound is still served: it is the newest.
    expect(small.respond(small.register("x".repeat(20))).status).toBe(200);
  });

  it("installs on the app session's protocol under its own scheme", async () => {
    const frames = new VisualFrames(8, 1024, tokens());
    const handlers = new Map<string, (request: Request) => Response | Promise<Response>>();
    installVisualFrameProtocol({ handle: (scheme, handler) => { handlers.set(scheme, handler); } }, frames);
    const url = frames.register("<p>x</p>");
    const handler = handlers.get("iface-visual");
    expect(handler).toBeDefined();
    expect((await handler!(new Request(url))).status).toBe(200);
  });

  it("lets only an empty subframe load a visual, and nothing move one afterwards", () => {
    const url = `iface-visual://frame/${"a".repeat(32)}`;
    expect(isVisualFrameNavigation({ url, isMainFrame: false, frame: { url: "" } })).toBe(true);
    expect(isVisualFrameNavigation({ url, isMainFrame: false, frame: { url: "about:blank" } })).toBe(true);
    expect(isVisualFrameNavigation({ url, isMainFrame: false, frame: null })).toBe(true);
    // The window itself never shows a visual.
    expect(isVisualFrameNavigation({ url, isMainFrame: true, frame: { url: "" } })).toBe(false);
    // A visual moving its own frame: a link, a script, a refresh, even to another visual.
    expect(isVisualFrameNavigation({ url, isMainFrame: false, frame: { url } })).toBe(false);
    expect(isVisualFrameNavigation({ url: "https://example.com/", isMainFrame: false,
      frame: { url } })).toBe(false);
    expect(isVisualFrameNavigation({ url: "https://example.com/", isMainFrame: false,
      frame: { url: "" } })).toBe(false);
    expect(isVisualFrameNavigation({ url: "iface-visual://frame/x", isMainFrame: false,
      frame: { url: "" } })).toBe(false);
  });

  it("guards the window: visuals load into new frames, the app stays home, the rest is refused", () => {
    const guard = rendererNavigationGuard(url => url === "app://index.html");
    const decide = (event: { url: string; isMainFrame?: boolean; frame?: { url: string } | null }) => {
      let prevented = false;
      guard({ ...event, preventDefault: () => { prevented = true; } });
      return prevented ? "refused" : "allowed";
    };
    const visual = `iface-visual://frame/${"a".repeat(32)}`;
    expect(decide({ url: visual, isMainFrame: false, frame: { url: "" } })).toBe("allowed");
    expect(decide({ url: "app://index.html", isMainFrame: true })).toBe("allowed");
    expect(decide({ url: visual })).toBe("refused");
    expect(decide({ url: visual, isMainFrame: false, frame: { url: visual } })).toBe("refused");
    expect(decide({ url: "https://example.com/", isMainFrame: false, frame: { url: visual } })).toBe("refused");
    const gone = { get url(): string { throw new Error("Render frame was disposed"); } };
    expect(decide({ url: visual, isMainFrame: false, frame: gone })).toBe("refused");
  });

  it("is wired into main: the protocol, the navigation guard and the authorized channel", () => {
    const source = readFileSync(new URL("./main.ts", import.meta.url), "utf8");
    expect(source).toContain("installVisualFrameProtocol(session.defaultSession.protocol, visualFrames);");
    expect(source).toContain("const guardNavigation = rendererNavigationGuard(url => isAllowedRendererNavigation(url, location));");
    for (const event of ["will-navigate", "will-frame-navigate", "will-redirect"]) {
      expect(source).toContain(`win.webContents.on("${event}", guardNavigation);`);
    }
    const handler = source.slice(source.indexOf("ipcMain.handle(VISUAL_FRAME_CHANNEL"),
      source.indexOf("ipcMain.on(WORK_IN_FLIGHT_CHANNEL"));
    expect(handler).toContain("isAuthorizedIpcSender(event, contents)");
    expect(handler).toContain("isAllowedRendererNavigation(event.senderFrame?.url ?? \"\", location)");
    expect(handler).toContain("parseVisualFrameRequest(payload)");
    // No postMessage channel exists for a visual anywhere in main.
    expect(source).not.toContain("postMessage");
  });
});
