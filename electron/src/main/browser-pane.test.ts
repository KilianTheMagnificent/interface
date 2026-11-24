import { execFileSync } from "node:child_process";
import { EventEmitter } from "node:events";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { BrowserWindow, BrowserWindowConstructorOptions, Rectangle, Session, WebContents, WebPreferences } from "electron";
import { afterEach, describe, expect, it, vi } from "vitest";
import * as renderer from "../renderer/browser-pane";
import type { BrowserEventV1 } from "../shared/browser-events";
import {
  BROWSER_ACTIVATION_MS, BROWSER_PANE_PARTITION, BROWSER_QUARANTINE_ATTRIBUTE, BROWSER_WHERE_FROMS_ATTRIBUTE, BrowserGrants,
  MAX_BROWSER_POPUPS, MIN_BROWSER_POPUP, browserContextMenu, browserDownloadName, browserDownloadOrigin, browserDownloadPath,
  browserDownloadRequiresSaveDialog, browserExternalPreview, browserExternalScheme, browserOpenAction, browserPaneUrl, browserPaneUserAgent, browserPathExists, browserPermissionCheck,
  browserPermissionRequest, browserPopupBounds, browserPopupOptions, browserPopupTitle, browserPopupWindowOptions, browserQuarantineValue,
  browserQuestion, browserSearchLabel, browserWhereFroms, hardenBrowserPaneAttach,
  installBrowserPaneGuards, quarantineBrowserDownload, type BrowserMenuItem, type BrowserMenuParams, type BrowserPaneHooks, type BrowserQuestion,
} from "./browser-pane";

describe("Browser pane guards", () => {
  it("shares the one session name with the renderer", () => {
    expect(renderer.BROWSER_PANE_PARTITION).toBe(BROWSER_PANE_PARTITION);
    expect(BROWSER_PANE_PARTITION).not.toBe("");
  });

  it("loads only web pages with a host and no credentials", () => {
    expect(browserPaneUrl("http://localhost:5173")).toBe("http://localhost:5173/");
    expect(browserPaneUrl("https://example.com/a?b#c")).toBe("https://example.com/a?b#c");
    for (const refused of ["", "file:///etc/hosts", "javascript:alert(1)", "about:blank",
      "http://user:pw@example.com", "chrome://gpu", `https://example.com/${"a".repeat(9000)}`]) {
      expect(browserPaneUrl(refused), refused.slice(0, 40)).toBeNull();
    }
  });

  it("pins a webview's preferences whatever its markup asked for", () => {
    const preferences = {
      preload: "/tmp/evil.js", nodeIntegration: true, nodeIntegrationInSubFrames: true,
      contextIsolation: false, sandbox: false, webSecurity: false, webviewTag: true,
      allowRunningInsecureContent: true,
    } as WebPreferences & { preloadURL?: string };
    preferences.preloadURL = "file:///tmp/evil.js";
    expect(hardenBrowserPaneAttach(preferences, {
      partition: BROWSER_PANE_PARTITION, src: "http://localhost:3000",
    })).toBe(true);
    expect(preferences).toMatchObject({
      nodeIntegration: false, nodeIntegrationInSubFrames: false, contextIsolation: true,
      sandbox: true, webSecurity: true, webviewTag: false, allowRunningInsecureContent: false,
    });
    expect(preferences.preload).toBeUndefined();
    expect(preferences.preloadURL).toBeUndefined();
  });

  it("refuses a webview in any other session or for anything but a web page", () => {
    const attach = (params: Record<string, string>): boolean => hardenBrowserPaneAttach({}, params);
    expect(attach({ src: "http://localhost:3000" })).toBe(false);
    expect(attach({ partition: "persist:other", src: "http://localhost:3000" })).toBe(false);
    expect(attach({ partition: BROWSER_PANE_PARTITION, src: "file:///etc/hosts" })).toBe(false);
    expect(attach({ partition: BROWSER_PANE_PARTITION })).toBe(false);
  });

  it("presents pages as Chrome, without Electron's or the app's tokens", () => {
    expect(browserPaneUserAgent(
      "Mozilla/5.0 (Macintosh) AppleWebKit/537.36 (KHTML, like Gecko) interface-electron/0.3.139 Chrome/140.0.0.0 Electron/43.3.0 Safari/537.36",
    )).toBe("Mozilla/5.0 (Macintosh) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36");
  });

  it("installs the guards on the app window before its first document", () => {
    const main = readFileSync(new URL("./main.ts", import.meta.url), "utf8");
    expect(main).toContain("webviewTag: true,");
    const guards = main.indexOf("installBrowserPaneGuards(win.webContents, session.fromPartition(BROWSER_PANE_PARTITION),");
    expect(guards).toBeGreaterThan(main.indexOf("const win = createWindow("));
    expect(guards).toBeLessThan(main.indexOf("const host = new RustHostClient("));
  });
});

const NO_EDIT = {
  canUndo: false, canRedo: false, canCut: false, canCopy: false, canPaste: false, canDelete: false,
  canSelectAll: false, canEditRichly: false,
};
const menuParams = (change: Partial<BrowserMenuParams>): BrowserMenuParams => ({
  x: 10, y: 20, linkURL: "", srcURL: "", mediaType: "none", isEditable: false, selectionText: "",
  misspelledWord: "", dictionarySuggestions: [], editFlags: NO_EDIT, ...change,
});
const labels = (items: readonly BrowserMenuItem[]): string[] =>
  items.map(item => item.kind === "separator" ? "—" : item.enabled ? item.label : `(${item.label})`);

describe("Browser pages behave like a Chrome tab", () => {
  it("shows PDFs in the page and keeps a page's fullscreen inside the window", () => {
    const preferences: WebPreferences = { plugins: false };
    expect(hardenBrowserPaneAttach(preferences, { partition: BROWSER_PANE_PARTITION, src: "https://example.com/" }))
      .toBe(true);
    expect(preferences).toMatchObject({ plugins: true, disableHtmlFullscreenWindowResize: true });
  });

  it("keeps a page's fullscreen in the app window and the guards quiet in hands-free runs", () => {
    const main = readFileSync(new URL("./main.ts", import.meta.url), "utf8");
    const window = main.slice(main.indexOf("new BrowserWindow({"), main.indexOf("installWindowVisibility(win);"));
    expect(window).toContain("disableHtmlFullscreenWindowResize: true,");
    expect(main).toContain("browserPaneNativeHooks(() => appWindow, handsfree.enabled || packagedSmoke));");
    const native = readFileSync(new URL("./browser-pane-native.ts", import.meta.url), "utf8");
    expect(native).toContain("if (!quiet) return app.getPath(\"downloads\");");
    expect(native).toContain("if (quiet || win === null || win.isDestroyed()) return false;");
    expect(native).toContain("openExternal: url => quiet ? Promise.resolve() : shell.openExternal(url),");
    expect(native).toContain("showPopups: !quiet,");
  });

  it("makes Block the sheet's default, so only a deliberate Allow grants", () => {
    const native = readFileSync(new URL("./browser-pane-native.ts", import.meta.url), "utf8");
    const sheet = /buttons: \[([^\]]*)\], defaultId: (\d+), cancelId: (\d+)/u.exec(native);
    expect(sheet).not.toBeNull();
    const buttons = sheet![1]!.split(",").map(label => label.trim().replace(/"/gu, ""));
    expect(buttons).toEqual(["Block", "Allow"]);
    expect(buttons[Number(sheet![2])]).toBe("Block");
    expect(buttons[Number(sheet![3])]).toBe("Block");
    // Only Allow's index grants.
    expect(native).toContain("return response === 1;");
  });

  it("opens new tabs and windows the way Chrome does", () => {
    const open = (url: string, disposition: string, features = "") =>
      browserOpenAction({ url, disposition: disposition as "default", features });
    expect(open("https://example.com/a", "foreground-tab"))
      .toEqual({ kind: "tab", url: "https://example.com/a", background: false });
    expect(open("https://example.com/a", "background-tab"))
      .toEqual({ kind: "tab", url: "https://example.com/a", background: true });
    // Shift-click asks for a window without features: Interface has tabs.
    expect(open("https://example.com/a", "new-window")).toEqual({ kind: "tab", url: "https://example.com/a", background: false });
    expect(open("https://accounts.google.com/o/oauth2", "new-window", "width=500,height=600"))
      .toEqual({ kind: "popup" });
    expect(open("mailto:someone@example.com", "foreground-tab"))
      .toEqual({ kind: "external", url: "mailto:someone@example.com" });
    for (const refused of ["file:///etc/hosts", "javascript:alert(1)", "about:blank", "data:text/html,hi",
      "chrome://settings", "http://user:pw@example.com/"]) {
      expect(open(refused, "new-window", "width=300"), refused).toEqual({ kind: "refuse" });
    }
  });

  it("names another app's links by their scheme and none of the web's own", () => {
    expect(browserExternalScheme("mailto:a@example.com")).toBe("mailto");
    expect(browserExternalScheme("zoommtg://zoom.us/join?confno=1")).toBe("zoommtg");
    expect(browserExternalScheme("SLACK://open")).toBe("slack");
    for (const web of ["https://example.com", "http://x", "file:///etc", "javascript:1", "data:,x", "blob:x",
      "about:blank", "chrome://gpu", "chrome-extension://x", "devtools://x", "view-source:https://x",
      "ws://x", "smb://fileserver/share", "afp://server/vol", "nfs://server/export", "cifs://server/share",
      "webdav://server/dav", "dav://server/dav", "davs://server/dav", "applescript://com.apple.scripteditor?action=new",
      "x-apple.systempreferences:com.apple.preference.security", "prefs:root=General",
      "no scheme", "", `${"a".repeat(40)}:x`, "mailto:a\nb", "zoommtg:\u0000"]) {
      expect(browserExternalScheme(web), web).toBeNull();
    }
  });

  it("pins a popup window exactly as a page, with no preload and no Node", () => {
    const options = browserPopupOptions(null, false);
    expect(options.show).toBe(false);
    expect(options.parent).toBeUndefined();
    expect(options.webPreferences).toEqual({
      nodeIntegration: false, nodeIntegrationInSubFrames: false, nodeIntegrationInWorker: false,
      contextIsolation: true, sandbox: true, webSecurity: true, allowRunningInsecureContent: false,
      experimentalFeatures: false, webviewTag: false, plugins: true, disableHtmlFullscreenWindowResize: true,
    });
    expect(options.webPreferences).not.toHaveProperty("preload");
  });

  it("builds the right-click menu for what is under the pointer, in Chrome's order", () => {
    const history = { back: true, forward: false };
    expect(labels(browserContextMenu(menuParams({}), history)))
      .toEqual(["Back", "(Forward)", "Reload", "—", "Inspect"]);
    expect(labels(browserContextMenu(menuParams({ linkURL: "https://example.com/docs" }), history))).toEqual([
      "Open Link in New Tab", "Copy Link Address", "—", "Back", "(Forward)", "Reload", "—", "Inspect"]);
    expect(labels(browserContextMenu(menuParams({ linkURL: "mailto:a@example.com" }), history))[0])
      .toBe("Copy Link Address");
    expect(labels(browserContextMenu(menuParams({
      linkURL: "https://example.com/", mediaType: "image", srcURL: "https://example.com/cat.png",
    }), history))).toEqual(["Open Link in New Tab", "Copy Link Address", "—",
      "Open Image in New Tab", "Save Image As…", "Copy Image", "—", "Back", "(Forward)", "Reload", "—", "Inspect"]);
    expect(labels(browserContextMenu(menuParams({ selectionText: "  the quick  brown fox " }), history)))
      .toEqual(["Copy", "Search Google for “the quick brown fox”", "—", "Back", "(Forward)", "Reload", "—", "Inspect"]);
    expect(labels(browserContextMenu(menuParams({
      isEditable: true, selectionText: "teh", misspelledWord: "teh", dictionarySuggestions: ["the", "tech"],
      editFlags: { ...NO_EDIT, canCopy: true, canPaste: true, canSelectAll: true },
    }), history))).toEqual(["the", "tech", "—", "(Cut)", "Copy", "Paste", "Select All", "—",
      "Back", "(Forward)", "Reload", "—", "Inspect"]);
    const items = browserContextMenu(menuParams({ selectionText: "café" }), history);
    const search = items.find(item => item.kind === "item" && item.label.startsWith("Search"));
    expect(search).toMatchObject({ action: { kind: "open-tab", url: "https://www.google.com/search?q=caf%C3%A9", background: false } });
    const link = browserContextMenu(menuParams({ linkURL: "https://example.com/x" }), history)[0];
    expect(link).toMatchObject({ action: { kind: "open-tab", url: "https://example.com/x", background: true } });
    expect(browserSearchLabel("a".repeat(40))).toBe(`Search Google for “${"a".repeat(31)}…”`);
  });

  it("names downloads as Chrome does and never takes a name already used", () => {
    expect(browserDownloadName("report.pdf")).toBe("report.pdf");
    expect(browserDownloadName("../../etc/passwd")).toBe("_.._etc_passwd");
    expect(browserDownloadName("a:b*c?.txt")).toBe("a_b_c_.txt");
    expect(browserDownloadName(" .hidden ")).toBe("hidden");
    expect(browserDownloadName("...")).toBe("download");
    expect(browserDownloadName(`${"x".repeat(300)}.pdf`)).toBe(`${"x".repeat(196)}.pdf`);
    const taken = new Set(["/d/report.pdf", "/d/report (1).pdf", "/d/a.tar.gz", "/d/README"]);
    const path = (name: string): string => browserDownloadPath("/d", name, candidate => taken.has(candidate));
    expect(path("report.pdf")).toBe("/d/report (2).pdf");
    expect(path("new.pdf")).toBe("/d/new.pdf");
    expect(path("a.tar.gz")).toBe("/d/a (1).tar.gz");
    expect(path("README")).toBe("/d/README (1)");
  });

  it("asks for the camera, microphone, notifications and the clipboard, allows fullscreen and refuses the rest", () => {
    expect(browserPermissionRequest("media", { mediaTypes: ["video", "audio"] }))
      .toEqual({ kind: "ask", asks: ["camera", "microphone"] });
    expect(browserPermissionRequest("media", { mediaTypes: ["audio"] })).toEqual({ kind: "ask", asks: ["microphone"] });
    expect(browserPermissionRequest("media", { mediaTypes: [] })).toEqual({ kind: "refuse" });
    expect(browserPermissionRequest("notifications", {})).toEqual({ kind: "ask", asks: ["notifications"] });
    expect(browserPermissionRequest("clipboard-read", {})).toEqual({ kind: "ask", asks: ["clipboard-read"] });
    expect(browserPermissionRequest("fullscreen", {})).toEqual({ kind: "allow" });
    expect(browserPermissionRequest("pointerLock", {})).toEqual({ kind: "allow" });
    // Writing to the clipboard (a site's Copy button) is allowed as Chrome allows it; reading asks.
    expect(browserPermissionRequest("clipboard-sanitized-write", {})).toEqual({ kind: "allow" });
    expect(browserPermissionRequest("openExternal", { externalURL: "zoommtg://zoom.us/join" }))
      .toEqual({ kind: "ask", asks: ["external:zoommtg"] });
    expect(browserPermissionRequest("openExternal", { externalURL: "file:///etc" })).toEqual({ kind: "refuse" });
    for (const refused of ["geolocation", "display-capture", "midi", "midiSysex", "hid", "serial", "usb",
      "idle-detection", "mediaKeySystem", "keyboardLock", "window-management",
      "speaker-selection", "storage-access", "fileSystem", "unknown"]) {
      expect(browserPermissionRequest(refused, {}), refused).toEqual({ kind: "refuse" });
    }
    const granted = (ask: string): boolean => ask === "camera" || ask === "notifications";
    expect(browserPermissionCheck("media", "video", granted)).toBe(true);
    expect(browserPermissionCheck("media", "audio", granted)).toBe(false);
    expect(browserPermissionCheck("notifications", undefined, granted)).toBe(true);
    expect(browserPermissionCheck("clipboard-read", undefined, granted)).toBe(false);
    expect(browserPermissionCheck("fullscreen", undefined, granted)).toBe(true);
    expect(browserPermissionCheck("clipboard-sanitized-write", undefined, () => false)).toBe(true);
    expect(browserPermissionCheck("geolocation", undefined, () => true)).toBe(false);
  });

  it("asks a site once per permission for the app run, and once at a time", async () => {
    const grants = new BrowserGrants();
    const asked: string[][] = [];
    let answer = true;
    const question = async (open: readonly string[]): Promise<boolean> => { asked.push([...open]); return answer; };
    const meet = "https://meet.google.com";
    const both = [grants.ask(meet, ["camera", "microphone"], question), grants.ask(meet, ["camera", "microphone"], question)];
    expect(await Promise.all(both)).toEqual([true, true]);
    expect(asked).toEqual([["camera", "microphone"]]);
    expect(await grants.ask(meet, ["camera"], question)).toBe(true);
    expect(grants.granted(meet, "camera")).toBe(true);
    expect(grants.granted("https://evil.example", "camera")).toBe(false);
    answer = false;
    expect(await grants.ask(meet, ["notifications"], question)).toBe(false);
    expect(await grants.ask(meet, ["notifications"], question)).toBe(false);
    expect(asked).toHaveLength(2);
    expect(await grants.ask(meet, ["notifications", "camera"], question)).toBe(false);
    expect(asked).toHaveLength(2);
    // Blocked once, refused after without asking, alone or in a combined request.
    const zoom = "https://zoom.us";
    answer = false;
    expect(await grants.ask(zoom, ["microphone"], question)).toBe(false);
    answer = true;
    expect(await grants.ask(zoom, ["microphone"], question)).toBe(false);
    expect(await grants.ask(zoom, ["camera", "microphone"], question)).toBe(false);
    expect(asked).toEqual([["camera", "microphone"], ["notifications"], ["microphone"]]);
    expect(grants.granted(zoom, "camera")).toBe(false);
    expect(await grants.ask("https://other.example", ["camera"], async () => { throw new Error("no window"); })).toBe(false);
  });

  it("words the sheet with the site and what it wants", () => {
    expect(browserQuestion("meet.google.com", ["camera", "microphone"]).message)
      .toBe("“meet.google.com” wants to use your camera and microphone");
    expect(browserQuestion("example.com", ["notifications"]).message).toBe("“example.com” wants to show notifications");
    expect(browserQuestion("zoom.us", ["external:zoommtg"]).message)
      .toBe("“zoom.us” wants to open “zoommtg:” links in another app");
  });

  it("includes a truncated destination URL preview in external link prompts", () => {
    const short = browserQuestion("zoom.us", ["external:zoommtg"], "zoommtg://zoom.us/join?confno=123");
    expect(short.detail).toBe(
      "Destination: zoommtg://zoom.us/join?confno=123\n\nYour answer holds for this site until the app quits.",
    );
    const longUrl = `zoommtg://zoom.us/join?confno=${"9".repeat(200)}\u202e`;
    const truncated = browserQuestion("zoom.us", ["external:zoommtg"], longUrl);
    expect(truncated.detail).not.toContain("\u202e");
    expect(browserExternalPreview(longUrl).endsWith("…")).toBe(true);
    expect(Array.from(browserExternalPreview(longUrl))).toHaveLength(120);
    expect(truncated.detail).toContain(`Destination: ${browserExternalPreview(longUrl)}`);
  });
});

class FakeContents extends EventEmitter {
  zoom = 1;
  calls: string[] = [];
  openHandler: ((details: { url: string; disposition: string; features: string }) => { action: string }) | null = null;
  readonly navigationHistory = {
    canGoBack: () => true, canGoForward: () => false,
    goBack: () => { this.calls.push("back"); }, goForward: () => { this.calls.push("forward"); },
  };
  constructor(readonly id: number, private readonly url = "https://example.com/") { super(); }
  getURL(): string { return this.url; }
  setWindowOpenHandler(handler: FakeContents["openHandler"]): void { this.openHandler = handler; }
  getZoomFactor(): number { return this.zoom; }
  setZoomFactor(factor: number): void { this.zoom = factor; }
  reload(): void { this.calls.push("reload"); }
  reloadIgnoringCache(): void { this.calls.push("hard-reload"); }
  executeJavaScript(code: string): Promise<unknown> { this.calls.push(code); return Promise.resolve(); }
  downloadURL(url: string): void { this.calls.push(`download ${url}`); }
}

type RequestHandler = (contents: unknown, permission: string, decide: (allow: boolean) => void, details: unknown) => void;
type CheckHandler = (contents: unknown, permission: string, origin: string, details: { mediaType?: string }) => boolean;

class FakeSession extends EventEmitter {
  request: RequestHandler | null = null;
  check: CheckHandler | null = null;
  device: (() => boolean) | null = null;
  readonly protocol = { handle: (): void => undefined };
  setPermissionRequestHandler(handler: RequestHandler): void { this.request = handler; }
  setPermissionCheckHandler(handler: CheckHandler): void { this.check = handler; }
  setDevicePermissionHandler(handler: () => boolean): void { this.device = handler; }
  getUserAgent(): string { return "Chrome/1 Electron/43"; }
  setUserAgent(): void { /* fixed agent */ }
}

/** Lets promise chains with no real I/O run to the end. */
const settle = (): Promise<void> => new Promise(resolve => { setImmediate(resolve); });

function installFake(folder: string, extra: Partial<BrowserPaneHooks> = {}) {
  const host = new EventEmitter();
  const session = new FakeSession();
  const sent: BrowserEventV1[] = [];
  const questions: BrowserQuestion[] = [];
  const opened: string[] = [];
  const savePrompts: Array<{ defaultPath: string; name: string }> = [];
  let answer = true;
  let chosenSavePath: string | null = null;
  installBrowserPaneGuards(host as unknown as WebContents, session as unknown as Session, {
    downloads: () => folder,
    ask: async question => { questions.push(question); return answer; },
    showMenu: () => undefined,
    openExternal: async url => { opened.push(url); },
    copyText: () => undefined,
    send: event => { sent.push(event); },
    window: () => null,
    showPopups: false,
    chooseSavePath: (defaultPath, name) => { savePrompts.push({ defaultPath, name }); return chosenSavePath; },
    // The fake downloads write no files; the real flag is tested on a real one below.
    quarantine: async () => undefined,
    ...extra,
  });
  const attach = (page: FakeContents): void => { host.emit("did-attach-webview", {}, page); };
  const request = (permission: string, details: Record<string, unknown>): Promise<boolean> =>
    new Promise(resolve => { session.request!({}, permission, resolve, details); });
  return {
    session, sent, questions, opened, savePrompts, attach, request,
    setAnswer: (value: boolean) => { answer = value; },
    setChosenSavePath: (value: string | null) => { chosenSavePath = value; },
  };
}

describe("Browser guards installed on a page", () => {
  it("decides each permission in main, asking once, and checks agree with what was granted", async () => {
    const fake = installFake(tmpdir());
    const site = "https://meet.google.com";
    expect(fake.session.check!({}, "media", site, { mediaType: "video" })).toBe(false);
    expect(await fake.request("media", { requestingUrl: `${site}/abc`, mediaTypes: ["video", "audio"] })).toBe(true);
    expect(fake.questions.map(question => question.message))
      .toEqual(["“meet.google.com” wants to use your camera and microphone"]);
    expect(fake.session.check!({}, "media", site, { mediaType: "video" })).toBe(true);
    expect(fake.session.check!({}, "media", "https://other.example", { mediaType: "video" })).toBe(false);
    expect(await fake.request("media", { requestingUrl: `${site}/xyz`, mediaTypes: ["video"] })).toBe(true);
    expect(fake.questions).toHaveLength(1);
    expect(await fake.request("geolocation", { requestingUrl: `${site}/` })).toBe(false);
    expect(await fake.request("fullscreen", { requestingUrl: `${site}/` })).toBe(true);
    expect(fake.questions).toHaveLength(1);
    expect(fake.session.check!({}, "geolocation", site, {})).toBe(false);
    expect(fake.session.device!()).toBe(false);
    fake.setAnswer(false);
    expect(await fake.request("notifications", { requestingUrl: "https://example.com/" })).toBe(false);
    expect(fake.session.check!({}, "notifications", "https://example.com", {})).toBe(false);
  });

  it("opens new tabs through the renderer, asks before another app, and refuses the rest", async () => {
    const fake = installFake(tmpdir());
    const page = new FakeContents(41);
    fake.attach(page);
    // Each opening follows a press in the page, as a click on a link does.
    const open = (url: string, disposition: string, features = "") => {
      page.emit("input-event", {}, { type: "mouseDown" });
      return page.openHandler!({ url, disposition, features });
    };
    expect(open("https://example.com/next", "foreground-tab")).toEqual({ action: "deny" });
    expect(open("https://example.com/later", "background-tab")).toEqual({ action: "deny" });
    expect(fake.sent).toEqual([
      { version: 1, kind: "open-tab", page: 41, url: "https://example.com/next", background: false },
      { version: 1, kind: "open-tab", page: 41, url: "https://example.com/later", background: true },
    ]);
    expect(open("https://accounts.example.com/", "new-window", "width=400,height=500")).toMatchObject({
      action: "allow", overrideBrowserWindowOptions: { show: false, webPreferences: { sandbox: true, contextIsolation: true } },
    });
    expect(open("file:///etc/hosts", "foreground-tab")).toEqual({ action: "deny" });
    expect(open("mailto:a@example.com", "foreground-tab")).toEqual({ action: "deny" });
    await new Promise(resolve => setTimeout(resolve, 0));
    expect(fake.questions.at(-1)?.message).toBe("“example.com” wants to open “mailto:” links in another app");
    expect(fake.questions.at(-1)?.detail).toContain("Destination: mailto:a@example.com");
    expect(fake.opened).toEqual(["mailto:a@example.com"]);
    expect(fake.sent).toHaveLength(2);
    // Without a fresh press, neither window.open nor will-navigate opens another app,
    // and will-redirect never opens another app even right after a press.
    expect(page.openHandler!({ url: "mailto:no-press@example.com", disposition: "foreground-tab", features: "" }))
      .toEqual({ action: "deny" });
    let prevented = 0;
    page.emit("will-navigate", { url: "mailto:no-press@example.com", isMainFrame: true, preventDefault: () => { prevented++; } });
    page.emit("input-event", {}, { type: "mouseDown" });
    page.emit("will-redirect", { url: "mailto:redirect@example.com", isMainFrame: true, preventDefault: () => { prevented++; } });
    await new Promise(resolve => setTimeout(resolve, 0));
    expect(fake.opened).toEqual(["mailto:a@example.com"]);
    expect(prevented).toBe(2);
    page.emit("will-navigate", { url: "file:///etc/hosts", isMainFrame: true, preventDefault: () => { prevented++; } });
    page.emit("will-navigate", { url: "javascript:alert(1)", isMainFrame: true, preventDefault: () => { prevented++; } });
    page.emit("will-navigate", { url: "https://example.com/ok", isMainFrame: true, preventDefault: () => { prevented++; } });
    expect(prevented).toBe(4);
  });

  it("takes the page's chords in the page and leaves the app's alone", () => {
    const fake = installFake(tmpdir());
    const page = new FakeContents(7);
    fake.attach(page);
    const press = (key: string, modifiers: Partial<Record<"meta" | "shift" | "control" | "alt", boolean>> = {}): boolean => {
      let prevented = false;
      page.emit("before-input-event", { preventDefault: () => { prevented = true; } }, {
        type: "keyDown", key, meta: false, shift: false, control: false, alt: false, ...modifiers,
      });
      return prevented;
    };
    expect(press("=", { meta: true })).toBe(true);
    expect(page.zoom).toBe(1.1);
    expect(press("-", { meta: true }) && press("-", { meta: true })).toBe(true);
    expect(page.zoom).toBe(0.9);
    expect(press("0", { meta: true })).toBe(true);
    expect(page.zoom).toBe(1);
    expect(press("r", { meta: true }) && press("R", { meta: true, shift: true })
      && press("[", { meta: true }) && press("]", { meta: true })).toBe(true);
    expect(page.calls).toEqual(["reload", "hard-reload", "back"]);
    expect(press("f", { meta: true }) && press("l", { meta: true })).toBe(true);
    expect(fake.sent).toEqual([{ version: 1, kind: "find", page: 7 }, { version: 1, kind: "address", page: 7 }]);
    for (const [key, modifiers] of [["w", { meta: true }], ["t", { meta: true }], ["q", { meta: true }],
      ["p", { meta: true, shift: true }], ["d", { meta: true }], ["[", { meta: true, shift: true }],
      ["r", { control: true }], ["Escape", {}], ["a", {}]] as const) {
      expect(press(key, modifiers), `${key} ${JSON.stringify(modifiers)}`).toBe(false);
    }
    // Esc leaves a page's fullscreen.
    page.emit("enter-html-full-screen");
    expect(press("Escape")).toBe(true);
    expect(page.calls.at(-1)).toContain("document.exitFullscreen()");
  });

  it("saves downloads into the folder it was given, Chrome's way, and says so", async () => {
    const folder = mkdtempSync(join(tmpdir(), "browser-downloads-"));
    try {
      writeFileSync(join(folder, "report.pdf"), "already here");
      const fake = installFake(folder);
      const page = new FakeContents(9);
      fake.attach(page);
      const download = (name: string) => {
        const item = Object.assign(new EventEmitter(), {
          path: "", getFilename: () => name, setSavePath(path: string) { this.path = path; },
        });
        fake.session.emit("will-download", {}, item, page);
        return item;
      };
      const first = download("report.pdf");
      const second = download("report.pdf");
      expect([first.path, second.path]).toEqual([join(folder, "report (1).pdf"), join(folder, "report (2).pdf")]);
      first.emit("done", {}, "completed");
      // Saved is said once the file is flagged for Gatekeeper.
      await settle();
      second.emit("done", {}, "interrupted");
      expect(fake.sent).toEqual([
        { version: 1, kind: "download", page: 9, state: "started", name: "report (1).pdf" },
        { version: 1, kind: "download", page: 9, state: "started", name: "report (2).pdf" },
        { version: 1, kind: "download", page: 9, state: "saved", name: "report (1).pdf" },
        { version: 1, kind: "download", page: 9, state: "failed", name: "report (2).pdf" },
      ]);
      expect(readFileSync(join(folder, "report.pdf"), "utf8")).toBe("already here");
    } finally { rmSync(folder, { recursive: true, force: true }); }
  });

  it("fails a download whose connection drops, frees its name and cancels it", async () => {
    const folder = mkdtempSync(join(tmpdir(), "browser-downloads-"));
    try {
      const fake = installFake(folder);
      const page = new FakeContents(9);
      fake.attach(page);
      const download = (name: string) => {
        const item = Object.assign(new EventEmitter(), {
          path: "", cancelled: 0, getFilename: () => name, setSavePath(path: string) { this.path = path; },
          cancel() { this.cancelled += 1; },
        });
        fake.session.emit("will-download", {}, item, page);
        return item;
      };
      const dropped = download("data.bin");
      dropped.emit("updated", {}, "progressing");
      expect(fake.sent.map(event => event.kind === "download" && event.state)).toEqual(["started"]);
      // Electron leaves a dropped download interrupted and never calls it done.
      dropped.emit("updated", {}, "interrupted");
      expect(fake.sent.at(-1)).toEqual({ version: 1, kind: "download", page: 9, state: "failed", name: "data.bin" });
      await new Promise(resolve => setImmediate(resolve));
      expect(dropped.cancelled).toBe(1);
      // The cancel's own done says nothing more, and the name is free again.
      dropped.emit("done", {}, "cancelled");
      expect(fake.sent).toHaveLength(2);
      expect(download("data.bin").path).toBe(join(folder, "data.bin"));
    } finally { rmSync(folder, { recursive: true, force: true }); }
  });
});

class FakePopup extends EventEmitter {
  title = "";
  destroyed = false;
  readonly webContents: FakeContents & { session: unknown };
  constructor(id: number, session: unknown, url: string) {
    super();
    this.webContents = Object.assign(new FakeContents(id, url), { session });
  }
  setTitle(title: string): void { this.title = title; }
  getTitle(): string { return this.title; }
  destroy(): void { this.destroyed = true; }
  close(): void { this.emit("closed"); }
}

describe("A page opens a tab or window only just after a press in it", () => {
  afterEach(() => { vi.useRealTimers(); });
  const mouse = (page: FakeContents): void => { page.emit("input-event", {}, { type: "mouseDown" }); };
  const key = (page: FakeContents, name: string): void => {
    page.emit("before-input-event", { preventDefault: () => undefined },
      { type: "keyDown", key: name, meta: false, control: false, alt: false, shift: false, isAutoRepeat: false });
  };
  const popup = { url: "https://accounts.example.com/signin", disposition: "new-window", features: "width=400,height=500" };
  const tab = { url: "https://example.com/next", disposition: "foreground-tab", features: "" };

  it("refuses a window.open with no press, and opens one per press", () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(1_000_000);
    const fake = installFake(tmpdir());
    const page = new FakeContents(51);
    fake.attach(page);
    // A script with no press opens nothing, quietly.
    expect(page.openHandler!(tab)).toEqual({ action: "deny" });
    expect(page.openHandler!(popup)).toEqual({ action: "deny" });
    expect(fake.sent).toEqual([]);
    // A click opens one tab; a second opening on the same click opens nothing.
    mouse(page);
    expect(page.openHandler!(tab)).toEqual({ action: "deny" });
    expect(page.openHandler!({ ...tab, url: "https://example.com/again" })).toEqual({ action: "deny" });
    expect(fake.sent).toEqual([{ version: 1, kind: "open-tab", page: 51, url: "https://example.com/next", background: false }]);
    // A key press opens a sign-in popup; Escape is no press.
    key(page, "Enter");
    expect(page.openHandler!(popup)).toMatchObject({ action: "allow" });
    key(page, "Escape");
    expect(page.openHandler!(popup)).toEqual({ action: "deny" });
    // A press counts for Chrome's five seconds, no longer.
    mouse(page);
    vi.setSystemTime(1_000_000 + BROWSER_ACTIVATION_MS + 1);
    expect(page.openHandler!(tab)).toEqual({ action: "deny" });
    expect(fake.sent).toHaveLength(1);
  });

  it("keeps at most two popup windows open per tab", () => {
    const fake = installFake(tmpdir());
    const page = new FakeContents(52);
    fake.attach(page);
    const windows: FakePopup[] = [];
    const openPopup = (): { action: string } => {
      mouse(page);
      const answer = page.openHandler!(popup);
      if (answer.action === "allow") {
        const window = new FakePopup(100 + windows.length, fake.session, popup.url);
        windows.push(window);
        page.emit("did-create-window", window, { url: popup.url });
      }
      return answer;
    };
    expect(openPopup()).toMatchObject({ action: "allow" });
    expect(openPopup()).toMatchObject({ action: "allow" });
    expect(openPopup()).toEqual({ action: "deny" });
    expect(MAX_BROWSER_POPUPS).toBe(2);
    // A popup's own popup counts against its tab too.
    mouse(windows[0]!.webContents);
    expect(windows[0]!.webContents.openHandler!(popup)).toEqual({ action: "deny" });
    // One closes: there is room for one more.
    windows[0]!.close();
    expect(openPopup()).toMatchObject({ action: "allow" });
    expect(windows).toHaveLength(3);
  });

  it("titles a popup window with its site first, whatever the page calls itself", () => {
    expect(browserPopupTitle("https://accounts.example.com/signin?x=1", "Interface — sign in again"))
      .toBe("accounts.example.com — Interface — sign in again");
    expect(browserPopupTitle("https://accounts.example.com/", "")).toBe("accounts.example.com");
    expect(browserPopupTitle("https://accounts.example.com/", "  \u202eInterface\u0007  ")).toBe("accounts.example.com — Interface");
    expect(browserPopupTitle("http://127.0.0.1:5173/", "http://127.0.0.1:5173/")).toBe("127.0.0.1:5173");
    const fake = installFake(tmpdir());
    const page = new FakeContents(53);
    fake.attach(page);
    mouse(page);
    expect(page.openHandler!(popup)).toMatchObject({ action: "allow" });
    const window = new FakePopup(200, fake.session, popup.url);
    page.emit("did-create-window", window, { url: popup.url });
    expect(window.title).toBe("accounts.example.com");
    let kept = true;
    window.emit("page-title-updated", { preventDefault: () => { kept = false; } }, "Interface — sign in again");
    expect(kept).toBe(false);
    expect(window.title).toBe("accounts.example.com — Interface — sign in again");
    window.webContents.emit("did-navigate", {}, "https://evil.example.net/next");
    expect(window.title).toBe("evil.example.net — Interface — sign in again");
  });
});

describe("Browser download symlink and installer prompts", () => {
  it("never overwrites a dangling symlink in Downloads and prompts before executable or installer downloads", async () => {
    const folder = mkdtempSync(join(tmpdir(), "browser-downloads-safe-"));
    try {
      // A dangling symlink reports false from existsSync, so lstatSync must guard it.
      const dangling = join(folder, "report.pdf");
      symlinkSync(join(folder, "missing-target-file"), dangling);
      expect(browserPathExists(dangling)).toBe(true);
      expect(browserPathExists(join(folder, "absent.pdf"))).toBe(false);

      const fake = installFake(folder);
      const page = new FakeContents(11);
      fake.attach(page);
      const download = (name: string) => {
        let prevented = false;
        const item = Object.assign(new EventEmitter(), {
          path: "", cancelled: 0, getFilename: () => name,
          setSavePath(path: string) { this.path = path; },
          cancel() { this.cancelled += 1; },
        });
        // The owner clicked the link: an installer a page starts on its own is refused before any Save As.
        page.emit("input-event", {}, { type: "mouseDown" });
        fake.session.emit("will-download", { preventDefault: () => { prevented = true; } }, item, page);
        return { item, prevented };
      };

      const safePdf = download("report.pdf");
      expect(safePdf.item.path).toBe(join(folder, "report (1).pdf"));
      expect(fake.savePrompts).toEqual([]);

      const dangerousExtensions = [
        ".command", ".sh", ".zsh", ".terminal", ".app", ".pkg", ".dmg", ".webloc", ".inetloc", ".fileloc",
      ];
      for (const ext of dangerousExtensions) {
        expect(browserDownloadRequiresSaveDialog(`payload${ext}`), ext).toBe(true);
        expect(browserDownloadRequiresSaveDialog(`PAYLOAD${ext.toUpperCase()}`), ext).toBe(true);
      }
      expect(browserDownloadRequiresSaveDialog("notes.txt")).toBe(false);

      // Cancelled / unconfirmed save dialog refuses the download without setting savePath.
      fake.setChosenSavePath(null);
      const refused = download("run-me.command");
      expect(refused.prevented).toBe(true);
      expect(refused.item.cancelled).toBe(1);
      expect(refused.item.path).toBe("");
      expect(fake.savePrompts).toEqual([{ defaultPath: join(folder, "run-me.command"), name: "run-me.command" }]);
      expect(fake.sent.at(-1)).toEqual({ version: 1, kind: "download", page: 11, state: "failed", name: "run-me.command" });

      // Confirmed save dialog writes only to the owner's chosen path.
      const chosen = join(folder, "verified-installer.pkg");
      fake.setChosenSavePath(chosen);
      const accepted = download("installer.pkg");
      expect(accepted.prevented).toBe(false);
      expect(accepted.item.path).toBe(chosen);
      accepted.item.emit("done", {}, "completed");
      await settle();
      expect(fake.sent.at(-1)).toEqual({ version: 1, kind: "download", page: 11, state: "saved", name: "verified-installer.pkg" });
    } finally { rmSync(folder, { recursive: true, force: true }); }
  });
});

const SCREEN: Rectangle = { x: 0, y: 25, width: 1440, height: 875 };

describe("A page's popup is an ordinary window the app builds", () => {
  const popupRequest = { url: "https://accounts.example.com/signin", disposition: "new-window", features: "width=400,height=500" };
  /** A popup window over the guest page Electron hands to createWindow. */
  class GuestWindow extends EventEmitter {
    title = "";
    destroyed = false;
    constructor(readonly webContents: FakeContents, public bounds: Rectangle) { super(); }
    setTitle(title: string): void { this.title = title; }
    destroy(): void { this.destroyed = true; }
    getBounds(): Rectangle { return this.bounds; }
    setBounds(bounds: Rectangle): void { this.bounds = bounds; }
    close(): void { this.emit("closed"); }
  }
  type OpenAnswer = { action: string; createWindow?: (options: BrowserWindowConstructorOptions) => WebContents };

  it("builds a page's popup window itself: framed, closable, movable, opaque, focusable and never on top", () => {
    const made: BrowserWindowConstructorOptions[] = [];
    const windows: GuestWindow[] = [];
    const fake = installFake(tmpdir(), {
      workArea: () => SCREEN,
      popupWindow: options => {
        made.push(options);
        const window = new GuestWindow((options as { webContents: FakeContents }).webContents,
          { x: options.x!, y: options.y!, width: options.width!, height: options.height! });
        windows.push(window);
        return window as unknown as BrowserWindow;
      },
    });
    const page = new FakeContents(61);
    fake.attach(page);
    const open = (): OpenAnswer => {
      page.emit("input-event", {}, { type: "mouseDown" });
      return page.openHandler!(popupRequest) as OpenAnswer;
    };
    const answer = open();
    expect(answer.action).toBe("allow");
    expect(typeof answer.createWindow).toBe("function");
    // Everything Electron lets a page's window.open features set, and a key it might add later.
    const guest = Object.assign(new FakeContents(62, popupRequest.url), { session: fake.session });
    const hostile = {
      show: true, width: 6000, height: 4000, x: -5000, y: -5000, frame: false, alwaysOnTop: true, closable: false,
      movable: false, minimizable: false, maximizable: false, focusable: false, transparent: true, opacity: 0.05,
      hasShadow: false, enableLargerThanScreen: true, hiddenInMissionControl: true, skipTaskbar: true,
      titleBarStyle: "hidden", modal: true, kiosk: true, fullscreen: true, acceptFirstMouse: true, useContentSize: true,
      title: "Interface", tabbingIdentifier: "interface", vibrancy: "sidebar", roundedCorners: false,
      someFutureWindowOption: true, webContents: guest,
    } as unknown as BrowserWindowConstructorOptions;
    expect(answer.createWindow!(hostile)).toBe(guest);
    expect(made).toHaveLength(1);
    const options = made[0]!;
    expect(options).toMatchObject({
      show: false, frame: true, titleBarStyle: "default", alwaysOnTop: false, closable: true, movable: true,
      minimizable: true, maximizable: true, resizable: true, focusable: true, transparent: false, opacity: 1,
      hasShadow: true, modal: false, kiosk: false, fullscreen: false, skipTaskbar: false, hiddenInMissionControl: false,
      enableLargerThanScreen: false, acceptFirstMouse: false, useContentSize: false,
      title: "accounts.example.com", webContents: guest,
      // Never larger than the screen's work area, and on it.
      x: SCREEN.x, y: SCREEN.y, width: SCREEN.width, height: SCREEN.height,
    });
    for (const key of ["tabbingIdentifier", "vibrancy", "roundedCorners", "someFutureWindowOption", "webPreferences"]) {
      expect(options, key).not.toHaveProperty(key);
    }
    expect(Object.keys(options).sort()).toEqual(Object.keys({
      ...browserPopupWindowOptions(null, false, "", SCREEN), webContents: guest,
    }).sort());
    // The window is the app's popup: titled with its site, guarded like its opener, counted against the tab.
    expect(windows[0]!.title).toBe("accounts.example.com");
    expect(guest.openHandler).not.toBeNull();
    const second = { width: 500, height: 600, webContents: Object.assign(new FakeContents(63), { session: fake.session }) };
    open().createWindow!(second as unknown as BrowserWindowConstructorOptions);
    expect(open()).toEqual({ action: "deny" });
    expect(MAX_BROWSER_POPUPS).toBe(2);
  });

  it("keeps a popup window on its screen, no larger than it and never too small to show its site", () => {
    // Larger than the screen, or off it: the work area, on it.
    expect(browserPopupBounds({ width: 6000, height: 4000, x: -5000, y: -5000 }, SCREEN, null)).toEqual(SCREEN);
    expect(browserPopupBounds({ width: 500, height: 600, x: 1300, y: 800 }, SCREEN, null))
      .toEqual({ x: 940, y: 300, width: 500, height: 600 });
    // Too small to read: the least size, centred on the screen.
    expect(browserPopupBounds({ width: 1, height: 1 }, SCREEN, null))
      .toEqual({ x: 560, y: 343, ...MIN_BROWSER_POPUP });
    // No size or place asked for (or nonsense): 800 × 600, centred on the app window.
    const app = { x: 100, y: 100, width: 1000, height: 700 };
    expect(browserPopupBounds({}, SCREEN, app)).toEqual({ x: 200, y: 150, width: 800, height: 600 });
    expect(browserPopupBounds({ width: Number.NaN, height: "tall", x: Number.POSITIVE_INFINITY }, SCREEN, app))
      .toEqual({ x: 200, y: 150, width: 800, height: 600 });
    // A screen smaller than the least size: the whole screen.
    expect(browserPopupBounds({ width: 1, height: 1 }, { x: 0, y: 0, width: 300, height: 200 }, null))
      .toEqual({ x: 0, y: 0, width: 300, height: 200 });
    // window.resizeTo and moveTo later keep to the same bounds.
    const window = Object.assign(new EventEmitter(), {
      title: "", bounds: { x: 200, y: 150, width: 400, height: 500 },
      setTitle(title: string) { this.title = title; }, getBounds() { return this.bounds; },
      setBounds(bounds: Rectangle) { this.bounds = bounds; }, destroy() { /* kept */ },
    });
    const fake = installFake(tmpdir(), { workArea: () => SCREEN, popupWindow: options => Object.assign(window,
      { webContents: (options as { webContents: FakeContents }).webContents }) as unknown as BrowserWindow });
    const page = new FakeContents(64);
    fake.attach(page);
    page.emit("input-event", {}, { type: "mouseDown" });
    const guest = Object.assign(new FakeContents(65, popupRequest.url), { session: fake.session });
    (page.openHandler!(popupRequest) as OpenAnswer).createWindow!({ webContents: guest } as unknown as BrowserWindowConstructorOptions);
    let prevented = false;
    guest.emit("content-bounds-updated", { preventDefault: () => { prevented = true; } }, { x: -9000, y: 9000, width: 1, height: 1 });
    expect(prevented).toBe(true);
    expect(window.bounds).toEqual({ x: 0, y: SCREEN.y + SCREEN.height - MIN_BROWSER_POPUP.height, ...MIN_BROWSER_POPUP });
    guest.emit("content-bounds-updated", { preventDefault: () => undefined }, { x: 0, y: 0, width: 9000, height: 9000 });
    expect(window.bounds).toEqual(SCREEN);
  });
});

describe("Browser downloads ask when the page starts them alone, and are flagged for Gatekeeper", () => {
  /** A download as Electron hands it over: its addresses, whether a press started it, and its controls. */
  const downloadIn = (session: FakeSession, page: FakeContents, name: string, url: string, gesture = false) => {
    const item = Object.assign(new EventEmitter(), {
      path: "", paused: 0, resumed: 0, cancelled: 0,
      getFilename: () => name, getURL: () => url, getURLChain: () => [url], hasUserGesture: () => gesture,
      setSavePath(path: string) { this.path = path; },
      pause() { this.paused += 1; }, resume() { this.resumed += 1; }, cancel() { this.cancelled += 1; },
    });
    session.emit("will-download", { preventDefault: () => undefined }, item, page);
    return item;
  };
  const folders: string[] = [];
  const folder = (): string => { const made = mkdtempSync(join(tmpdir(), "browser-flagged-")); folders.push(made); return made; };
  afterEach(() => { for (const made of folders.splice(0)) rmSync(made, { recursive: true, force: true }); });

  it("flags a finished download for Gatekeeper before calling it saved, and fails and removes one it cannot flag", async () => {
    const flagged: Array<{ path: string; origin: readonly string[] }> = [];
    let flag = async (): Promise<void> => undefined;
    const fake = installFake(folder(), { quarantine: async (path, origin) => { flagged.push({ path, origin }); await flag(); } });
    const page = new FakeContents(71, "https://files.example.com/list");
    fake.attach(page);
    const saved = downloadIn(fake.session, page, "report.pdf", "https://reader:secret@files.example.com/report.pdf", true);
    writeFileSync(saved.path, "numbers");
    saved.emit("done", {}, "completed");
    // Not saved until it is flagged.
    expect(fake.sent.at(-1)).toMatchObject({ kind: "download", state: "started", name: "report.pdf" });
    await settle();
    expect(flagged).toEqual([{ path: saved.path,
      origin: ["https://files.example.com/report.pdf", "https://files.example.com/list"] }]);
    expect(fake.sent.at(-1)).toEqual({ version: 1, kind: "download", page: 71, state: "saved", name: "report.pdf" });
    // A file that cannot be flagged is never called saved, and does not stay.
    flag = async () => { throw new Error("The volume took no attributes."); };
    const unflagged = downloadIn(fake.session, page, "tool.zip", "https://files.example.com/tool.zip", true);
    writeFileSync(unflagged.path, "PK");
    unflagged.emit("done", {}, "completed");
    await vi.waitFor(() => {
      expect(fake.sent.at(-1)).toEqual({ version: 1, kind: "download", page: 71, state: "failed", name: "tool.zip" });
    });
    expect(existsSync(unflagged.path)).toBe(false);
    expect(fake.sent.filter(event => event.kind === "download" && event.state === "saved")).toHaveLength(1);
  });

  it("writes macOS's quarantine attribute as a browser does for a web download", async () => {
    // Flags 0081 (a web download, not sandboxed), the time in hexadecimal seconds, the app, the event's id.
    expect(browserQuarantineValue(Date.UTC(2026, 10, 23, 12, 0, 0), "9254457E-7C49-47A0-9603-831CA167C137"))
      .toBe("0081;6b042ac0;Interface;9254457E-7C49-47A0-9603-831CA167C137");
    expect(browserDownloadOrigin("https://u:p@example.com/a.zip?x=1", "https://example.com/page"))
      .toEqual(["https://example.com/a.zip?x=1", "https://example.com/page"]);
    expect(browserDownloadOrigin("blob:https://example.com/1234", "https://example.com/page")).toEqual(["https://example.com/page"]);
    expect(browserDownloadOrigin("data:text/plain,hi", "")).toEqual([]);
    expect(browserDownloadOrigin("https://example.com/", "https://example.com/")).toEqual(["https://example.com/"]);
    // A binary property list: the header, one array of two short strings, its offsets and trailer.
    const plist = browserWhereFroms(["a", "bc"]);
    expect(plist.subarray(0, 8).toString("latin1")).toBe("bplist00");
    expect([...plist.subarray(8, 16)]).toEqual([0xa2, 1, 2, 0x51, 0x61, 0x52, 0x62, 0x63]);
    expect([...plist.subarray(16, 19)]).toEqual([8, 11, 13]);
    expect(plist).toHaveLength(19 + 32);
    if (process.platform !== "darwin") return;
    // On a real file: macOS reads both back as written.
    const made = folder();
    const file = join(made, "tool.zip");
    writeFileSync(file, "PK");
    const long = `https://example.com/${"é".repeat(150)}`;
    await quarantineBrowserDownload(file, ["https://example.com/tool.zip", long]);
    const value = execFileSync("/usr/bin/xattr", ["-p", BROWSER_QUARANTINE_ATTRIBUTE, file], { encoding: "utf8" }).trim();
    expect(value).toMatch(/^0081;[0-9a-f]{8};Interface;[0-9A-F]{8}-[0-9A-F]{4}-4[0-9A-F]{3}-[89AB][0-9A-F]{3}-[0-9A-F]{12}$/u);
    expect(Math.abs(Number.parseInt(value.split(";")[1]!, 16) - Date.now() / 1000)).toBeLessThan(120);
    const where = join(made, "where.plist");
    writeFileSync(where, Buffer.from(execFileSync("/usr/bin/xattr", ["-px", BROWSER_WHERE_FROMS_ATTRIBUTE, file],
      { encoding: "utf8" }).replace(/\s+/gu, ""), "hex"));
    expect(JSON.parse(execFileSync("/usr/bin/plutil", ["-convert", "json", "-o", "-", where], { encoding: "utf8" })))
      .toEqual(["https://example.com/tool.zip", long]);
    // Anything but a plain file, or none, is refused, so it is never called saved.
    const inner = join(made, "folder.app");
    mkdirSync(inner);
    await expect(quarantineBrowserDownload(inner, [])).rejects.toThrow();
    await expect(quarantineBrowserDownload(join(made, "gone.zip"), [])).rejects.toThrow();
  });

  it("asks before saving a download the page started without a click, and never for the owner's own", async () => {
    const fake = installFake(folder());
    const page = new FakeContents(81, "https://files.example.com/");
    fake.attach(page);
    const download = (name: string, gesture = false) =>
      downloadIn(fake.session, page, name, `https://files.example.com/${name}`, gesture);
    // A script's download with no press is held, and asks with the site and the file named; Block cancels it.
    fake.setAnswer(false);
    const sneaky = download("invoice.zip");
    expect(sneaky.paused).toBe(1);
    await settle();
    expect(fake.questions.map(question => [question.message, question.detail])).toEqual([[
      "“files.example.com” wants to download files without a click",
      "File: invoice.zip\n\nYour answer holds for this site until the app quits."]]);
    expect([sneaky.resumed, sneaky.cancelled]).toEqual([0, 1]);
    expect(fake.sent.at(-1)).toEqual({ version: 1, kind: "download", page: 81, state: "failed", name: "invoice.zip" });
    // The answer holds without asking again; a file that finished meanwhile is removed, not saved.
    const raced = download("raced.zip");
    writeFileSync(raced.path, "PK");
    raced.emit("done", {}, "completed");
    await vi.waitFor(() => { expect(fake.sent.at(-1)).toMatchObject({ state: "failed", name: "raced.zip" }); });
    expect(existsSync(raced.path)).toBe(false);
    // An installer or script the page starts on its own is refused outright, before any Save As.
    const installer = download("setup.pkg");
    expect([installer.path, installer.cancelled]).toEqual(["", 1]);
    expect(fake.savePrompts).toEqual([]);
    expect(fake.questions).toHaveLength(1);
    // The owner's own, even on that site: a press in the page just before (one download per press)…
    const owners: Array<ReturnType<typeof download>> = [];
    page.emit("input-event", {}, { type: "mouseDown" });
    owners.push(download("clicked.zip"));
    const unpressed = download("unpressed.zip");
    // …a download Chromium saw a press start, the right-click menu's Save Image As…,
    owners.push(download("gesture.zip", true));
    page.downloadURL("https://files.example.com/cat.png");
    owners.push(download("cat.png"));
    // …an address the app loads (the address bar), but not one the page navigates to itself…
    page.emit("did-start-navigation", { url: "https://files.example.com/typed.zip", isMainFrame: true, isSameDocument: false });
    owners.push(download("typed.zip"));
    page.emit("did-start-navigation", { url: "https://files.example.com/scripted.zip", isMainFrame: true,
      isSameDocument: false, initiator: {} });
    page.emit("will-navigate", { url: "https://files.example.com/scripted.zip", isMainFrame: true, preventDefault: () => undefined });
    const scripted = download("scripted.zip");
    // …and a tab with no page yet, opened on a download's address.
    const fresh = new FakeContents(82, "");
    fake.attach(fresh);
    owners.push(downloadIn(fake.session, fresh, "fresh.zip", "https://files.example.com/fresh.zip"));
    await settle();
    for (const owned of owners) {
      expect([owned.path === "", owned.paused, owned.cancelled], owned.path).toEqual([false, 0, 0]);
    }
    expect([unpressed.cancelled, scripted.cancelled]).toEqual([1, 1]);
    expect(fake.questions).toHaveLength(1);
    // Another site allowed once: its download goes on, and the next asks nothing.
    const other = new FakeContents(83, "https://reports.example.org/");
    fake.attach(other);
    fake.setAnswer(true);
    const report = downloadIn(fake.session, other, "report.csv", "https://reports.example.org/report.csv");
    await settle();
    expect([report.paused, report.resumed, report.cancelled]).toEqual([1, 1, 0]);
    const next = downloadIn(fake.session, other, "next.csv", "https://reports.example.org/next.csv");
    expect([next.paused, next.cancelled]).toEqual([0, 0]);
    expect(fake.questions.map(question => question.message)).toEqual([
      "“files.example.com” wants to download files without a click",
      "“reports.example.org” wants to download files without a click",
    ]);
  });
});
