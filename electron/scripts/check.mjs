import { spawn } from "node:child_process";
import { CHECK_PARTS, root } from "./check-parts.mjs";

// Recent Electron packages provision the binary on first import. Finish that
// before starting concurrent parts: fake providers intentionally have no Node
// on PATH and run this checkout's Electron, including its path.txt manifest.
// Otherwise the unit part can race the renderer part's initial installation.
await import("electron");

// `npm run check`, the Electron half of the gate: runs CHECK_PARTS side by
// side, and each part prints its output as one block when it finishes. The
// first failure stops the other parts and fails the check with its output.
// INTERFACE_CHECK_SERIAL=1 runs the parts one after another instead.
const children = new Set();
let stopping = false;

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => { stopAll(signal); process.exit(signal === "SIGINT" ? 130 : 143); });
}
const started = performance.now();
const results = [];
if (process.env.INTERFACE_CHECK_SERIAL === "1") {
  for (const part of CHECK_PARTS) {
    results.push(await runPart(...part));
    if (!results.at(-1).ok) break;
  }
} else results.push(...await Promise.all(CHECK_PARTS.map(part => runPart(...part))));
const failed = results.find(result => !result.ok && !result.stopped);
if (failed) {
  const stopped = results.filter(result => result.stopped).map(result => result.name);
  console.error(`check: ${failed.name} failed at \`${failed.command}\``
    + (stopped.length ? `; stopped ${stopped.join(", ")}.` : "."));
  process.exitCode = 1;
} else {
  console.log(`check: passed in ${since(started)} s (${results.map(result => `${result.name} ${result.seconds} s`).join(", ")}).`);
}

/** Runs one part's commands in order and prints its output as one block. */
async function runPart(name, commands) {
  const partStarted = performance.now();
  const chunks = [];
  for (const [line, command, args] of commands) {
    if (stopping) return { name, ok: false, stopped: true, command: line, seconds: since(partStarted) };
    const code = await new Promise(resolve => {
      // Its own process group, so a stop also reaches Vite and the Electron lanes.
      const child = spawn(command, args, { cwd: root, stdio: ["ignore", "pipe", "pipe"], detached: true });
      children.add(child);
      child.stdout.on("data", chunk => chunks.push(chunk));
      child.stderr.on("data", chunk => chunks.push(chunk));
      child.once("error", error => { chunks.push(Buffer.from(`${error.message}\n`)); children.delete(child); resolve(1); });
      child.once("close", code => { children.delete(child); resolve(code ?? 1); });
    });
    if (code !== 0) {
      const stopped = stopping;
      stopAll("SIGTERM");
      if (!stopped) process.stderr.write(`\n── ${name}: \`${line}\` failed ──\n${Buffer.concat(chunks)}`);
      return { name, ok: false, stopped, command: line, seconds: since(partStarted) };
    }
  }
  process.stdout.write(`\n── ${name}: passed in ${since(partStarted)} s ──\n${Buffer.concat(chunks)}`);
  return { name, ok: true, seconds: since(partStarted) };
}

function stopAll(signal) {
  stopping = true;
  for (const child of children) {
    try { process.kill(-child.pid, signal); } catch { /* already exited */ }
  }
}

function since(from) { return ((performance.now() - from) / 1000).toFixed(1); }
