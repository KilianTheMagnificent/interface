// Starts the real Interface desktop app for hands-free agent testing: builds the
// checkout's debug interface-host and the renderer/main bundles, then hands the
// unpackaged app to the vendored driver (scripts/handsfree.mjs), which runs it
// hidden, never frontmost, on a throwaway profile (see src/main/handsfree.ts).
//   npm run handsfree                  build, then start
//   npm run handsfree -- --no-build    start the last build
// Drive it afterwards from electron/ (the session is keyed to that directory):
//   node scripts/handsfree.mjs wait '.workspace-toolbar' | click | type | shot | stop
import { spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import electron from "electron";
import { cargoBinary } from "./cargo-target.mjs";

const electronRoot = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const workspaceRoot = path.join(electronRoot, "..");
const args = process.argv.slice(2);
const build = !args.includes("--no-build");
const forwarded = args.filter(arg => arg !== "--no-build");
// Agent shells often lack rustup's bin directory; everything spawned below needs cargo.
const env = { ...process.env, PATH: [process.env.PATH, path.join(os.homedir(), ".cargo", "bin")].filter(Boolean).join(path.delimiter) };
delete env.ELECTRON_RUN_AS_NODE;

function run(command, commandArgs, cwd) {
  // Heavy builds yield to the owner's foreground work.
  const result = spawnSync("nice", ["-n", "10", command, ...commandArgs], { cwd, env, stdio: ["ignore", 2, 2] });
  if (result.status !== 0) {
    console.error(`handsfree: ${command} ${commandArgs.join(" ")} failed`);
    process.exit(result.status ?? 1);
  }
}

if (build) {
  run("cargo", ["build", "--package", "interface-host"], workspaceRoot);
  run("npm", ["run", "build"], electronRoot);
}
const host = cargoBinary(workspaceRoot, "debug", "interface-host");
for (const required of [host, path.join(electronRoot, "dist", "index.html"), path.join(electronRoot, "dist-main", "main", "main.js")]) {
  if (!fs.existsSync(required)) {
    console.error(`handsfree: ${required} is missing; run without --no-build`);
    process.exit(1);
  }
}
// Stock Electron keeps AppKit state (the shared com.github.Electron defaults
// domain, saved window state) under the real ~/Library. Point Foundation at the
// session, as the driver does for native apps; its `stop` deletes the folder.
// The session directory is the driver's: --session, HANDSFREE_SESSION, or a
// hash of its working directory.
const named = forwarded.indexOf("--session");
const sessionName = (named >= 0 && forwarded[named + 1]) || process.env.HANDSFREE_SESSION
  || crypto.createHash("sha1").update(fs.realpathSync(electronRoot)).digest("hex").slice(0, 8);
const sessionDirectory = path.join(os.tmpdir(), "handsfree", sessionName);
const foundationHome = path.join(sessionDirectory, "home");
if (!fs.existsSync(path.join(sessionDirectory, "session.json"))) {
  fs.mkdirSync(foundationHome, { recursive: true, mode: 0o700 });
}
const started = spawnSync(process.execPath, [
  path.join(electronRoot, "scripts", "handsfree.mjs"), "start", "electron", electronRoot, "--electron", electron, ...forwarded,
], {
  cwd: electronRoot,
  env: { ...env, INTERFACE_RUST_HOST: host, CFFIXED_USER_HOME: foundationHome },
  stdio: ["ignore", "pipe", "inherit"],
  encoding: "utf8",
});
process.stdout.write(started.stdout ?? "");
if (started.status !== 0) process.exit(started.status ?? 1);
// The window is created before the host boots and loads the renderer later, so
// a driver command sent at once can land on the blank page and lose it to that
// navigation. Return only once the renderer's own page is the target.
const { port } = JSON.parse(started.stdout.trim().split("\n").pop());
const renderer = pathToFileURL(path.join(electronRoot, "dist", "index.html")).href;
for (const deadline = Date.now() + 120_000; ;) {
  const pages = await fetch(`http://127.0.0.1:${port}/json/list`).then(r => r.json(), () => []);
  if (pages.some(page => page.type === "page" && page.url === renderer)) break;
  if (Date.now() > deadline) {
    console.error("handsfree: the renderer never loaded; read `node scripts/handsfree.mjs logs`");
    process.exit(1);
  }
  await new Promise(resolve => setTimeout(resolve, 200));
}
console.error("handsfree: drive from electron/ with node scripts/handsfree.mjs <command>, or pass --session");
