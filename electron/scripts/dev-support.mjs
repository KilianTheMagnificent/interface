import { execFileSync } from "node:child_process";
import fs, { existsSync, readFileSync } from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { findExecutable } from "./cargo-target.mjs";

// The dev supervisor's decisions, kept apart from dev.mjs so they can be run in
// a test: dev.mjs starts Vite, tsc and Electron the moment it is imported.

// `nohup npm run dev` inherits a login PATH that has no `$HOME/.cargo/bin` on
// it, which is how the 2026-09-04 relaunch died on a bare `spawn cargo ENOENT`.
// Resolve every Rust tool before spawning it, so a missing toolchain says which
// two places were searched instead of naming a syscall.
export function resolveRustTool(name) {
  const found = findExecutable(name);
  if (found !== null) return found;
  const cargoBinDir = path.join(os.homedir(), ".cargo", "bin");
  throw new Error(
    `${name} is not executable anywhere on PATH and ${path.join(cargoBinDir, name)} `
    + "is missing; install the Rust toolchain with rustup, or put "
    + `${cargoBinDir} on PATH before starting the supervisor`,
  );
}

function swiftToolchainVersions() {
  return [["--version"], ["swift", "--version"]].map(args =>
    execFileSync("/usr/bin/xcrun", args, {
      encoding: "utf8", stdio: ["ignore", "pipe", "pipe"],
    }).trim());
}

// Size and mtime of Package.swift and every file under Sources/, plus the
// toolchain that compiles them — a Command Line Tools upgrade changes the
// product without touching a source file, so it has to change the stamp too.
// Null means an input could not be read, and an unreadable input has to mean
// "build", never "skip".
export function voiceSourceStamp(packagePath, toolchainVersions = swiftToolchainVersions) {
  const lines = [];
  const walk = entry => {
    const stats = fs.statSync(entry);
    if (stats.isDirectory()) {
      for (const name of fs.readdirSync(entry).sort()) walk(path.join(entry, name));
      return;
    }
    lines.push(`${path.relative(packagePath, entry)} ${String(stats.size)} ${
      String(stats.mtimeMs)}`);
  };
  try {
    walk(path.join(packagePath, "Package.swift"));
    walk(path.join(packagePath, "Sources"));
    lines.push(...toolchainVersions());
  } catch { return null; }
  return lines.join("\n");
}

/** Why the voice helper has to be built, or null when the installed one is current. */
export function voiceBuildReason({ installedPath, stampPath, stamp }) {
  if (!existsSync(installedPath)) return `${installedPath} is missing`;
  if (stamp === null) return "the voice sources could not be read";
  let recorded = null;
  try { recorded = readFileSync(stampPath, "utf8"); } catch { /* never stamped */ }
  if (recorded === null) return "no previous voice build is recorded";
  if (recorded !== stamp) return "apps/interface-voice sources or the Swift toolchain changed";
  return null;
}

// Vite takes the dev port with `--strictPort`, so a second supervisor can only
// lose it — but it discovers that after a cargo build, a swift build and a
// materialized dev app. Ask the port first: one TCP connect, and a start that
// cannot win costs a line instead of a build. See the note above
// buildVoiceHelper in dev.mjs for what made this worth having. A port that
// refuses or never answers is free.
export function devPortIsBusy(host, port, timeoutMs = 500) {
  return new Promise(resolve => {
    const socket = net.createConnection({ host, port });
    const settle = busy => {
      socket.destroy();
      resolve(busy);
    };
    socket.setTimeout(timeoutMs);
    socket.once("connect", () => { settle(true); });
    socket.once("timeout", () => { settle(false); });
    socket.once("error", () => { settle(false); });
  });
}
