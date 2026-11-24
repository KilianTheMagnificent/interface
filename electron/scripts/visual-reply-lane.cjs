"use strict";
// The visual-reply renderer lane (T3 section 8): a visual reply's page in a
// real sandboxed frame, served on `iface-visual:` by the app's own frame store
// (dist-main/main/visual-frames.js) behind the app's own navigation guard. The
// page's inline script draws a chart and then probes its fence; this process
// reads the results inside the frame, serves a loopback origin that must never
// hear from it, clicks a link in it, and shoots the card and the overlay in
// dark and light. A second window loads a page under the packaged app's exact
// CSP and frames the same kind of page, proving its inline script runs there.
// The renderer half is src/renderer/fixtures/visual-reply-fixture.tsx.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const http = require("node:http");
const path = require("node:path");
const { pathToFileURL } = require("node:url");
const { BrowserWindow, ipcMain, session } = require("electron");

/** The page under test. Its script draws, then tries every way out and keeps
 * what happened in `window.probe` for the lane to read. */
const PAGE = port => `<!doctype html><html><head><meta charset="utf-8"><style>
:root { --bar: #2f6fde; --fg: #1d1d1f; }
@media (prefers-color-scheme: dark) { :root { --bar: #7aa7ff; --fg: #ececf0; } }
body { margin: 0; padding: 14px 18px; font: 14px -apple-system, sans-serif; color: var(--fg); }
canvas { width: 640px; height: 260px; display: block; }
a { display: inline-block; margin-top: 8px; }
</style></head><body>
<h1 style="font-size:16px;margin:0 0 8px">Signups by week</h1>
<canvas id="chart" width="640" height="260"></canvas>
<a id="away" href="http://127.0.0.1:${port}/navigated">Read the source data</a>
<a id="self" target="_self" href="http://127.0.0.1:${port}/navigated">Open here</a>
<script>
window.probe = { drew: 0, dark: matchMedia("(prefers-color-scheme: dark)").matches, origin: String(origin) };
const chart = document.getElementById("chart").getContext("2d");
const bar = getComputedStyle(document.documentElement).getPropertyValue("--bar").trim();
[12, 19, 15, 27, 31, 24, 36].forEach((value, index) => {
  chart.fillStyle = bar;
  chart.fillRect(index * 90 + 12, 250 - value * 6, 60, value * 6);
  window.probe.drew += 1;
});
try { window.probe.parent = String(parent.document.title); } catch (error) { window.probe.parent = "threw " + error.name; }
try { localStorage.setItem("k", "v"); window.probe.storage = "stored"; } catch (error) { window.probe.storage = "threw " + error.name; }
try { document.cookie = "k=v"; window.probe.cookie = document.cookie; } catch (error) { window.probe.cookie = "threw " + error.name; }
window.probe.fetch = "pending";
fetch("http://127.0.0.1:${port}/leak").then(() => { window.probe.fetch = "reached"; },
  error => { window.probe.fetch = "blocked " + error.name; });
window.probe.clicks = 0;
for (const id of ["away", "self"]) document.getElementById(id).addEventListener("click", () => { window.probe.clicks += 1; });
</script></body></html>`;

async function serve() {
  const heard = [];
  const server = http.createServer((request, response) => {
    if (request.url === "/packaged") {
      // The packaged window gets the app's exact production policy.
      response.setHeader("content-security-policy", server.packagedCsp);
      response.setHeader("content-type", "text/html; charset=utf-8");
      response.end('<!doctype html><html><head><meta charset="utf-8"></head><body><script src="/packaged.js"></script></body></html>');
      return;
    }
    if (request.url === "/packaged.js") {
      response.setHeader("content-type", "text/javascript");
      response.end(`const frame = document.createElement("iframe");
frame.setAttribute("sandbox", "allow-scripts"); frame.src = ${JSON.stringify(server.packagedFrame)};
frame.style.cssText = "width:700px;height:420px;border:0"; document.body.append(frame);`);
      return;
    }
    heard.push(request.url);
    response.end("heard");
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  return { server, heard, port: server.address().port };
}

exports.exerciseVisualReply = async (window, output) => {
  const main = file => import(pathToFileURL(path.join(__dirname, "../dist-main", file)).href);
  const { VisualFrames, installVisualFrameProtocol, rendererNavigationGuard } = await main("main/visual-frames.js");
  const { parseVisualFrameRequest, VISUAL_FRAME_CHANNEL, wrapVisualDocument } = await main("shared/visual-frame.js");
  const { contentSecurityPolicy } = await main("main/window-security.js");
  const frames = new VisualFrames();
  installVisualFrameProtocol(session.defaultSession.protocol, frames);
  ipcMain.handle(VISUAL_FRAME_CHANNEL, (_event, payload) => {
    const request = parseVisualFrameRequest(payload);
    if (request === null) return null;
    if (request.kind === "register") return frames.register(request.html);
    frames.release(request.url);
    return null;
  });
  // The window's own guard, as main installs it, with every decision kept.
  const fixtureOrigin = new URL(window.webContents.getURL()).origin;
  const attempts = [];
  const guard = rendererNavigationGuard(url => url.startsWith(fixtureOrigin));
  const watched = event => {
    let prevented = false;
    guard({ url: event.url, isMainFrame: event.isMainFrame, frame: event.frame,
      preventDefault: () => { prevented = true; event.preventDefault(); } });
    attempts.push({ url: event.url, prevented });
  };
  for (const name of ["will-navigate", "will-frame-navigate", "will-redirect"]) window.webContents.on(name, watched);

  const { server, heard, port } = await serve();
  const js = code => window.webContents.executeJavaScript(code);
  const call = code => js(`Promise.resolve(${code}).catch(error => { throw new Error(error?.stack ?? String(error)); })`);
  const deadline = Date.now() + 30_000;
  const until = async (read, ready, message) => {
    for (;;) {
      const value = await read();
      if (ready(value)) return value;
      assert.ok(Date.now() < deadline, `${message}: ${JSON.stringify(value)}`);
      await new Promise(resolve => setTimeout(resolve, 25));
    }
  };
  const visualFrame = () => window.webContents.mainFrame.framesInSubtree
    .filter(frame => frame.url.startsWith("iface-visual://frame/"));
  const inFrame = (frame, code) => frame.executeJavaScript(code);
  const shot = async name => {
    window.webContents.invalidate();
    await fs.writeFile(path.join(output, `${name}.png`), (await window.webContents.capturePage()).toPNG());
  };
  const report = { themes: {} };
  try {
    window.setContentSize(1100, 760);
    for (const theme of ["dark", "light"]) {
      const state = await call(`window.visualReplyFixture(${JSON.stringify(theme)}, ${JSON.stringify(PAGE(port))})`);
      assert.equal(state.frames.length, 1, "The card drew no frame or more than one");
      const [drawn] = state.frames;
      assert.equal(drawn.sandbox, "allow-scripts", "The frame is not sandboxed to scripts alone");
      assert.equal(drawn.srcdoc, false, "The frame used srcdoc");
      assert.match(drawn.src, /^iface-visual:\/\/frame\/[0-9a-f]{32}$/u, "The frame is not on the visual scheme");
      const [frame] = await until(visualFrame, found => found.length === 1, "The visual frame did not load");
      const probe = await until(() => inFrame(frame, "window.probe ?? null"),
        value => value !== null && value.fetch !== "pending", "The page's script did not finish");
      // It drew: seven bars, and the canvas holds coloured pixels.
      assert.equal(probe.drew, 7, "The page's inline script did not draw");
      const painted = await inFrame(frame, `(() => { const data = document.getElementById("chart").getContext("2d")
        .getImageData(0, 0, 640, 260).data; let n = 0; for (let i = 3; i < data.length; i += 4) if (data[i] > 0) n += 1; return n; })()`);
      assert.ok(painted > 10_000, `The chart painted only ${painted} pixels`);
      // The fence: opaque origin, no parent, no storage, no network.
      assert.equal(probe.origin, "null", "The page has an origin of its own");
      assert.match(probe.parent, /^threw /u, "The page read the app's document");
      assert.equal(probe.storage, "threw SecurityError", "The page reached localStorage");
      assert.match(probe.cookie, /^threw |^$/u, "The page kept a cookie");
      assert.match(probe.fetch, /^blocked /u, "The page's fetch was not blocked");
      assert.equal(probe.dark, theme === "dark", "The page does not follow the app's colour scheme");
      assert.equal(await inFrame(frame, "document.documentElement.dataset.theme"), theme, "The wrapper's data-theme is wrong");
      // A real click on a link that targets the frame itself (a plain link
      // opens a popup the sandbox drops; see the packaged window below): the
      // click lands, the guard refuses, nothing moves.
      const link = await inFrame(frame, `(() => { const r = document.getElementById("self").getBoundingClientRect();
        return { x: r.x + r.width / 2, y: r.y + r.height / 2 }; })()`);
      const box = drawn.box;
      const at = { x: Math.round(box.x + link.x), y: Math.round(box.y + link.y) };
      for (const type of ["mouseMoved", "mousePressed", "mouseReleased"]) {
        await window.webContents.debugger.sendCommand("Input.dispatchMouseEvent",
          { type, x: at.x, y: at.y, button: type === "mouseMoved" ? "none" : "left", buttons: type === "mousePressed" ? 1 : 0,
            clickCount: type === "mouseMoved" ? 0 : 1 });
      }
      await until(() => inFrame(frame, "window.probe.clicks"), clicks => clicks === 1, "The link click did not reach the page");
      const refused = await until(() => attempts.filter(attempt => attempt.url.endsWith("/navigated")),
        found => found.length > 0, "The link click never reached the navigation guard");
      assert.ok(refused.every(attempt => attempt.prevented), "The guard let a visual navigate away");
      const still = visualFrame();
      assert.equal(still.length, 1, "The visual frame went away after the click");
      assert.equal(await inFrame(still[0], "window.probe.drew"), 7, "The page was replaced after the click");
      assert.ok(attempts.filter(attempt => attempt.url.startsWith("iface-visual:")).every(attempt => !attempt.prevented),
        "The guard refused the card's own frame");
      await shot(`visual-reply-card-${theme}`);
      // Larger, over most of the window: a second frame of the same page.
      await call("window.visualReplyPress('Open larger')");
      const opened = await until(() => call("window.visualReplyState()"),
        value => value.frames.some(entry => entry.inOverlay), "Open larger showed no frame");
      assert.ok(opened.overlay.width > opened.window.width * 0.85 && opened.overlay.height > opened.window.height * 0.85,
        `The overlay does not cover most of the window: ${JSON.stringify(opened.overlay)}`);
      const overlayFrames = await until(visualFrame, found => found.length === 2, "The overlay frame did not load");
      await until(() => Promise.all(overlayFrames.map(frame => inFrame(frame, "window.probe?.drew ?? 0"))),
        drew => drew.every(count => count === 7), "The overlay's page did not draw");
      await shot(`visual-reply-overlay-${theme}`);
      await call("window.visualReplyPress('Source', true)");
      const source = await call(`document.querySelector(".visual-reply-overlay .visual-reply-source")?.textContent ?? ""`);
      assert.ok(source.includes('<canvas id="chart"'), "Source does not show the page's HTML");
      await call("window.visualReplyPress('Close', true)");
      report.themes[theme] = { drew: probe.drew, painted, fetch: probe.fetch, parent: probe.parent,
        storage: probe.storage, attempts: refused.length };
    }
    assert.deepEqual(heard, [], `The lane's server heard from a visual: ${heard.join(", ")}`);

    // Output preview's Render: source first, then the same sandboxed frame.
    const preview = await call(`window.visualPreviewFixture("light", ${JSON.stringify(PAGE(port))})`);
    assert.equal(preview.frames.length, 1, "Render drew no frame or more than one");
    assert.equal(preview.frames[0].sandbox, "allow-scripts", "The rendered file is not sandboxed to scripts alone");
    assert.match(preview.frames[0].src, /^iface-visual:\/\/frame\/[0-9a-f]{32}$/u, "The rendered file is not on the visual scheme");
    const [rendered] = await until(visualFrame, found => found.length === 1, "The rendered file did not load");
    const renderedProbe = await until(() => inFrame(rendered, "window.probe ?? null"),
      value => value !== null && value.fetch !== "pending", "The rendered file's script did not finish");
    assert.equal(renderedProbe.drew, 7, "The rendered file's script did not draw");
    assert.match(renderedProbe.fetch, /^blocked /u, "The rendered file's fetch was not blocked");
    assert.equal(renderedProbe.storage, "threw SecurityError", "The rendered file reached localStorage");
    await shot("visual-reply-output-preview-light");
    report.preview = { drew: renderedProbe.drew, fetch: renderedProbe.fetch };

    // The packaged app's exact policy: inline script inside the frame still runs.
    server.packagedCsp = contentSecurityPolicy(undefined);
    server.packagedFrame = frames.register(wrapVisualDocument(PAGE(port), "light"));
    const packaged = new BrowserWindow({ show: false, width: 760, height: 480,
      webPreferences: { sandbox: true, contextIsolation: true, nodeIntegration: false, backgroundThrottling: false } });
    try {
      await packaged.loadURL(`http://127.0.0.1:${port}/packaged`);
      const frame = await until(() => packaged.webContents.mainFrame.framesInSubtree
        .filter(candidate => candidate.url === server.packagedFrame), found => found.length === 1,
      "The packaged-policy page did not frame the visual").then(found => found[0]);
      const probe = await until(() => frame.executeJavaScript("window.probe ?? null"),
        value => value !== null && value.fetch !== "pending", "Under the packaged policy the page's script did not run");
      assert.equal(probe.drew, 7, "Under the packaged policy the page's inline script did not draw");
      assert.match(probe.fetch, /^blocked /u, "Under the packaged policy the page's fetch was not blocked");
      // A real click on its link under the app's own policy, whose
      // `frame-src iface-visual:` refuses the navigation before the guard
      // sees it: the page must stay, not turn into a blank error page.
      packaged.webContents.debugger.attach("1.3");
      await packaged.webContents.debugger.sendCommand("Emulation.setFocusEmulationEnabled", { enabled: true });
      packaged.webContents.invalidate();
      await packaged.webContents.capturePage();
      const box = await packaged.webContents.executeJavaScript(`(() => {
        const r = document.querySelector("iframe").getBoundingClientRect(); return { x: r.x, y: r.y }; })()`);
      const link = await frame.executeJavaScript(`(() => { const r = document.getElementById("away").getBoundingClientRect();
        return { x: r.x + r.width / 2, y: r.y + r.height / 2 }; })()`);
      const at = { x: Math.round(box.x + link.x), y: Math.round(box.y + link.y) };
      for (const type of ["mouseMoved", "mousePressed", "mouseReleased"]) {
        await packaged.webContents.debugger.sendCommand("Input.dispatchMouseEvent",
          { type, x: at.x, y: at.y, button: type === "mouseMoved" ? "none" : "left", buttons: type === "mousePressed" ? 1 : 0,
            clickCount: type === "mouseMoved" ? 0 : 1 });
      }
      await until(() => frame.executeJavaScript("window.probe.clicks").catch(() => -1),
        clicks => clicks !== 0, "Under the packaged policy the link click did not reach the page");
      await new Promise(resolve => setTimeout(resolve, 500));
      const stayed = packaged.webContents.mainFrame.framesInSubtree
        .filter(candidate => candidate.url === server.packagedFrame);
      assert.equal(stayed.length, 1, "Under the packaged policy a link click moved the visual's frame");
      assert.equal(await stayed[0].executeJavaScript("window.probe?.drew ?? 0").catch(() => 0), 7,
        "Under the packaged policy a link click blanked the visual");
      assert.equal(BrowserWindow.getAllWindows().length, 2, "A link click in a visual opened a window");
      report.packaged = { csp: server.packagedCsp, drew: probe.drew, fetch: probe.fetch, clickStayed: true };
    } finally {
      packaged.destroy();
    }
    assert.deepEqual(heard, [], `The lane's server heard from a visual: ${heard.join(", ")}`);
    report.heard = heard.length;
    report.webrtc = await exerciseWebRtcFence({ port, frames, server, installVisualFrameProtocol, wrapVisualDocument, main });
    return report;
  } finally {
    server.close();
  }
};

/** WebRTC is outside CSP: a visual's script can send STUN over UDP and TURN
 * over TCP to any address it names unless the window's own network fence
 * (dist-main/main/visual-network-fence.js) is in place. Under the packaged
 * app's exact CSP, with the fence the app window gets, local UDP and TCP
 * listeners on loopback and on this Mac's own LAN address must hear nothing. */
async function exerciseWebRtcFence({ port, frames, server, installVisualFrameProtocol, wrapVisualDocument, main }) {
  const dgram = require("node:dgram");
  const net = require("node:net");
  const os = require("node:os");
  const { fenceWindowNetwork } = await main("main/visual-network-fence.js");
  const lan = Object.values(os.networkInterfaces()).flat()
    .find(entry => entry?.family === "IPv4" && !entry.internal)?.address;
  const hosts = ["127.0.0.1", ...(lan === undefined ? [] : [lan])];
  const heard = [];
  const listeners = [];
  const targets = [];
  for (const host of hosts) {
    const udp = dgram.createSocket("udp4");
    udp.on("message", (message, from) => heard.push(`udp ${host} from ${from.address}:${from.port} ${message.length}B`));
    await new Promise(resolve => udp.bind(0, host, resolve));
    const tcp = net.createServer(socket => { heard.push(`tcp ${host} from ${socket.remoteAddress}`); socket.destroy(); });
    await new Promise(resolve => tcp.listen(0, host, resolve));
    listeners.push(udp, tcp);
    targets.push({ host, udp: udp.address().port, tcp: tcp.address().port });
  }
  const page = `<!doctype html><body><script>
    window.probe = { made: 0 };
    for (const target of ${JSON.stringify(targets)}) {
      for (const config of [
        { iceServers: [{ urls: "stun:" + target.host + ":" + target.udp }] },
        { iceTransportPolicy: "relay", iceServers: [{ urls: "turn:" + target.host + ":" + target.tcp + "?transport=tcp",
          username: "visual", credential: "visual" }] },
      ]) {
        const connection = new RTCPeerConnection(config);
        connection.createDataChannel("x");
        connection.createOffer().then(offer => connection.setLocalDescription(offer));
        window.probe.made += 1;
      }
    }
  </script></body>`;
  const fenced = session.fromPartition("visual-reply-lane-fence");
  installVisualFrameProtocol(fenced.protocol, frames);
  server.packagedFrame = frames.register(wrapVisualDocument(page, "light"));
  const window = new BrowserWindow({ show: false, width: 400, height: 300,
    webPreferences: { session: fenced, sandbox: true, contextIsolation: true, nodeIntegration: false } });
  let made = 0;
  try {
    // As main.ts fences the app window; the lane's own server stands in for
    // the dev server, the one origin the fence lets through.
    await fenceWindowNetwork(window.webContents, fenced, `http://127.0.0.1:${port}`);
    await window.loadURL(`http://127.0.0.1:${port}/packaged`);
    const deadline = Date.now() + 10_000;
    while (made < targets.length * 2 && Date.now() < deadline) {
      const frame = window.webContents.mainFrame.framesInSubtree.find(candidate => candidate.url === server.packagedFrame);
      made = frame === undefined ? 0 : await frame.executeJavaScript("window.probe?.made ?? 0").catch(() => 0);
      await new Promise(resolve => setTimeout(resolve, 50));
    }
    assert.equal(made, targets.length * 2, "The WebRTC probe did not run in the visual");
    // ICE gathering sends its first STUN binding within a second.
    await new Promise(resolve => setTimeout(resolve, 4000));
  } finally {
    window.destroy();
    for (const listener of listeners) listener.close();
  }
  assert.deepEqual(heard, [], `A visual reached the network through WebRTC: ${heard.slice(0, 4).join(", ")}`);
  return { hosts, connections: made, heard: heard.length };
}
