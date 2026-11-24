import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import fs, { existsSync, mkdirSync, readFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { setTimeout as wait } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { cargoTargetDirectory } from "./cargo-target.mjs";
import {
  DEV_APP_DISPLAY_NAME, DEV_APP_INSTALL_PATH, electronExecutable, installDevApp,
  installedDevAppMatches, isNewerMainBuild, materializeDevApp,
} from "./dev-app.mjs";
import {
  devPortIsBusy, resolveRustTool, voiceBuildReason, voiceSourceStamp,
} from "./dev-support.mjs";

const electronRoot = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const workspaceRoot = path.join(electronRoot, "..");
const devHost = "127.0.0.1";
const devPort = 5199;
const devUrl = `http://${devHost}:${String(devPort)}`;
const runId = `interface-rust-dev-${String(process.pid)}-${randomUUID()}`;
const tool = name => path.join(electronRoot, "node_modules", ".bin", name);
// One directory holds both the Electron profile and the Rust host stores, the
// same shape production uses. It cannot nest the host root deeper, and the name
// has to stay short: the collaboration broker binds
// `<root>/collaboration/sockets/<16 hex>.sock`, and macOS rejects an AF_UNIX
// path over 103 bytes. "Iface Dev" leaves the same headroom production has.
const devProfile = process.env.INTERFACE_DEV_PROFILE
  ?? path.join(os.homedir(), "Library", "Application Support", "Iface Dev");
const devHostDataRoot = devProfile;
const maxSocketPathBytes = 103;
const socketPathBytes = Buffer.byteLength(
  path.join(devHostDataRoot, "collaboration", "sockets", `${"0".repeat(16)}.sock`),
);
const children = [];
const watchers = [];
let watchedHostBinary = null;
let restartTimer = null;
let shuttingDown = false;

function stopWatchers() {
  if (restartTimer !== null) {
    clearTimeout(restartTimer);
    restartTimer = null;
  }
  while (watchers.length > 0) {
    const watcher = watchers.pop();
    try { watcher.close(); } catch { /* the watcher is already closed */ }
  }
  if (watchedHostBinary !== null) {
    fs.unwatchFile(watchedHostBinary);
    watchedHostBinary = null;
  }
}

function stopChildren() {
  stopWatchers();
  for (const child of children) {
    if (child.exitCode === null && child.signalCode === null) child.kill();
  }
}

function abort(message) {
  if (shuttingDown) return;
  shuttingDown = true;
  console.error(message);
  stopChildren();
  process.exit(1);
}

function handleSignal(signal) {
  if (shuttingDown) return;
  shuttingDown = true;
  stopChildren();
  process.exit(128 + (os.constants.signals[signal] ?? 0));
}

for (const signal of ["SIGHUP", "SIGINT", "SIGTERM"]) {
  process.once(signal, () => { handleSignal(signal); });
}
process.once("exit", stopChildren);

function run(command, args, options = {}) {
  if (path.isAbsolute(command) && !existsSync(command)) {
    abort(`dev.mjs: ${command} is missing; install the workspace dependencies first`);
  }
  const child = spawn(command, args, {
    cwd: options.cwd ?? electronRoot,
    stdio: "inherit",
    env: { ...process.env, ...(options.env ?? {}) },
  });
  children.push(child);
  return child;
}

function supervise(child, label) {
  child.once("error", error => {
    abort(`dev.mjs: failed to start ${label}: ${error.message}`);
  });
  child.once("exit", (code, signal) => {
    if (shuttingDown) return;
    const outcome = signal === null
      ? `exit code ${String(code)}` : `signal ${String(signal)}`;
    abort(`dev.mjs: ${label} exited unexpectedly (${outcome})`);
  });
  return child;
}

async function runChecked(command, args, label, cwd = electronRoot) {
  const child = run(command, args, { cwd });
  await new Promise((resolve, reject) => {
    child.once("error", error => reject(
      new Error(`failed to start ${label}: ${error.message}`),
    ));
    child.once("exit", (code, signal) => {
      if (code === 0) resolve();
      else reject(new Error(signal === null
        ? `${label} failed with exit code ${String(code)}`
        : `${label} failed from signal ${String(signal)}`));
    });
  });
}

// The host spawns the voice helper from its own directory, so the debug build
// has to land beside `interface-host` — in the directory cargo really builds
// into, which is `<checkout>/target` unless `CARGO_TARGET_DIR` or a user-level
// cargo config moves it, so cargo is asked. Resolved after the port check
// below, so a supervisor that has already lost the port pays no cargo call.
const voicePackagePath = path.join(workspaceRoot, "apps", "interface-voice");
let voiceInstalledPath = null;
let voiceStampPath = null;
let hostBinary = null;

// Why the stamp is here. `buildVoiceHelper()` runs once per supervisor process,
// before Electron starts, and nothing in the watch/restart path further down can
// re-enter it: a new `dist-main` emit or a rebuilt `interface-host` only ever
// restarts the Electron child. The "Building for debugging… Build complete!"
// pair the supervisor printed every 10-30 s for hours on 2026-09-04 was
// therefore not one supervisor looping — it was an external respawner starting
// dev.mjs on a fixed interval. A transient launchd job, `com.interface.dev.codex`
// with KeepAlive set, had run `npm run dev` 4,637 times, every one of them
// exiting 1, while a healthy supervisor held 5199 and its Electron pid never
// moved. Each of those starts paid a cargo no-op and then an unconditional
// `swift build` — 0.1 s of "Build complete!" — before Vite's `--strictPort`
// killed it for losing the port.
//
// Two things come out of that. The port is checked before any build now, so a
// start that cannot win says so in one line. And the stamp takes the swift build
// out of every start that does not need it, legitimate restarts included: a
// build happens only when a watched source input changed, and every decision
// prints its reason. The stamp also keeps the two halves of `target/debug`
// decoupled — the helper is copied in beside the `interface-host` binary the
// watcher at the end of this file polls, so a watcher ever widened from that one
// file to the directory would otherwise see this copy and restart Electron.
async function buildVoiceHelper() {
  const manifest = path.join(voicePackagePath, "Package.swift");
  if (!existsSync(manifest)) {
    throw new Error(`${manifest} is missing; the voice helper cannot be built`);
  }
  const stamp = voiceSourceStamp(voicePackagePath);
  const reason = voiceBuildReason({
    installedPath: voiceInstalledPath, stampPath: voiceStampPath, stamp,
  });
  if (reason === null) {
    console.log(
      "dev.mjs: voice helper is current; skipping its swift build "
      + `(delete ${voiceInstalledPath} to force a rebuild)`,
    );
    return;
  }
  console.log(`dev.mjs: building the voice helper (${reason})`);
  // Package.swift embeds Info.plist from a package-relative linker path, so the
  // build runs with the package itself as its working directory.
  await runChecked("/usr/bin/xcrun", ["swift", "build"], "voice helper build", voicePackagePath);
  mkdirSync(path.dirname(voiceInstalledPath), { recursive: true });
  fs.rmSync(voiceInstalledPath, { force: true });
  fs.copyFileSync(
    path.join(voicePackagePath, ".build", "debug", "interface-voice"), voiceInstalledPath,
  );
  // Stamped after the copy, so an interrupted build never claims to be current.
  if (stamp !== null) fs.writeFileSync(voiceStampPath, stamp);
  else fs.rmSync(voiceStampPath, { force: true });
}

async function waitForVite() {
  const readiness = new URL("/__interface_dev_ready__", devUrl);
  readiness.searchParams.set("run", runId);
  for (let attempt = 0; attempt < 60; attempt += 1) {
    await wait(500);
    try {
      const response = await fetch(readiness, {
        cache: "no-store",
        signal: AbortSignal.timeout(1_000),
      });
      if (response.ok && await response.text() === runId) return;
    } catch { /* the supervised Vite process is not ready yet */ }
  }
  throw new Error(`Vite never identified itself on ${devUrl}`);
}

console.log(
  `dev.mjs: dev profile ${devProfile} (host data ${devHostDataRoot}); `
  + "the installed Interface keeps its own data",
);
if (socketPathBytes > maxSocketPathBytes) {
  console.warn(
    `dev.mjs: ${devHostDataRoot} is too long for a collaboration socket `
    + `(${String(socketPathBytes)} > ${String(maxSocketPathBytes)} bytes); `
    + "cross-model collaboration will report itself unavailable. "
    + "Set INTERFACE_DEV_PROFILE to a shorter path.",
  );
}

// Before any build: see devPortIsBusy (dev-support.mjs).
if (await devPortIsBusy(devHost, devPort)) {
  abort(
    `dev.mjs: another supervisor already owns ${devHost}:${String(devPort)} `
    + "(pid unknown); quit it first",
  );
}

try {
  const cargo = resolveRustTool("cargo");
  const targetDir = cargoTargetDirectory(workspaceRoot, cargo);
  hostBinary = path.join(targetDir, "debug", "interface-host");
  voiceInstalledPath = path.join(targetDir, "debug", "interface-voice");
  voiceStampPath = path.join(targetDir, "debug", ".interface-voice-dev-stamp");
  console.log(`dev.mjs: building the Rust host with ${cargo} into ${targetDir}`);
  await runChecked(cargo, ["build", "-p", "interface-host"], "Rust host build", workspaceRoot);
  await buildVoiceHelper();
  supervise(run(tool("vite"), [
    "--host", devHost,
    "--port", String(devPort),
    "--strictPort",
    "--mode", runId,
  ]), "Vite");
  await Promise.all([
    waitForVite(),
    runChecked(tool("tsc"), ["-p", "tsconfig.main.json"], "initial main compile"),
    // Dock/Spotlight launches load dist/index.html after the supervisor quits.
    // Refresh that renderer too, so reopening Dev retains the verified UI.
    runChecked(tool("vite"), ["build"], "Dock renderer build"),
    runChecked(tool("vite"), ["build", "--config", "vite.remote.config.ts"], "iPhone renderer build"),
  ]);
} catch (error) {
  abort(`dev.mjs: ${error instanceof Error ? error.message : String(error)}`);
}

supervise(run(tool("tsc"), [
  "-p", "tsconfig.main.json", "--watch", "--preserveWatchOutput",
]), "TypeScript watcher");

// The dev instance is isolated from the installed app: its own Electron profile
// (`--user-data-dir`) so the single-instance lock does not collide, and its own
// Rust host data root so the two hosts never write the same canonical stores.
// `INTERFACE_HOST_TEST_HOME` stays the real home so providers keep working.
//
// Live updates, four paths:
//   * renderer edits (`src/renderer/**`) — Vite swaps them in place over HMR;
//     nothing restarts.
//   * main-process edits (`src/main/**`) — the tsc watcher emits into
//     `dist-main`, and the watcher below restarts Electron (~2 s).
//   * Rust host edits — run `cargo build -p interface-host` in another terminal;
//     the rebuilt binary restarts Electron, which respawns the host child.
//   * voice helper edits — the helper is built at startup, and only when
//     `apps/interface-voice` changed since the last build (see the stamp above);
//     the line it prints says which. To pick up an edit without restarting the
//     supervisor, run `swift build` in `apps/interface-voice` and copy the
//     product over the `interface-voice` beside the host binary this script
//     names on startup: the host spawns a fresh helper on the next toggle, so
//     nothing has to restart.
mkdirSync(devHostDataRoot, { recursive: true, mode: 0o700 });

let electronChild = null;
let electronStartedAt = 0;
let restartPending = false;

function resolveDevElectron() {
  if (process.platform !== "darwin") {
    return { command: tool("electron"), args: [".", `--user-data-dir=${devProfile}`] };
  }
  const packageJson = JSON.parse(readFileSync(path.join(electronRoot, "package.json"), "utf8"));
  const materialized = materializeDevApp({
    destDir: path.join(electronRoot, ".dev-app"),
    electronDist: path.join(electronRoot, "node_modules", "electron", "dist"),
    electronRoot,
    // A Dock or Spotlight launch inherits no environment, so the resolved host
    // path is baked into the bundle's own launcher rather than passed to it.
    hostBinary,
    iconPath: path.join(electronRoot, "build", "icon-dev.icns"),
    version: String(packageJson.version ?? ""),
  });
  const reinstall = !materialized.reused || !existsSync(electronExecutable(DEV_APP_INSTALL_PATH))
    || !installedDevAppMatches(materialized.appPath, DEV_APP_INSTALL_PATH);
  const installed = reinstall
    ? installDevApp(materialized.appPath, DEV_APP_INSTALL_PATH)
    : { appPath: DEV_APP_INSTALL_PATH, executablePath: electronExecutable(DEV_APP_INSTALL_PATH) };
  console.log(
    reinstall
      ? `dev.mjs: installed ${DEV_APP_DISPLAY_NAME} at ${installed.appPath}`
      : `dev.mjs: launching ${DEV_APP_DISPLAY_NAME} (${installed.appPath})`,
  );
  return {
    command: installed.executablePath,
    args: [electronRoot, `--user-data-dir=${devProfile}`],
  };
}

let devElectron;
try {
  devElectron = resolveDevElectron();
} catch (error) {
  abort(`dev.mjs: ${error instanceof Error ? error.message : String(error)}`);
}

function startElectron() {
  electronStartedAt = Date.now();
  const child = run(devElectron.command, devElectron.args, {
    env: {
      INTERFACE_DEV_SUPERVISED: "1",
      // The main process would otherwise guess `<checkout>/target/debug`, which
      // in a worktree is missing or stale. Hand it the binary the build above
      // actually produced.
      INTERFACE_RUST_HOST: hostBinary,
      VITE_DEV_SERVER_URL: devUrl,
      INTERFACE_HOST_TEST_DATA_ROOT: devHostDataRoot,
      INTERFACE_HOST_TEST_HOME: os.homedir(),
    },
  });
  electronChild = child;
  child.once("error", error => {
    abort(`dev.mjs: failed to start Electron: ${error.message}`);
  });
  child.once("exit", (code, signal) => {
    const slot = children.indexOf(child);
    if (slot !== -1) children.splice(slot, 1);
    if (electronChild === child) electronChild = null;
    if (shuttingDown) return;
    if (restartPending) {
      restartPending = false;
      startElectron();
      return;
    }
    shuttingDown = true;
    stopChildren();
    if (signal !== null) {
      process.exit(128 + (os.constants.signals[signal] ?? 0));
    }
    process.exit(code ?? 0);
  });
  return child;
}

function scheduleRestart(reason) {
  if (shuttingDown) return;
  if (restartTimer !== null) clearTimeout(restartTimer);
  restartTimer = setTimeout(() => {
    restartTimer = null;
    if (shuttingDown) return;
    console.log(`dev.mjs: ${reason} changed, restarting Electron`);
    if (electronChild === null) {
      restartPending = false;
      startElectron();
      return;
    }
    restartPending = true;
    electronChild.kill("SIGTERM");
  }, 400);
}

startElectron();

const distMain = path.join(electronRoot, "dist-main");
if (existsSync(distMain)) {
  const emittedAt = name => {
    try { return fs.statSync(path.join(distMain, name)).mtimeMs; } catch { return null; }
  };
  const distWatcher = fs.watch(distMain, { recursive: true }, (_event, name) => {
    if (!isNewerMainBuild(name, electronStartedAt, emittedAt)) return;
    scheduleRestart("main process");
  });
  distWatcher.on("error", () => { /* a dropped watch must not kill the supervisor */ });
  watchers.push(distWatcher);
} else {
  console.log(`dev.mjs: ${distMain} is missing; main-process restarts are off`);
}

watchedHostBinary = hostBinary;
fs.watchFile(hostBinary, { interval: 1_000 }, (curr, prev) => {
  if (curr.mtimeMs !== prev.mtimeMs && curr.size > 0) scheduleRestart("Rust host");
});
