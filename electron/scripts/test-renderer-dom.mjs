import { spawn } from "node:child_process";
import { mkdtemp, readdir, rm, stat } from "node:fs/promises";
import { availableParallelism, tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer } from "vite";
import electron from "electron";
import lanes from "./renderer-lanes.cjs";

// Real sandboxed preload/MessagePort; no host, provider, microphone, live profile,
// or new test dependency. Compile main first with npm run test:renderer.
//
// Lanes run in a bounded pool: each is its own Electron process with its own
// profile and deadline (scripts/renderer-lanes.cjs). INTERFACE_RENDERER_LANES_SERIAL=1
// runs them one at a time in their listed order, streaming their output;
// INTERFACE_RENDERER_POOL=<n> sets the pool size. The first failure stops the
// run and prints that lane's output.
const { LANES, selectLanes, laneRank } = lanes;
const root = fileURLToPath(new URL("..", import.meta.url));
const selected = selectLanes(process.env);
const serial = process.env.INTERFACE_RENDERER_LANES_SERIAL === "1";
// Three leaves room for the owner's apps on a 16-core machine; each lane also
// spawns a GPU and a renderer process. The gate passes five when the Electron
// half runs alone, and four beside the Rust half.
const requested = Number.parseInt(process.env.INTERFACE_RENDERER_POOL ?? "", 10);
const poolSize = serial ? 1 : Math.max(1, Math.min(requested > 0 ? requested : 3, availableParallelism()));
// The process deadline is the lane's own; this is the backstop for a lane that
// cannot even run its timer (a blocked main thread, a wedged GPU process).
const KILL_GRACE_MS = 15_000;
const VITE_START_MS = 60_000;
const KEEP_RUNS = 5;
const PRUNE_AFTER_MS = 60 * 60 * 1000;

await pruneEarlierRuns();
const output = await mkdtemp(join(tmpdir(), "interface-renderer-fixtures-"));
const server = await bounded(createServer({ root, server: { host: "127.0.0.1", port: 0 }, logLevel: "error" }));
let passed = false;
try {
  await bounded(server.listen());
  const address = server.httpServer.address();
  const url = `http://127.0.0.1:${address.port}/tests/renderer/index.html`;
  passed = await runLanes(url);
} finally { await server.close(); }
if (passed) {
  // The Chromium profiles have no diagnostic value after a pass; the
  // screenshots and reports stay. A failed run keeps everything.
  for (const name of selected) await rm(join(output, `profile-${name}`), { recursive: true, force: true });
}
process.exitCode = passed ? 0 : 1;

async function runLanes(url) {
  const started = performance.now();
  const pending = [...selected];
  const done = new Set();
  const running = new Map();
  const times = [];
  let failure = null;
  if (!serial) console.log(`Renderer DOM: ${selected.length} lanes, ${poolSize} at a time. Artifacts: ${output}`);
  while (failure === null && (pending.length > 0 || running.size > 0)) {
    while (running.size < poolSize) {
      const ready = pending.filter(name => (LANES[name].after ?? []).every(dependency => done.has(dependency)));
      if (ready.length === 0) break;
      // Serial keeps the listed order; the pool starts the longest chains first.
      const next = serial ? ready[0] : ready.reduce((best, name) => laneRank(name) > laneRank(best) ? name : best);
      pending.splice(pending.indexOf(next), 1);
      running.set(next, runLane(next, url));
    }
    if (running.size === 0) throw new Error(`Renderer lanes cannot start: ${pending.join(", ")}`);
    const result = await Promise.race(running.values());
    running.delete(result.name);
    times.push(`${result.name} ${result.seconds.toFixed(1)} s`);
    if (result.ok) {
      done.add(result.name);
      if (!serial) process.stdout.write(`${result.log}renderer lane ${result.name}: passed in ${result.seconds.toFixed(1)} s\n`);
    } else failure = result;
  }
  if (failure !== null) {
    const stopped = [...running.keys()];
    await Promise.all([...running.values()].map(lane => lane.stop()));
    if (!serial) process.stderr.write(failure.log);
    console.error(`renderer lane ${failure.name}: ${failure.reason} after ${failure.seconds.toFixed(1)} s`
      + (stopped.length ? `; stopped ${stopped.join(", ")}` : "") + `. Artifacts: ${output}`);
    return false;
  }
  console.log(`Renderer DOM: ${selected.length} lanes passed in ${((performance.now() - started) / 1000).toFixed(1)} s`
    + ` (${poolSize} at a time). Lane times: ${times.join(", ")}. Artifacts: ${output}`);
  return true;
}

/** One lane process. Resolves once it exits; `stop()` ends it early. */
function runLane(name, url) {
  const started = performance.now();
  const child = spawn(electron, [join(root, "scripts/renderer-fixture-main.cjs"), url, output, name],
    { stdio: serial ? "inherit" : ["ignore", "pipe", "pipe"] });
  const chunks = [];
  child.stdout?.on("data", chunk => chunks.push(chunk));
  child.stderr?.on("data", chunk => chunks.push(chunk));
  let reason = null;
  const kill = setTimeout(() => {
    reason = `killed: no exit within its ${LANES[name].seconds} s deadline`;
    child.kill("SIGKILL");
  }, LANES[name].seconds * 1000 + KILL_GRACE_MS);
  const exited = new Promise(resolve => {
    child.once("error", error => resolve({ code: null, error }));
    child.once("exit", (code, signal) => resolve({ code, signal }));
  });
  const result = exited.then(async ({ code, signal, error }) => {
    clearTimeout(kill);
    // Output pipes close once the helpers exit too; never wait on a straggler.
    if (!serial) await Promise.race([new Promise(resolve => child.once("close", resolve)), delay(2000)]);
    return { name, ok: code === 0 && reason === null, seconds: (performance.now() - started) / 1000,
      log: Buffer.concat(chunks).toString(),
      reason: reason ?? (error ? `could not start: ${error.message}` : signal ? `ended by ${signal}` : `exited with ${code}`) };
  });
  result.stop = async () => {
    reason ??= "stopped";
    child.kill("SIGTERM");
    const escalate = setTimeout(() => child.kill("SIGKILL"), 5000);
    await exited;
    clearTimeout(escalate);
  };
  return result;
}

/** Vite can stall on file reads; fail with the cause instead of hanging. */
function bounded(promise) {
  let timer;
  return Promise.race([promise.finally(() => clearTimeout(timer)), new Promise(() => {
    timer = setTimeout(() => {
      console.error(`The renderer fixture server did not start within ${VITE_START_MS / 1000} s. A checkout inside `
        + "iCloud Drive (~/Desktop/Projects) can stall Vite's file reads; run the gate from a worktree outside it.");
      process.exit(1);
    }, VITE_START_MS);
  })]);
}

/** Each run leaves screenshots and reports in $TMPDIR. Keep the newest few and
 * anything written in the last hour (a gate running elsewhere); remove the rest. */
async function pruneEarlierRuns() {
  const parent = tmpdir();
  const runs = [];
  for (const entry of await readdir(parent).catch(() => [])) {
    if (!entry.startsWith("interface-renderer-fixtures-")) continue;
    const info = await stat(join(parent, entry)).catch(() => null);
    if (info?.isDirectory()) runs.push({ path: join(parent, entry), modified: info.mtimeMs });
  }
  runs.sort((a, b) => b.modified - a.modified);
  for (const run of runs.slice(KEEP_RUNS)) {
    if (Date.now() - run.modified > PRUNE_AFTER_MS) await rm(run.path, { recursive: true, force: true }).catch(() => {});
  }
}

function delay(ms) { return new Promise(resolve => setTimeout(resolve, ms)); }
