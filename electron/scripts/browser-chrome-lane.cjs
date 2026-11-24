"use strict";
// The browser-chrome renderer lane: a Browser tab behaving like a Chrome tab,
// against real http pages in real <webview>s. This process serves the pages,
// installs the app's own guards (dist-main/main/browser-pane.js) with test
// hooks — a temp Downloads folder, a scripted answer instead of the native
// sheet, a recorded menu instead of the native one, nothing opened outside —
// and drives the pages. The renderer half, the real terminal centre with one
// pane holding a shell and a Browser tab, is
// src/renderer/fixtures/browser-chrome-fixture.tsx.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const http = require("node:http");
const os = require("node:os");
const path = require("node:path");
const { pathToFileURL } = require("node:url");
const { app, BrowserWindow, session, webContents } = require("electron");

const PAGE = `<!doctype html><html><head><meta charset="utf-8"><title>Chrome fixture</title></head>
<body style="margin:0;font:15px sans-serif">
<p><a id="blank" href="/second" target="_blank">Second, in a new tab</a></p>
<p><a id="plain" href="/third">Third</a></p>
<p>needle one, needle two, needle three.</p>
<p><input id="field" value="hello"></p>
</body></html>`;
const SECOND = `<!doctype html><html><head><meta charset="utf-8"><title>Second</title></head>
<body style="margin:0;font:15px sans-serif"><h1>Second page</h1>
<p><a id="later" href="/third">Third, behind</a></p>
<p><a id="report" href="/report.txt">Report</a></p>
<p>needle one, needle two, needle three.</p>
</body></html>`;
const THIRD = `<!doctype html><html><head><title>Third</title></head><body><h1>Third page</h1></body></html>`;
// Counts its timer ticks and animation frames: what a page covered but not parked keeps doing.
const COUNTER = `<!doctype html><html><head><title>Counter</title></head><body style="margin:0;height:4000px">
<script>window.ticks = 0; window.frames = 0; setInterval(() => { window.ticks++; }, 16);
(function frame() { window.frames++; requestAnimationFrame(frame); })();</script></body></html>`;
const POPUP = `<!doctype html><html><head><title>Sign in</title></head><body><h1>Sign in</h1>
<a id="out" href="/third" target="_blank">Elsewhere</a></body></html>`;

/** A one-page PDF with a correct cross-reference table. */
function pdf() {
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Resources << >> >>",
    "<< /Length 0 >>\nstream\n\nendstream",
  ];
  let body = "%PDF-1.4\n";
  const offsets = objects.map((object, index) => {
    const at = Buffer.byteLength(body);
    body += `${index + 1} 0 obj\n${object}\nendobj\n`;
    return at;
  });
  const xref = Buffer.byteLength(body);
  body += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
  for (const at of offsets) body += `${String(at).padStart(10, "0")} 00000 n \n`;
  body += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return Buffer.from(body);
}

async function serve() {
  const document = pdf();
  const server = http.createServer((request, response) => {
    const route = new URL(request.url, "http://fixture").pathname;
    if (route === "/report.txt") {
      response.setHeader("content-type", "text/plain");
      response.setHeader("content-disposition", 'attachment; filename="report.txt"');
      response.end("quarterly numbers\n");
      return;
    }
    if (route === "/broken.bin") {
      // Promises 100000 bytes, sends 1000 and drops the connection.
      response.setHeader("content-type", "application/octet-stream");
      response.setHeader("content-disposition", 'attachment; filename="broken.bin"');
      response.setHeader("content-length", "100000");
      response.write(Buffer.alloc(1000));
      setTimeout(() => request.socket.destroy(), 200);
      return;
    }
    if (route === "/doc.pdf") {
      response.setHeader("content-type", "application/pdf");
      response.end(document);
      return;
    }
    response.setHeader("content-type", "text/html; charset=utf-8");
    response.end(route === "/second" ? SECOND : route === "/third" ? THIRD : route === "/popup" ? POPUP
      : route === "/counter" ? COUNTER : PAGE);
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  return server;
}

exports.exerciseBrowserChrome = async (window, output) => {
  const { installBrowserPaneGuards, BROWSER_PANE_PARTITION } = await import(
    pathToFileURL(path.join(__dirname, "../dist-main/main/browser-pane.js")).href);
  const { BROWSER_EVENT_CHANNEL } = await import(
    pathToFileURL(path.join(__dirname, "../dist-main/shared/browser-events.js")).href);
  // Never the owner's Downloads folder.
  const downloads = await fs.mkdtemp(path.join(os.tmpdir(), "interface-browser-lane-downloads-"));
  const questions = [];
  const menus = [];
  const opened = [];
  let answer = false;
  const pages = session.fromPartition(BROWSER_PANE_PARTITION);
  installBrowserPaneGuards(window.webContents, pages, {
    downloads: () => downloads,
    ask: async question => { questions.push(question.message); return answer; },
    showMenu: template => { menus.push(template); },
    openExternal: async url => { opened.push(url); },
    copyText: () => undefined,
    send: event => { window.webContents.send(BROWSER_EVENT_CHANNEL, event); },
    window: () => window,
    showPopups: false,
  });
  const server = await serve();
  const port = server.address().port;
  const home = `http://127.0.0.1:${port}/`;
  const js = code => window.webContents.executeJavaScript(code);
  const call = code => js(`Promise.resolve(${code}).catch(error => { throw new Error(error?.stack ?? String(error)); })`);
  const state = () => call("window.browserChromeState()");
  const until = async (read, ready, message, timeout = 8000) => {
    const deadline = Date.now() + timeout;
    for (;;) {
      const value = await read();
      if (ready(value)) return value;
      assert.ok(Date.now() < deadline, `${message}: ${JSON.stringify(value)}`);
      await new Promise(resolve => setTimeout(resolve, 40));
    }
  };
  const settle = ms => new Promise(resolve => setTimeout(resolve, ms));
  const guest = id => {
    const contents = webContents.fromId(id);
    assert.ok(contents && !contents.isDestroyed(), `Page ${id} is gone`);
    return contents;
  };
  // With a user gesture, as a click would give.
  const inPage = (contents, code) => contents.executeJavaScript(code, true);
  const loaded = (contents, suffix) => until(async () => contents.getURL(),
    url => url.endsWith(suffix) && !contents.isLoading(), `The page never reached ${suffix}`);
  const chord = async (contents, keyCode, modifiers = ["meta"]) => {
    contents.sendInputEvent({ type: "keyDown", keyCode, modifiers });
    contents.sendInputEvent({ type: "keyUp", keyCode, modifiers });
    await settle(60);
  };
  const report = {};
  try {
    window.setContentSize(1100, 760);
    await call(`window.browserChromeFixture(${JSON.stringify(home)})`);
    const first = await until(state, value => value.pages.length === 1 && value.pages[0].id !== null
      && value.pages[0].url === home, "The Browser tab never attached its page");
    const firstId = first.pages[0].id;
    const page = guest(firstId);
    await loaded(page, "/");
    assert.deepEqual(first.tabs.map(tab => tab.active), [false, true], "The Browser tab is not the one shown");

    // Real input, as the owner's: a page opens a tab or a window only just
    // after a press in it (browser-pane.ts), never from a script alone.
    const at = async (contents, id) => inPage(contents, `(() => { const r = document.getElementById(${JSON.stringify(id)})
      .getBoundingClientRect(); return { x: Math.round(r.x + 4), y: Math.round(r.y + r.height / 2) }; })()`);
    const click = (contents, point, button = "left", modifiers = []) => {
      contents.sendInputEvent({ type: "mouseDown", button, x: point.x, y: point.y, clickCount: 1, modifiers });
      contents.sendInputEvent({ type: "mouseUp", button, x: point.x, y: point.y, clickCount: 1, modifiers });
    };
    /**
     * Waits until the page takes pointer input at `point`: a page that has
     * just loaded can still drop a click (Chromium delivers nothing to it
     * yet, though main sees the press), so moves go in until one arrives.
     * `routed` sends them through the app window, as a real pointer.
     */
    const takesInput = async (contents, point, routed = null) => {
      await inPage(contents, "window.__laneMoves = 0; document.addEventListener('mousemove', () => { window.__laneMoves++; }, true); true");
      const deadline = Date.now() + 8000;
      for (;;) {
        if (routed === null) contents.sendInputEvent({ type: "mouseMove", x: point.x, y: point.y });
        else await window.webContents.debugger.sendCommand("Input.dispatchMouseEvent", { type: "mouseMoved", ...routed, button: "none" });
        await settle(20);
        if (await contents.executeJavaScript("window.__laneMoves") > 0) return;
        assert.ok(Date.now() < deadline, "The page never took pointer input");
      }
    };
    /** A click on nothing in particular: a press the page's next window.open may spend. */
    const press = async contents => { click(contents, { x: 500, y: 400 }); await settle(150); };
    const windowsBesides = () => BrowserWindow.getAllWindows().filter(entry => entry !== window);

    // A script with no press opens no tab and no window, quietly, as Chrome's
    // popup blocker refuses it.
    await page.executeJavaScript("window.open('/second'); window.open('/popup', 'nopress', 'width=300,height=300'); true", false);
    await settle(600);
    const unpressed = await state();
    assert.equal(unpressed.tabs.length, 2, "A window.open with no press opened a tab");
    assert.equal(windowsBesides().length, 0, "A window.open with no press opened a window");
    report.noPress = "no tab, no window";

    // target=_blank opens a second Browser tab in the same pane, right after
    // the page, in front, on the link's page. The click goes to the app
    // window, which routes it into the page, as the owner's does.
    const link = await at(page, "blank");
    const slot = (await state()).slot;
    const routed = { x: Math.round(slot.x + link.x), y: Math.round(slot.y + link.y) };
    await takesInput(page, link, routed);
    for (const type of ["mouseMoved", "mousePressed", "mouseReleased"]) {
      await window.webContents.debugger.sendCommand("Input.dispatchMouseEvent", { type, ...routed,
        button: type === "mouseMoved" ? "none" : "left", buttons: type === "mousePressed" ? 1 : 0, clickCount: type === "mouseMoved" ? 0 : 1 });
    }
    const two = await until(state, value => value.tabs.length === 3 && value.pages.length === 2
      && value.pages.some(entry => entry.id !== firstId && entry.id !== null && !entry.parked),
    "target=_blank did not open a second tab");
    assert.equal(two.panes, 1, "target=_blank opened a pane");
    assert.deepEqual(two.tabs.map(tab => tab.active), [false, false, true], "The new tab is not after the page, in front");
    const secondId = two.pages.find(entry => entry.id !== firstId).id;
    const secondTab = two.tabs[2].id;
    const second = guest(secondId);
    await loaded(second, "/second");
    report.newTab = "in the same pane, after its page, in front";

    // ⌘-click opens one behind, after the page it came from. A real click:
    // Chromium opens a script's synthetic ⌘-click in front.
    const later = await at(second, "later");
    await takesInput(second, later);
    click(second, later, "left", ["meta"]);
    const behind = await until(state, value => value.tabs.length === 4, "⌘-click did not open a tab");
    assert.deepEqual(behind.tabs.map(tab => tab.active), [false, false, true, false], "⌘-click did not open its tab behind");
    report.backgroundTab = "behind, after its page";

    // A download lands in the given folder under Chrome's names.
    await inPage(second, "document.getElementById('report').click(); true");
    await until(async () => (await fs.readdir(downloads)).sort(), names => names.includes("report.txt"),
      "The first download did not land");
    await inPage(second, "document.getElementById('report').click(); true");
    const saved = await until(async () => (await fs.readdir(downloads)).sort(),
      names => names.includes("report (1).txt"), "The second download did not get ' (1)'");
    assert.deepEqual(saved, ["report (1).txt", "report.txt"]);
    assert.equal(await fs.readFile(path.join(downloads, "report (1).txt"), "utf8"), "quarterly numbers\n");
    await until(state, value => value.hints.includes("Saved report (1).txt to Downloads"),
      "The pane did not say where the download went");
    report.downloads = saved;
    // A download whose connection drops says it failed, is cancelled (so
    // Chromium cannot resume it later) and leaves nothing behind.
    const broken = new Promise(resolve => { pages.once("will-download", (_event, item) => { resolve(item); }); });
    second.downloadURL(`http://127.0.0.1:${port}/broken.bin`);
    await until(state, value => value.hints.includes("broken.bin could not be downloaded."),
      "A dropped download did not say it failed", 15_000);
    const afterBroken = await until(async () => (await fs.readdir(downloads)).sort(),
      names => names.length === 2, "A dropped download left a file behind");
    assert.deepEqual(afterBroken, saved, "A dropped download left a file behind");
    const brokenItem = await broken;
    await until(async () => brokenItem.getState(), value => value === "cancelled", "A dropped download was left resumable");
    report.droppedDownload = "said it failed, left nothing";

    // ⌘F in the page opens the find field; typing reports a count.
    await chord(second, "f");
    await until(state, value => value.find !== null && value.find.focused, "⌘F in the page opened no find field");
    await call("window.browserChromeFindType('needle')");
    const found = await until(state, value => value.find?.status === "1 of 3", "Find reported no count");
    await call("window.browserChromeKey('find', 'Enter')");
    await until(state, value => value.find?.status === "2 of 3", "Enter did not step to the next match");
    await call("window.browserChromeKey('find', 'Escape')");
    await until(state, value => value.find === null, "Escape did not close the find field");
    report.find = found.find.status;

    // Zoom, from the page and from the address bar, changes the page's factor.
    await chord(second, "=");
    await until(async () => second.getZoomFactor(), factor => Math.abs(factor - 1.1) < 0.001, "⌘= did not zoom the page");
    await chord(second, "0");
    await until(async () => second.getZoomFactor(), factor => Math.abs(factor - 1) < 0.001, "⌘0 did not reset the zoom");
    await call("window.browserChromeKey('address', '-', { meta: true })");
    await until(async () => second.getZoomFactor(), factor => Math.abs(factor - 0.9) < 0.001,
      "⌘− in the address bar did not zoom the page");
    await call("window.browserChromeKey('address', '0', { meta: true })");
    await until(async () => second.getZoomFactor(), factor => Math.abs(factor - 1) < 0.001, "⌘0 in the address bar did not reset");
    // Chromium keeps a site's zoom: another site opens at 100%, the first comes back zoomed.
    await chord(second, "=");
    await until(async () => second.getZoomFactor(), factor => Math.abs(factor - 1.1) < 0.001, "⌘= did not zoom the page");
    await second.loadURL(`http://localhost:${port}/second`);
    await loaded(second, "/second");
    const otherSite = second.getZoomFactor();
    await second.loadURL(`http://127.0.0.1:${port}/second`);
    await loaded(second, "/second");
    const sameSite = second.getZoomFactor();
    assert.ok(Math.abs(otherSite - 1) < 0.001 && Math.abs(sameSite - 1.1) < 0.001,
      `The zoom was not kept per site: ${JSON.stringify({ otherSite, sameSite })}`);
    await chord(second, "0");
    report.zoom = { otherSite, sameSite };
    // ⌘L in the page puts the keyboard in the address bar.
    await chord(second, "l");
    await until(state, value => value.addressFocused, "⌘L did not focus the address bar");

    // Reported, not asserted: a key pressed while a page has focus never
    // reaches the app's renderer (the command palette's ⌘⇧P, for one).
    await js("window.__laneKeys = []; window.addEventListener('keydown', event => window.__laneKeys.push(event.key), true); true");
    second.focus();
    await chord(second, "p", ["meta", "shift"]);
    report.appKeysFromPage = await js("window.__laneKeys.length");

    // A camera request reaches the prompt and is refused or granted as answered.
    // Chromium's fake camera only (renderer-fixture-main.cjs sets the switch):
    // a granted request never opens a real one.
    assert.ok(app.commandLine.hasSwitch("use-fake-device-for-media-stream"), "The lane would reach a real camera");
    const camera = `navigator.mediaDevices.getUserMedia({ video: true }).then(stream => {
      const label = stream.getVideoTracks()[0]?.label ?? ""; stream.getTracks().forEach(track => track.stop());
      return "granted:" + label; }, error => error.name)`;
    answer = false;
    assert.equal(await inPage(second, camera), "NotAllowedError", "A refused camera request was not refused");
    assert.deepEqual(questions, [`“127.0.0.1:${port}” wants to use your camera`]);
    assert.equal(await inPage(second, camera), "NotAllowedError");
    assert.equal(questions.length, 1, "A refused site was asked again");
    // Another site, granted: the request goes on to look for the (absent) device.
    await second.loadURL(`http://localhost:${port}/second`);
    await loaded(second, "/second");
    answer = true;
    const granted = await inPage(second, camera);
    assert.match(granted, /^granted:fake/u, "A granted camera request was refused, or reached a real camera");
    assert.equal(questions.at(-1), `“localhost:${port}” wants to use your camera`);
    assert.equal(await inPage(second, "navigator.permissions.query({ name: 'camera' }).then(status => status.state)"),
      "granted", "The permission check disagrees with the grant");
    assert.equal(await inPage(second, "navigator.permissions.query({ name: 'microphone' }).then(status => status.state)"),
      "denied", "A permission never granted reads as granted");
    report.camera = { refused: "NotAllowedError", granted };

    // A permission outside the list is refused without a prompt.
    const asked = questions.length;
    assert.equal(await inPage(second, `new Promise(resolve => navigator.geolocation.getCurrentPosition(
      () => resolve("allowed"), error => resolve(error.code)))`), 1, "Geolocation was not refused");
    assert.equal(await inPage(second, "navigator.requestMIDIAccess().then(() => 'allowed', error => error.name)"),
      "NotAllowedError", "MIDI was not refused");
    assert.equal(questions.length, asked, "A refused permission asked the owner");

    // file: and javascript: stay refused, as a navigation and as a new tab or window.
    const tabsBefore = (await state()).tabs.length;
    await inPage(second, "location.href = 'file:///etc/hosts'; true");
    await inPage(second, "window.open('file:///etc/hosts'); window.open('javascript:document.title=1'); true");
    await settle(400);
    assert.ok(second.getURL().endsWith("/second"), `A file: navigation left the page: ${second.getURL()}`);
    assert.equal((await state()).tabs.length, tabsBefore, "A file: or javascript: window opened a tab");
    assert.equal(BrowserWindow.getAllWindows().length, 1, "A file: or javascript: window opened a window");
    // Another app's link asks with its scheme, and opens nothing when refused.
    answer = false;
    await inPage(second, "location.href = 'mailto:someone@example.com'; true");
    await until(async () => questions.at(-1), message => message?.includes("“mailto:”"), "mailto: did not ask");
    assert.deepEqual(opened, [], "A refused app link was opened");
    report.refused = ["file:", "javascript:", "geolocation", "midi"];

    // A sign-in popup: a hidden child window in the same session, pinned like a page, keeping window.opener.
    await press(second);
    await inPage(second, "window.signin = window.open('/popup', 'signin', 'width=420,height=520'); true");
    const popup = await until(async () => BrowserWindow.getAllWindows().find(entry => entry !== window) ?? null,
      entry => entry !== null, "The popup did not open");
    await loaded(popup.webContents, "/popup");
    const preferences = popup.webContents.getLastWebPreferences();
    assert.equal(popup.isVisible(), false, "The lane's popup was shown");
    assert.equal(popup.getParentWindow(), window, "The popup is not the app window's child");
    assert.equal(popup.webContents.session, pages, "The popup left the Browser session");
    assert.deepEqual({ sandbox: preferences.sandbox, contextIsolation: preferences.contextIsolation,
      nodeIntegration: preferences.nodeIntegration, webviewTag: preferences.webviewTag,
      preload: preferences.preload ?? null }, { sandbox: true, contextIsolation: true, nodeIntegration: false,
      webviewTag: false, preload: null }, "The popup is not pinned like a page");
    assert.deepEqual(await inPage(popup.webContents, "[window.opener !== null, typeof require, typeof window.iface]"),
      [true, "undefined", "undefined"], "The popup lost its opener or reached Node or the app");
    // Its own navigations and new tabs follow the same guards.
    await inPage(popup.webContents, "location.href = 'file:///etc/hosts'; true");
    await settle(300);
    assert.ok(popup.webContents.getURL().endsWith("/popup"), "The popup navigated to file:");
    const beforePopupTab = (await state()).tabs.length;
    const out = await at(popup.webContents, "out");
    await takesInput(popup.webContents, out);
    click(popup.webContents, out);
    await until(state, value => value.tabs.length === beforePopupTab + 1, "The popup's new tab did not open in the pane");
    // Its title leads with the site it is on, whatever the page calls itself.
    await inPage(popup.webContents, "document.title = 'Interface — sign in again'; true");
    await until(async () => popup.getTitle(), title => title === `localhost:${port} — Interface — sign in again`,
      "The popup's title does not lead with its site");
    // One press opens one window: a second window.open on it opens nothing.
    // And a tab holds two popups at most: a third is refused.
    await press(second);
    await inPage(second, "window.two = window.open('/popup', 'two', 'width=300,height=300'); window.extra = window.open('/popup', 'extra', 'width=300,height=300'); [window.two !== null, window.extra === null]");
    await until(async () => windowsBesides().length, count => count === 2, "The second popup did not open");
    await press(second);
    assert.equal(await inPage(second, "window.open('/popup', 'third', 'width=300,height=300') === null"), true,
      "A third popup was not refused");
    await settle(400);
    assert.equal(windowsBesides().length, 2, "A tab holds more than two popups");
    await inPage(second, "window.two.close(); true");
    await until(async () => windowsBesides().length, count => count === 1, "The second popup did not close");
    report.popupLimits = { title: popup.getTitle(), onePerPress: true, third: "refused" };

    // The page closes it.
    await inPage(second, "window.signin.close(); true");
    await until(async () => popup.isDestroyed(), closed => closed, "The page could not close its popup");
    report.popup = "hidden child, same session, pinned, opener kept, closed by its page";

    // The right-click menu on a link: Chrome's items; Open Link in New Tab opens one behind.
    await call(`window.browserChromeSelect(${JSON.stringify(secondTab)})`);
    await until(state, value => value.shownPage === secondId, "The second tab did not come back");
    await second.loadURL(`http://localhost:${port}/second`);
    await loaded(second, "/second");
    const laterAgain = await at(second, "later");
    await takesInput(second, laterAgain);
    click(second, laterAgain, "right");
    const menu = await until(async () => menus.at(-1) ?? null, entry => entry !== null, "No right-click menu");
    const labels = menu.map(entry => entry.type === "separator" ? "—" : entry.label);
    assert.deepEqual(labels.slice(0, 3), ["Open Link in New Tab", "Copy Link Address", "—"], `Menu: ${labels.join(", ")}`);
    assert.equal(labels.at(-1), "Inspect");
    const beforeMenuTab = (await state()).tabs.length;
    menu[0].click();
    const menuTab = await until(state, value => value.tabs.length === beforeMenuTab + 1, "Open Link in New Tab opened nothing");
    assert.ok(menuTab.tabs.find(tab => tab.active)?.id !== undefined);
    report.menu = labels;

    // A PDF shows in the page instead of downloading.
    const files = (await fs.readdir(downloads)).length;
    await second.loadURL(`http://localhost:${port}/doc.pdf`);
    await loaded(second, "/doc.pdf");
    await settle(800);
    assert.equal((await fs.readdir(downloads)).length, files, "The PDF was downloaded instead of shown");
    // Chromium's viewer is an extension frame inside the page.
    const viewer = await until(async () => second.mainFrame.framesInSubtree.map(frame => frame.url),
      urls => urls.some(url => url.startsWith("chrome-extension://")), "The PDF viewer did not load in the page");
    report.pdf = viewer.find(url => url.startsWith("chrome-extension://")).split("/").slice(0, 3).join("/");

    // A page's fullscreen fills the app window, not a new Space, and gives the pane back.
    await second.loadURL(`http://localhost:${port}/second`);
    await loaded(second, "/second");
    const before = await state();
    const shownBefore = before.pages.find(entry => entry.id === secondId);
    const [paneWidth, paneHeight] = await inPage(second, "[innerWidth, innerHeight]");
    assert.ok(paneWidth < before.window.width && paneHeight < before.window.height, "The page already filled the window");
    await inPage(second, "document.documentElement.requestFullscreen().then(() => true, error => error.name)");
    const full = await until(state, value => value.pages.find(entry => entry.id === secondId)?.fullscreen === true,
      "The page's fullscreen did not reach its host");
    // What the page itself sees is the window's whole content area. Its host
    // box is not: in the App, `.workspace-surface { contain: layout }` holds
    // it to the work area while Chromium's top layer draws the <webview> over
    // the whole window.
    const pageView = () => inPage(second, "[innerWidth, innerHeight]");
    await until(pageView, ([width, height]) => width === full.window.width && height === full.window.height,
      `The fullscreen page does not fill the ${full.window.width}x${full.window.height} window`);
    assert.equal(window.isFullScreen(), false, "The page's fullscreen made the window fullscreen");
    // Reported, not asserted: what the app window holds fullscreen.
    const fullscreenElement = await js("document.fullscreenElement?.tagName.toLowerCase() ?? null");
    await chord(second, "Escape", []);
    const back = await until(state, value => value.pages.find(entry => entry.id === secondId)?.fullscreen === false,
      "Escape did not leave fullscreen");
    assert.deepEqual(back.pages.find(entry => entry.id === secondId).box, shownBefore.box,
      "Leaving fullscreen did not give the pane back as it was");
    await until(pageView, ([width, height]) => width === paneWidth && height === paneHeight,
      `Leaving fullscreen did not give the page its ${paneWidth}x${paneHeight} pane back`);
    report.fullscreen = { result: "the page's viewport fills the window, Escape restores the pane", fullscreenElement };

    // A flipped card parks its page as a background tab's is parked: timers
    // drop to about one a second, no frames, and both come back with the
    // page where it was scrolled.
    await second.loadURL(`http://localhost:${port}/counter`);
    await loaded(second, "/counter");
    await inPage(second, "window.scrollTo(0, 600); true");
    const rates = async () => {
      const [ticks, frames] = await second.executeJavaScript("[window.ticks, window.frames]");
      await settle(1500);
      const [ticksAfter, framesAfter] = await second.executeJavaScript("[window.ticks, window.frames]");
      return { ticks: Math.round((ticksAfter - ticks) / 1.5), frames: Math.round((framesAfter - frames) / 1.5) };
    };
    const parkedOf = value => value.pages.find(entry => entry.id === secondId)?.parked;
    await settle(300);
    const running = await rates();
    assert.ok(running.ticks >= 30 && running.frames >= 10, `The shown counter page did not run: ${JSON.stringify(running)}`);
    assert.equal(await call("window.browserChromeFlip()"), "cards", "The panes did not flip to cards");
    await until(state, value => parkedOf(value) === true, "The flipped card left its page unparked");
    await settle(500);
    const flipped = await rates();
    assert.ok(flipped.ticks <= 3 && flipped.frames === 0,
      `The page under a flipped card kept running: ${JSON.stringify(flipped)}`);
    assert.equal(await call("window.browserChromeFlip()"), "split", "The panes did not flip back");
    await until(state, value => parkedOf(value) === false, "The page did not come back with its card");
    await settle(300);
    const resumed = await rates();
    assert.ok(resumed.ticks >= 30 && resumed.frames >= 10, `The page did not resume: ${JSON.stringify(resumed)}`);
    assert.equal(await inPage(second, "window.scrollY"), 600, "Flipping the card lost the page's scroll");
    report.flippedCard = { running, flipped, resumed };

    // A tab's sign-in popup closes with the tab that opened it.
    await press(second);
    await inPage(second, "window.open('/popup', 'signin', 'width=420,height=520'); true");
    const orphan = await until(async () => BrowserWindow.getAllWindows().find(entry => entry !== window) ?? null,
      entry => entry !== null, "The popup did not open");
    await loaded(orphan.webContents, "/popup");
    await call(`window.browserChromeClose(${JSON.stringify(secondTab)})`);
    await until(state, value => !value.tabs.some(tab => tab.id === secondTab), "The tab did not close");
    await until(async () => orphan.isDestroyed() && BrowserWindow.getAllWindows().length === 1, closed => closed,
      "The popup outlived the tab that opened it");
    report.popupClosesWithTab = true;

    // A download link that opens a new tab (target=_blank, window.open): the
    // new tab never gets a document, yet it says where the file went.
    const beforeDownloadTab = (await state()).tabs.length;
    await call(`window.browserChromeSelect(${JSON.stringify(first.tabs[1].id)})`);
    await until(state, value => value.shownPage === firstId, "The first tab did not come back");
    await press(page);
    await inPage(page, "window.open('/report.txt'); true");
    await until(state, value => value.tabs.length === beforeDownloadTab + 1, "The download link opened no tab");
    await until(async () => (await fs.readdir(downloads)).sort(), names => names.includes("report (2).txt"),
      "The download from a new tab did not land");
    await until(state, value => value.hints.includes("Saved report (2).txt to Downloads"),
      "A download that opened in a new tab never said where it went");
    report.downloadInNewTab = "says where it went";
    return report;
  } finally {
    server.close();
    await js("window.clearBrowserChromeFixture()").catch(() => undefined);
    for (const entry of BrowserWindow.getAllWindows()) if (entry !== window) entry.destroy();
    await fs.rm(downloads, { recursive: true, force: true }).catch(() => undefined);
    void output;
  }
};
