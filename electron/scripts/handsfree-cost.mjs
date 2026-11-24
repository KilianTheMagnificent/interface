#!/usr/bin/env node
// What the hands-free app costs to draw right now: processor use by process
// kind, the renderer's main-thread share, and which compositor layers were
// drawn again while it watched. Start the app first (npm run handsfree), put
// it in the state to measure, then from electron/:
//
//     node scripts/handsfree-cost.mjs [seconds] [--session name]
//
// The battery figures in docs/THEMES.md and docs/SETTLED_LOOKS.md (looks,
// 2026-11-15) came from this. A hidden window is not a real display: compare
// runs with each other, and repeat one before trusting a difference under two
// points. A layer drawn again on every frame ("repainted 30/s") is what makes
// a moving backdrop expensive; docs/theme-examples/check-theme.mjs names the
// usual causes from the theme file alone.
import { execFileSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const argv = process.argv.slice(2);
const at = argv.indexOf("--session");
const named = at >= 0 ? argv.splice(at, 2)[1] : undefined;
const seconds = Number(argv[0] ?? 10);
if (!Number.isFinite(seconds) || seconds <= 0) {
  console.error("usage: node scripts/handsfree-cost.mjs [seconds] [--session name]");
  process.exit(2);
}
const sessionName = named || process.env.HANDSFREE_SESSION
  || crypto.createHash("sha1").update(process.cwd()).digest("hex").slice(0, 8);
const stateFile = path.join(os.tmpdir(), "handsfree", sessionName, "session.json");
if (!fs.existsSync(stateFile)) {
  console.error(`handsfree-cost: no session at ${stateFile}; run npm run handsfree first`);
  process.exit(1);
}
const session = JSON.parse(fs.readFileSync(stateFile, "utf8"));

/** Every process the session started, by pid. */
function family(root) {
  const all = [root];
  for (let frontier = [root]; frontier.length > 0;) {
    const next = [];
    for (const pid of frontier) {
      let out = "";
      try { out = execFileSync("pgrep", ["-P", String(pid)], { encoding: "utf8" }); } catch { /* no children */ }
      for (const child of out.split("\n").filter(Boolean).map(Number)) { next.push(child); all.push(child); }
    }
    frontier = next;
  }
  return all;
}
function kindOf(pid) {
  let command = "";
  try { command = execFileSync("ps", ["-o", "command=", "-p", String(pid)], { encoding: "utf8" }); } catch { /* gone */ }
  if (/--type=renderer/u.test(command)) return "page";
  if (/--type=gpu-process/u.test(command)) return "graphics";
  if (/--type=/u.test(command)) return "helpers";
  return /Electron/u.test(command) ? "main" : "other";
}

const pages = await (await fetch(`http://127.0.0.1:${session.port}/json/list`)).json();
const target = pages.find(page => page.type === "page" && !page.url.startsWith("devtools://"));
if (target === undefined) { console.error("handsfree-cost: no page to measure"); process.exit(1); }
const socket = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((ok, fail) => { socket.onopen = ok; socket.onerror = () => fail(new Error("CDP connect failed")); });
let id = 0;
const waiting = new Map();
let layers;
socket.onmessage = event => {
  const message = JSON.parse(event.data);
  if (message.method === "LayerTree.layerTreeDidChange" && message.params.layers) layers = message.params.layers;
  const waiter = waiting.get(message.id);
  if (waiter === undefined) return;
  waiting.delete(message.id);
  if (message.error) waiter.fail(new Error(message.error.message)); else waiter.ok(message.result);
};
const send = (method, params = {}) => new Promise((ok, fail) => {
  const n = ++id;
  waiting.set(n, { ok, fail });
  socket.send(JSON.stringify({ id: n, method, params }));
});
// A hidden window never reports focus; without it Interface rests its loops.
await send("Emulation.setFocusEmulationEnabled", { enabled: true });
await send("Performance.enable");
const metrics = async () => Object.fromEntries((await send("Performance.getMetrics")).metrics.map(m => [m.name, m.value]));
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));

// Processor use first, with nothing else watching: reporting the layer tree
// on every frame costs the page and the main process a few points itself.
const pids = family(session.pids[0]);
const kinds = new Map(pids.map(pid => [pid, kindOf(pid)]));
const before = await metrics();
const args = ["-l", "2", "-s", String(seconds), "-stats", "pid,cpu"];
for (const pid of pids) args.push("-pid", String(pid));
const sample = execFileSync("top", args, { encoding: "utf8" }).split(/^PID.*$/mu).pop();
const after = await metrics();

// Then the layers, over a few seconds of their own.
const LAYER_SECONDS = 3;
await send("DOM.enable");
await send("LayerTree.enable");
for (let tries = 0; tries < 30 && layers === undefined; tries += 1) await pause(100);
const paintsBefore = new Map((layers ?? []).map(layer => [layer.layerId, layer.paintCount]));
await pause(LAYER_SECONDS * 1000);

const share = {};
for (const line of sample.split("\n")) {
  const row = /^\s*(\d+)\s+([\d.]+)/u.exec(line);
  if (row === null) continue;
  const kind = kinds.get(Number(row[1])) ?? "other";
  share[kind] = (share[kind] ?? 0) + Number(row[2]);
}
const total = Object.values(share).reduce((sum, value) => sum + value, 0);
const thread = name => (((after[name] - before[name]) / seconds) * 100).toFixed(1);
console.log(`${total.toFixed(1)}% of one core over ${seconds}s`
  + ` (page ${(share.page ?? 0).toFixed(1)}, graphics ${(share.graphics ?? 0).toFixed(1)}, main ${(share.main ?? 0).toFixed(1)},`
  + ` other ${((share.helpers ?? 0) + (share.other ?? 0)).toFixed(1)})`);
console.log(`page main thread: ${thread("TaskDuration")}% (script ${thread("ScriptDuration")}, style ${thread("RecalcStyleDuration")}, layout ${thread("LayoutDuration")})`);

const repainted = [];
for (const layer of layers ?? []) {
  const paints = layer.paintCount - (paintsBefore.get(layer.layerId) ?? layer.paintCount);
  if (paints / LAYER_SECONDS < 1) continue;
  let name = "";
  if (layer.backendNodeId) {
    try {
      const node = (await send("DOM.describeNode", { backendNodeId: layer.backendNodeId })).node;
      const attributes = node.attributes ?? [];
      const value = key => attributes.includes(key) ? attributes[attributes.indexOf(key) + 1] : undefined;
      name = node.localName + (value("class") ? `.${String(value("class")).split(" ").join(".")}` : "")
        + (value("data-layer") ? `[data-layer="${value("data-layer")}"]` : "");
    } catch { /* the node went away */ }
  }
  repainted.push(`  ${String(layer.width).padStart(5)}x${String(layer.height).padEnd(5)} repainted ${Math.round(paints / LAYER_SECONDS)}/s  ${name}`);
}
console.log(`${(layers ?? []).length} compositor layers; ${repainted.length === 0 ? "none drawn again while watching" : `${repainted.length} drawn again:`}`);
for (const row of repainted) console.log(row);
socket.close();
