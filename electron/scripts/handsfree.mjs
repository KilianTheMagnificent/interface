#!/usr/bin/env node
// handsfree: test an app without touching the owner's mouse, keyboard, window focus or data.
// Web pages run in headless Chrome and Electron windows stay hidden; both are driven over the
// Chrome DevTools Protocol, so input goes to the page, never the OS. Native macOS apps are
// driven in-process through their Handsfree.swift hook. Zero dependencies (Node >= 22).
import { spawn, execFileSync } from "node:child_process";
import { createRequire } from "node:module";
import crypto from "node:crypto";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const USAGE = `usage: handsfree <command> [args] [--session name]
Start (each app gets its own process group, HANDSFREE=1 and a throwaway HANDSFREE_PROFILE dir;
native apps also get CFFIXED_USER_HOME, so Application Support, Caches and Logs are throwaway):
  start chrome <url> [--serve "<cmd>"] [--size 1280x800]
        headless Chrome; --serve starts the app's server first with a free $HANDSFREE_PORT,
        and {port} in <url> expands to it
  start electron <appDir> [--electron bin] [-- app args]   Electron app with the hands-free hook
  start command -- <launcher...>                           launcher whose Electron app has the hook
  start native -- <binary|App.app> [args]                  macOS app with the Handsfree.swift hook
Drive (web/Electron take CSS selectors; native apps take a label substring or #identifier):
  click <selector> | click --text <label>
  type <text> [--into <selector>]
  key <Enter|Tab|Escape|ArrowUp|Backspace|Meta+k|...>
  wait <selector> [--gone] [--timeout ms] | wait --text <substring>
  shot <file.png> [--selector css] [--full]
  text [css]              page text; for native apps, the accessibility tree of the window
  eval <js>               web/Electron only: run in the page, print the JSON result
  action <name> [json]    native only: app-defined seams (fixtures, hotkey-triggered flows)
  targets | logs [--lines n] | focus | stop
Common: --target <substring> picks a window (page url/title, or native window title).
Every command fails with exit 3 if the app under test ever becomes the frontmost app.`;

const argv = process.argv.slice(2);
const passthrough = argv.includes("--") ? argv.splice(argv.indexOf("--")).slice(1) : [];
const flags = {}, pos = [];
for (let i = 0; i < argv.length; i++) {
  if (!argv[i].startsWith("--")) { pos.push(argv[i]); continue; }
  const key = argv[i].slice(2);
  flags[key] = argv[i + 1] === undefined || argv[i + 1].startsWith("--") ? true : argv[++i];
}
const [cmd, ...rest] = pos;
const sessionName = flags.session || process.env.HANDSFREE_SESSION
  || crypto.createHash("sha1").update(process.cwd()).digest("hex").slice(0, 8);
const dir = path.join(os.tmpdir(), "handsfree", sessionName);
const stateFile = path.join(dir, "session.json");
const sleep = ms => new Promise(r => setTimeout(r, ms));
const die = (msg, code = 1) => { console.error(`handsfree: ${msg}`); process.exit(code); };
const state = () => fs.existsSync(stateFile) ? JSON.parse(fs.readFileSync(stateFile, "utf8")) : die(`no session in ${dir}; run start first`);
const alive = pid => { try { process.kill(pid, 0); return true; } catch { return false; } };

// macOS reports the frontmost app without any permission. Everything a session launches
// shares its process groups, and none of it may ever become the frontmost app.
function frontmostGroup() {
  try {
    const asn = execFileSync("lsappinfo", ["front"], { encoding: "utf8" }).trim();
    const pid = /pid = (\d+)/.exec(execFileSync("lsappinfo", ["info", "-only", "pid", asn], { encoding: "utf8" }))?.[1];
    return pid ? Number(execFileSync("ps", ["-o", "pgid=", "-p", pid], { encoding: "utf8" }).trim()) : null;
  } catch { return null; }
}
function assertNotFrontmost(groups) {
  if (groups.includes(frontmostGroup())) die("app under test became the frontmost app (focus stolen)", 3);
}

async function until(what, ms, probe) {
  const deadline = Date.now() + ms;
  for (;;) {
    const value = await probe();
    if (value) return value;
    if (Date.now() > deadline) die(`${what} within ${ms}ms; see ${path.join(dir, "app.log")}`);
    await sleep(200);
  }
}

// Chromium announces its port on stderr and in <profile>/DevToolsActivePort; launchers that
// swallow stderr still leave the file.
const devtoolsPort = (log, profile) => {
  const file = path.join(profile, "DevToolsActivePort");
  const fromFile = fs.existsSync(file) && Number(fs.readFileSync(file, "utf8").split("\n")[0]);
  return fromFile || Number(/DevTools listening on ws:\/\/[^:]+:(\d+)\//.exec(fs.readFileSync(log, "utf8"))?.[1]) || 0;
};

const freePort = () => new Promise(resolve => {
  const server = net.createServer().listen(0, "127.0.0.1", () => { const { port } = server.address(); server.close(() => resolve(port)); });
});

async function start() {
  if (fs.existsSync(stateFile)) die(`session ${sessionName} already running; stop it or pass --session`);
  fs.mkdirSync(dir, { recursive: true });
  const [kind, target] = rest;
  const log = path.join(dir, "app.log"), profile = path.join(dir, "profile");
  const timeout = Number(flags.timeout || 120000);
  const env = { ...process.env, HANDSFREE: "1", HANDSFREE_PROFILE: profile };
  const fd = fs.openSync(log, "a");
  const pids = [];
  const save = extra => fs.writeFileSync(stateFile, JSON.stringify({ kind, pids, log, ...extra }));
  const launch = (bin, args, extraEnv = {}) => {
    const child = spawn(bin, args, { detached: true, stdio: ["ignore", fd, fd], env: { ...env, ...extraEnv } });
    child.unref();
    // A failed spawn has no pid; recording it would make stop signal process group 0 (our own).
    if (!(Number.isInteger(child.pid) && child.pid > 0)) { fs.rmSync(dir, { recursive: true, force: true }); die(`cannot launch ${bin}`); }
    pids.push(child.pid);
    save({});
    return child;
  };
  let port;
  if (kind === "chrome") {
    let url = target || "about:blank";
    if (flags.serve) {
      const appPort = await freePort();
      url = url.replaceAll("{port}", String(appPort));
      launch("/bin/sh", ["-c", flags.serve], { HANDSFREE_PORT: String(appPort) });
      await until("server did not answer", timeout, () => fetch(url).then(() => true, () => false));
    }
    const chromeProfile = path.join(dir, "chrome");
    const [w, h] = String(flags.size || "1280x800").split("x").map(Number);
    launch(process.env.HANDSFREE_CHROME || "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", [
      "--headless=new", `--user-data-dir=${chromeProfile}`, "--remote-debugging-port=0", "--no-first-run",
      "--no-default-browser-check", "--disable-extensions", "--enable-logging=stderr", `--window-size=${w},${h}`,
      // Keep the owner's Chrome install quiet: no updater wake-ups, sync or keychain prompts.
      "--disable-background-networking", "--disable-component-update", "--disable-sync", "--no-service-autorun",
      "--use-mock-keychain", "--password-store=basic",
      // Silent: the owner may be on a call.
      "--mute-audio", url]);
    port = await until("no DevTools port", timeout, () => devtoolsPort(log, chromeProfile));
    // Chrome has no app-side hook, so a small keeper holds a browser-level CDP session that
    // silences speech in every page (headless Chrome can speak aloud) and pins focus.
    launch(process.execPath, [fileURLToPath(import.meta.url), "__keep", String(port)]);
  } else if (kind === "electron" || kind === "command") {
    if (kind === "electron") launch(flags.electron || createRequire(path.join(process.cwd(), "package.json"))("electron"), [path.resolve(target || "."), ...passthrough]);
    else if (passthrough.length) launch(passthrough[0], passthrough.slice(1));
    else die(USAGE);
    port = await until("no DevTools port (does the app call the hands-free hook?)", timeout, () => devtoolsPort(log, profile));
  } else if (kind === "native" && passthrough.length) {
    // Exec the bundle's binary directly: `open` would go through LaunchServices and activate.
    const [app, ...args] = passthrough;
    const plist = path.join(app, "Contents", "Info.plist");
    const exe = () => execFileSync("plutil", ["-extract", "CFBundleExecutable", "raw", plist], { encoding: "utf8" }).trim();
    fs.mkdirSync(path.join(dir, "home"), { recursive: true });
    launch(app.endsWith(".app") ? path.join(app, "Contents", "MacOS", exe()) : app, args, { CFFIXED_USER_HOME: path.join(dir, "home") });
    await until("app never became ready (does it call Handsfree.start()?)", timeout, () => fs.existsSync(path.join(dir, "native-ready")));
  } else die(USAGE);
  save({ port });
  await sleep(500);
  assertNotFrontmost(pids);
  console.log(JSON.stringify({ session: sessionName, kind, port, pids, log }));
}

async function stop() {
  const s = state();
  const pids = s.pids.filter(pid => Number.isInteger(pid) && pid > 0);
  for (const pid of pids) try { process.kill(-pid, "SIGTERM"); } catch {}
  for (let i = 0; i < 50 && pids.some(alive); i++) await sleep(100);
  for (const pid of pids) try { process.kill(-pid, "SIGKILL"); } catch {}
  if (s.kind === "native") {
    // Handsfree.swift points standard preferences at this per-session domain.
    try { execFileSync("defaults", ["delete", `handsfree.${sessionName}`], { stdio: "ignore" }); } catch {}
    fs.rmSync(path.join(os.homedir(), "Library", "Preferences", `handsfree.${sessionName}.plist`), { force: true });
  }
  fs.rmSync(dir, { recursive: true, force: true });
  console.log(`stopped ${sessionName}`);
}

// ---- Chrome DevTools Protocol (web pages and Electron windows) ----

async function pages(port) {
  const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  return list.filter(t => t.type === "page" && !t.url.startsWith("devtools://"));
}

async function connect(s) {
  const all = await pages(s.port);
  const t = flags.target ? all.find(p => p.url.includes(flags.target) || p.title.includes(flags.target)) : all[0];
  if (!t) die(`no page target${flags.target ? ` matching ${flags.target}` : ""}; see targets`);
  const ws = new WebSocket(t.webSocketDebuggerUrl);
  await new Promise((ok, fail) => { ws.onopen = ok; ws.onerror = () => fail(new Error("CDP connect failed")); });
  let id = 0; const waiting = new Map();
  ws.onmessage = e => {
    const msg = JSON.parse(e.data);
    const w = waiting.get(msg.id); if (!w) return;
    waiting.delete(msg.id);
    msg.error ? w.fail(new Error(msg.error.message)) : w.ok(msg.result);
  };
  const send = (method, params = {}, ms = 15000) => new Promise((ok, fail) => {
    const n = ++id; waiting.set(n, { ok, fail });
    setTimeout(() => waiting.has(n) && (waiting.delete(n), fail(new Error(`${method} timed out`))), ms).unref();
    ws.send(JSON.stringify({ id: n, method, params }));
  });
  // Hidden windows never report focus, so focus handlers stay dead. Apps using the hands-free
  // hook pin emulated focus themselves; emulating here too would blur them on every disconnect.
  const focused = await send("Runtime.evaluate", { expression: "document.hasFocus()", returnByValue: true });
  if (!focused.result.value) await send("Emulation.setFocusEmulationEnabled", { enabled: true });
  return { send, close: () => ws.close() };
}

async function evaluate(c, expression) {
  const r = await c.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true, userGesture: true });
  if (r.exceptionDetails) die(`page threw: ${r.exceptionDetails.exception?.description || r.exceptionDetails.text}`);
  return r.result.value;
}

// Finds the element, scrolls it into view, and returns its centre in CSS pixels.
const locate = (css, text) => `(() => {
  const want = ${JSON.stringify(text ?? null)};
  const all = [...document.querySelectorAll(${JSON.stringify(css || "button,a,[role],input,textarea,select,label,summary,[tabindex]")})];
  const label = e => (e.innerText || e.value || e.getAttribute("aria-label") || e.title || "").trim();
  const matches = want === null ? [] : all.filter(e => label(e).includes(want));
  const exact = matches.filter(e => label(e) === want);
  const candidates = exact.length ? exact : matches;
  // A role-bearing app/group also contains its buttons' text. Prefer the exact
  // label and then its innermost match, rather than clicking that ancestor.
  const el = want === null ? all[0] : candidates.find(e => !candidates.some(child => child !== e && e.contains(child)));
  if (!el) return null;
  el.scrollIntoView({ behavior: "instant", block: "center", inline: "center" });
  const r = el.getBoundingClientRect();
  return { x: r.left + r.width / 2, y: r.top + r.height / 2, w: r.width, h: r.height };
})()`;

const KEYS = { Enter: [13, "\r"], Tab: [9, "\t"], Escape: [27], Backspace: [8], Delete: [46], Space: [32, " "],
  ArrowUp: [38], ArrowDown: [40], ArrowLeft: [37], ArrowRight: [39], Home: [36], End: [35], PageUp: [33], PageDown: [34] };
const MODS = { Alt: 1, Control: 2, Ctrl: 2, Meta: 4, Cmd: 4, Shift: 8 };

async function drivePage(s) {
  if (cmd === "targets") return console.log(JSON.stringify((await pages(s.port)).map(p => ({ title: p.title, url: p.url })), null, 2));
  if (cmd === "action") die("action is for native apps; use eval");
  const c = await connect(s);
  try {
    if (cmd === "eval") console.log(JSON.stringify(await evaluate(c, rest.join(" ")), null, 2));
    else if (cmd === "text") console.log(await evaluate(c, `(document.querySelector(${JSON.stringify(rest[0] || "body")})?.innerText ?? "")`));
    else if (cmd === "click") {
      const at = await evaluate(c, locate(rest[0], flags.text === true ? "" : flags.text));
      if (!at) die(`nothing matches ${rest[0] || ""} ${flags.text || ""}`.trim());
      if (!at.w || !at.h) die("element has no size (hidden?)");
      for (const type of ["mouseMoved", "mousePressed", "mouseReleased"])
        await c.send("Input.dispatchMouseEvent", { type, x: at.x, y: at.y, button: "left", clickCount: type === "mouseMoved" ? 0 : 1 });
      console.log(`clicked at ${Math.round(at.x)},${Math.round(at.y)}`);
    } else if (cmd === "type") {
      if (flags.into) await evaluate(c, `document.querySelector(${JSON.stringify(flags.into)})?.focus()`);
      await c.send("Input.insertText", { text: rest.join(" ") });
      console.log("typed");
    } else if (cmd === "key") {
      const parts = rest[0].split("+"), name = parts.pop();
      const modifiers = parts.reduce((m, p) => m | (MODS[p] ?? die(`unknown modifier ${p}`)), 0);
      const [code, text] = KEYS[name] || [name.toUpperCase().charCodeAt(0), name.length === 1 && !(modifiers & 6) ? name : undefined];
      const base = { key: name, windowsVirtualKeyCode: code, modifiers };
      await c.send("Input.dispatchKeyEvent", { type: text ? "keyDown" : "rawKeyDown", text, ...base });
      await c.send("Input.dispatchKeyEvent", { type: "keyUp", ...base });
      console.log(`pressed ${rest[0]}`);
    } else if (cmd === "wait") {
      const probe = flags.text
        ? `document.body.innerText.includes(${JSON.stringify(flags.text)})`
        : `(e => !!e && e.getClientRects().length > 0)(document.querySelector(${JSON.stringify(rest[0])}))`;
      await until("wait timed out", Number(flags.timeout || 10000), async () => Boolean(await evaluate(c, probe)) !== Boolean(flags.gone));
      console.log("ready");
    } else if (cmd === "shot") {
      const file = path.resolve(rest[0] || "handsfree.png");
      let clip;
      if (flags.selector) {
        const at = await evaluate(c, locate(flags.selector));
        if (!at) die(`nothing matches ${flags.selector}`);
        clip = { x: at.x - at.w / 2, y: at.y - at.h / 2, width: at.w, height: at.h, scale: 1 };
      }
      const r = await c.send("Page.captureScreenshot", { format: "png", clip, captureBeyondViewport: !!flags.full }, 30000);
      fs.writeFileSync(file, Buffer.from(r.data, "base64"));
      console.log(file);
    } else die(USAGE);
  } finally { c.close(); }
}

// ---- Native macOS apps (requests served in-process by Handsfree.swift) ----

async function request(cmdName, args, ms = 15000) {
  const id = crypto.randomUUID(), inbox = path.join(dir, "inbox"), reply = path.join(dir, "outbox", `${id}.json`);
  fs.mkdirSync(inbox, { recursive: true });
  fs.writeFileSync(path.join(inbox, `${id}.tmp`), JSON.stringify({ cmd: cmdName, args: { window: flags.target, ...args } }));
  fs.renameSync(path.join(inbox, `${id}.tmp`), path.join(inbox, `${id}.json`));
  await until(`app did not answer ${cmdName}`, ms, () => fs.existsSync(reply));
  const r = JSON.parse(fs.readFileSync(reply, "utf8"));
  fs.rmSync(reply);
  return r.ok ? r.result : die(r.error);
}

async function driveNative() {
  const query = flags.text === true ? "" : flags.text ?? rest[0];
  if (cmd === "text") console.log(await request("tree", {}));
  else if (cmd === "targets") console.log((await request("tree", {})).split("\n").filter(l => l.startsWith("window ")).join("\n"));
  else if (cmd === "click") console.log(await request("press", { query }));
  else if (cmd === "type") console.log(await request("type", { text: rest.join(" "), query: flags.into }));
  else if (cmd === "key") console.log(await request("key", { key: rest[0] }));
  else if (cmd === "shot") console.log(await request("shot", { path: path.resolve(rest[0] || "handsfree.png") }, 30000));
  else if (cmd === "action") console.log(JSON.stringify(await request("action", { name: rest[0], payload: JSON.parse(rest[1] || "{}") }, 60000), null, 2));
  else if (cmd === "wait") {
    await until("wait timed out", Number(flags.timeout || 10000),
      async () => (await request("tree", {})).includes(query) !== Boolean(flags.gone));
    console.log("ready");
  } else if (cmd === "eval") die("eval is for web/Electron; native apps expose seams through action");
  else die(USAGE);
}

const SPEECH_GUARD = `try { if (globalThis.speechSynthesis) speechSynthesis.speak = () => console.log("handsfree: blocked speech"); } catch {}`;

async function keep(port) {
  const { webSocketDebuggerUrl } = await (await fetch(`http://127.0.0.1:${port}/json/version`)).json();
  const ws = new WebSocket(webSocketDebuggerUrl);
  await new Promise(ok => { ws.onopen = ok; });
  let id = 0;
  const send = (method, params = {}, sessionId) => ws.send(JSON.stringify({ id: ++id, method, params, sessionId }));
  ws.onmessage = e => {
    const { method, params } = JSON.parse(e.data);
    if (method === "Target.targetCreated" && params.targetInfo.type === "page") send("Target.attachToTarget", { targetId: params.targetInfo.targetId, flatten: true });
    if (method === "Target.attachedToTarget") {
      const session = params.sessionId;
      send("Page.enable", {}, session);
      send("Page.addScriptToEvaluateOnNewDocument", { source: SPEECH_GUARD, runImmediately: true }, session);
      send("Runtime.evaluate", { expression: SPEECH_GUARD }, session);
      send("Emulation.setFocusEmulationEnabled", { enabled: true }, session);
    }
  };
  ws.onclose = () => process.exit(0);
  send("Target.setDiscoverTargets", { discover: true });
}

async function run() {
  if (cmd === "__keep") return keep(rest[0]);
  if (!cmd || flags.help) return console.log(USAGE);
  if (cmd === "start") return start();
  if (cmd === "stop") return stop();
  const s = state();
  if (cmd === "focus") { assertNotFrontmost(s.pids); return console.log("ok: app under test is not frontmost"); }
  if (cmd === "logs") return console.log(fs.readFileSync(s.log, "utf8").split("\n").slice(-Number(flags.lines || 80)).join("\n"));
  await (s.kind === "native" ? driveNative() : drivePage(s));
  assertNotFrontmost(s.pids);
}

run().catch(e => die(e.message));
