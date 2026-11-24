import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
  chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, utimesSync, writeFileSync,
} from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { afterEach, test } from "node:test";
import { findExecutable } from "./cargo-target.mjs";
import {
  DEV_APP_BUNDLE_ID, DEV_APP_COPYRIGHT, DEV_APP_DISPLAY_NAME, DEV_APP_FOLDER_USAGE,
  DEV_APP_CAMERA_USAGE, DEV_APP_INSTALL_PATH, DEV_APP_LAUNCHER_REVISION, DEV_APP_MICROPHONE_USAGE, PRODUCTION_APP_PATH,
  assertSafeInstallDestination, bundleStampPath, destAppPath, electronExecutable, installDevApp,
  installedDevAppMatches, isNewerMainBuild, materializeDevApp,
} from "./dev-app.mjs";
import {
  devPortIsBusy, resolveRustTool, voiceBuildReason, voiceSourceStamp,
} from "./dev-support.mjs";

const roots = [];

afterEach(() => {
  while (roots.length > 0) {
    rmSync(roots.pop(), { recursive: true, force: true });
  }
});

function tempRoot() {
  const root = mkdtempSync(path.join(os.tmpdir(), "interface-dev-app-"));
  roots.push(root);
  return root;
}

function writePlist(file, entries) {
  const body = Object.entries(entries)
    .map(([key, value]) => `  <key>${key}</key>\n  <string>${value}</string>`)
    .join("\n");
  writeFileSync(file, `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
${body}
</dict>
</plist>
`);
}

function plistString(appPath, key) {
  return execFileSync("/usr/bin/plutil", [
    "-extract", key, "raw", path.join(appPath, "Contents", "Info.plist"),
  ], { stdio: ["ignore", "pipe", "pipe"] }).toString().trim();
}

function seedElectronDist(root, overrides = {}) {
  const dist = path.join(root, "electron-dist");
  const app = path.join(dist, "Electron.app");
  const contents = path.join(app, "Contents");
  const macos = path.join(contents, "MacOS");
  const resources = path.join(contents, "Resources");
  mkdirSync(macos, { recursive: true });
  mkdirSync(resources, { recursive: true });
  writeFileSync(path.join(dist, "version"), `${overrides.electronVersion ?? "43.4.1"}\n`);
  writePlist(path.join(contents, "Info.plist"), {
    CFBundleDisplayName: "Electron",
    CFBundleExecutable: "Electron",
    CFBundleIconFile: "electron.icns",
    CFBundleIdentifier: "com.github.Electron",
    CFBundleName: "Electron",
    CFBundleShortVersionString: "43.4.1",
    CFBundleVersion: "43.4.1",
  });
  const binary = path.join(macos, "Electron");
  writeFileSync(binary, overrides.binaryContents ?? "electron-binary");
  chmodSync(binary, 0o755);
  writeFileSync(path.join(resources, "electron.icns"), overrides.stockIcon ?? "stock-icon");
  writeFileSync(path.join(resources, "default_app.asar"), "stock-default-app");
  return dist;
}

function seedIcon(root, contents = "fold-icon") {
  const iconPath = path.join(root, "icon.icns");
  writeFileSync(iconPath, contents);
  return iconPath;
}

function materialize(root, overrides = {}) {
  return materializeDevApp({
    destDir: path.join(root, "dev-app"),
    electronDist: overrides.electronDist ?? seedElectronDist(root),
    electronRoot: overrides.electronRoot ?? path.join(root, "project", "electron"),
    iconPath: overrides.iconPath ?? seedIcon(root),
    sign: false,
    version: overrides.version ?? "0.3.22",
    ...overrides,
  });
}

test("clones Electron.app into a named Interface Dev.app with the product identity", () => {
  const root = tempRoot();
  const result = materialize(root);
  const expectedApp = destAppPath(path.join(root, "dev-app"));
  assert.equal(result.reused, false);
  assert.equal(result.appPath, expectedApp);
  assert.equal(result.executablePath, electronExecutable(expectedApp));
  assert.equal(existsSync(result.executablePath), true);
  assert.equal(plistString(result.appPath, "CFBundleName"), DEV_APP_DISPLAY_NAME);
  assert.equal(plistString(result.appPath, "CFBundleDisplayName"), DEV_APP_DISPLAY_NAME);
  assert.equal(plistString(result.appPath, "CFBundleExecutable"), "Electron");
  assert.equal(plistString(result.appPath, "CFBundleIdentifier"), DEV_APP_BUNDLE_ID);
  assert.equal(plistString(result.appPath, "CFBundleIconFile"), "icon.icns");
  assert.equal(plistString(result.appPath, "CFBundleShortVersionString"), "0.3.22");
  assert.equal(plistString(result.appPath, "CFBundleVersion"), "0.3.22");
  assert.equal(plistString(result.appPath, "NSHumanReadableCopyright"), DEV_APP_COPYRIGHT);
  assert.equal(plistString(result.appPath, "NSDesktopFolderUsageDescription"), DEV_APP_FOLDER_USAGE);
  assert.equal(plistString(result.appPath, "NSDocumentsFolderUsageDescription"), DEV_APP_FOLDER_USAGE);
  assert.equal(plistString(result.appPath, "NSDownloadsFolderUsageDescription"), DEV_APP_FOLDER_USAGE);
  assert.equal(plistString(result.appPath, "NSMicrophoneUsageDescription"), DEV_APP_MICROPHONE_USAGE);
  assert.equal(plistString(result.appPath, "NSCameraUsageDescription"), DEV_APP_CAMERA_USAGE);
  assert.equal(readFileSync(path.join(result.appPath, "Contents", "Resources", "icon.icns"), "utf8"), "fold-icon");
  assert.equal(readFileSync(path.join(result.appPath, "Contents", "Resources", "electron.icns"), "utf8"), "fold-icon");
  const launcher = path.join(result.appPath, "Contents", "Resources", "default_app", "main.js");
  const asarPath = path.join(result.appPath, "Contents", "Resources", "default_app.asar");
  assert.equal(existsSync(asarPath), true);
  assert.notEqual(readFileSync(asarPath), Buffer.from("stock-default-app"));
  const launcherSource = readFileSync(launcher, "utf8");
  assert.match(launcherSource, /Iface Dev/);
  assert.match(launcherSource, /INTERFACE_DEV_SUPERVISED !== "1"/);
  assert.match(launcherSource, /delete process\.env\.VITE_DEV_SERVER_URL/);
  assert.equal(launcherSource.includes(path.resolve(root, "project", "electron")), true);
  assert.equal(DEV_APP_LAUNCHER_REVISION, 6);
  const supervisorSource = readFileSync(new URL("./dev.mjs", import.meta.url), "utf8");
  assert.match(supervisorSource, /INTERFACE_DEV_SUPERVISED: "1"/);
  // The supervisor tells the main process which host it just built; a Dock
  // launch inherits no environment, so the bundle carries the same path itself.
  assert.match(supervisorSource, /INTERFACE_RUST_HOST: hostBinary/);
  assert.match(launcherSource, /process\.env\.INTERFACE_RUST_HOST = host;/);
  assert.notEqual(DEV_APP_BUNDLE_ID, "com.andreromero.interface");
  assert.notEqual(DEV_APP_DISPLAY_NAME, "Interface");
});

// The host binary is wherever cargo built it, which `CARGO_TARGET_DIR` or a
// user-level cargo config can move out of the checkout, so the launcher cannot
// derive it from its own checkout. A bundle left pointing at a stale
// `<checkout>/target/debug` would still find a binary there and launch it,
// which is the failure that says nothing.
test("bakes the supplied host binary in and re-materializes when it moves", () => {
  const root = tempRoot();
  const electronDist = seedElectronDist(root);
  const iconPath = seedIcon(root);
  const launcherOf = result => readFileSync(
    path.join(result.appPath, "Contents", "Resources", "default_app", "main.js"), "utf8",
  );
  const shared = path.join(root, "shared-target", "debug", "interface-host");
  const first = materialize(root, { electronDist, iconPath, hostBinary: shared });
  assert.equal(first.reused, false);
  assert.equal(launcherOf(first).includes(JSON.stringify(shared)), true);
  assert.equal(
    materialize(root, { electronDist, iconPath, hostBinary: shared }).reused, true,
  );
  const moved = path.join(root, "other-target", "debug", "interface-host");
  const rebuilt = materialize(root, { electronDist, iconPath, hostBinary: moved });
  assert.equal(rebuilt.reused, false);
  assert.equal(launcherOf(rebuilt).includes(JSON.stringify(moved)), true);
});

test("reuses a stamped bundle and rebuilds when identity, version, or icon change", () => {
  const root = tempRoot();
  const electronDist = seedElectronDist(root);
  const iconPath = seedIcon(root, "icon-a");
  const first = materialize(root, { electronDist, iconPath, version: "0.3.22" });
  const second = materialize(root, { electronDist, iconPath, version: "0.3.22" });
  assert.equal(second.reused, true);
  assert.equal(second.appPath, first.appPath);

  const bumped = materialize(root, { electronDist, iconPath, version: "0.3.23" });
  assert.equal(bumped.reused, false);
  assert.equal(plistString(bumped.appPath, "CFBundleShortVersionString"), "0.3.23");

  writeFileSync(iconPath, "icon-b");
  const reiconed = materialize(root, { electronDist, iconPath, version: "0.3.23" });
  assert.equal(reiconed.reused, false);
  assert.equal(
    readFileSync(path.join(reiconed.appPath, "Contents", "Resources", "icon.icns"), "utf8"),
    "icon-b",
  );
});

// The stamp is compared as JSON text, so its key order is part of it: a
// reorder would re-materialize, re-sign and reinstall every checkout's bundle.
test("keeps the stamp's key order stable", () => {
  const root = tempRoot();
  materialize(root);
  const stamp = JSON.parse(readFileSync(path.join(root, "dev-app", "stamp.json"), "utf8"));
  assert.deepEqual(Object.keys(stamp), [
    "bundleId", "displayName", "electronVersion", "iconSha256", "electronRoot",
    "hostBinary", "launcherRevision", "signed", "sourceApp", "version",
  ]);
});

test("rebuilds when the stamped executable is missing", () => {
  const root = tempRoot();
  const electronDist = seedElectronDist(root);
  const first = materialize(root, { electronDist });
  rmSync(first.executablePath);
  const rebuilt = materialize(root, { electronDist });
  assert.equal(rebuilt.reused, false);
  assert.equal(existsSync(rebuilt.executablePath), true);
});

// The installed bundle is shared by every checkout, so this checkout's own
// stamp cannot say whether the installed copy is still this checkout's.
test("reinstalls when the shared installed copy came from another checkout", () => {
  const root = tempRoot();
  const electronDist = seedElectronDist(root);
  const iconPath = seedIcon(root);
  const dest = path.join(root, "Applications", "Interface Dev.app");
  const ours = materialize(root, { electronDist, iconPath });
  const stamped = JSON.parse(readFileSync(bundleStampPath(ours.appPath), "utf8"));
  assert.deepEqual(stamped, JSON.parse(readFileSync(path.join(root, "dev-app", "stamp.json"), "utf8")));
  assert.equal(installedDevAppMatches(ours.appPath, dest), false);
  installDevApp(ours.appPath, dest);
  assert.equal(installedDevAppMatches(ours.appPath, dest), true);

  const theirs = materializeDevApp({
    destDir: path.join(root, "other-checkout", "dev-app"),
    electronDist, iconPath, sign: false, version: "0.3.22",
    electronRoot: path.join(root, "other-checkout", "electron"),
  });
  installDevApp(theirs.appPath, dest);
  assert.equal(materialize(root, { electronDist, iconPath }).reused, true);
  assert.equal(installedDevAppMatches(ours.appPath, dest), false);

  installDevApp(ours.appPath, dest);
  assert.equal(installedDevAppMatches(ours.appPath, dest), true);
  rmSync(bundleStampPath(dest));
  assert.equal(installedDevAppMatches(ours.appPath, dest), false);

  const supervisorSource = readFileSync(new URL("./dev.mjs", import.meta.url), "utf8");
  assert.match(supervisorSource,
    /\|\| !installedDevAppMatches\(materialized\.appPath, DEV_APP_INSTALL_PATH\);/u);
});

test("re-materializes a bundle that lacks its in-bundle stamp", () => {
  const root = tempRoot();
  const electronDist = seedElectronDist(root);
  const first = materialize(root, { electronDist });
  rmSync(bundleStampPath(first.appPath));
  const rebuilt = materialize(root, { electronDist });
  assert.equal(rebuilt.reused, false);
  assert.equal(existsSync(bundleStampPath(rebuilt.appPath)), true);
});

test("install copies Interface Dev.app and refuses to replace Interface.app", () => {
  const root = tempRoot();
  const source = materialize(root).appPath;
  const applications = path.join(root, "Applications");
  const dest = path.join(applications, "Interface Dev.app");
  const installed = installDevApp(source, dest);
  assert.equal(installed.appPath, dest);
  assert.equal(existsSync(electronExecutable(dest)), true);
  assert.equal(plistString(dest, "CFBundleIdentifier"), DEV_APP_BUNDLE_ID);
  assert.equal(DEV_APP_INSTALL_PATH, "/Applications/Interface Dev.app");
  assert.equal(PRODUCTION_APP_PATH, "/Applications/Interface.app");

  assert.throws(
    () => assertSafeInstallDestination("/Applications/Interface.app"),
    /refusing to replace Interface\.app/,
  );
  assert.throws(
    () => assertSafeInstallDestination(path.join(root, "Interface.app")),
    /must be named Interface Dev\.app/,
  );
  assert.throws(
    () => installDevApp(source, "/Applications/Interface.app"),
    /refusing to replace Interface\.app/,
  );
  assert.throws(
    () => installDevApp(path.join(root, "missing.app"), dest),
    /is not a materialized Interface Dev\.app/,
  );
});

test("fails closed when Electron.app or the product icon is missing", () => {
  const root = tempRoot();
  const destDir = path.join(root, "dev-app");
  const iconPath = seedIcon(root);
  assert.throws(
    () => materializeDevApp({
      destDir,
      electronDist: path.join(root, "missing-dist"),
      electronRoot: path.join(root, "project", "electron"),
      iconPath,
      sign: false,
      version: "0.3.22",
    }),
    /Electron\.app is missing/,
  );
  const electronDist = seedElectronDist(root);
  assert.throws(
    () => materializeDevApp({
      destDir,
      electronDist,
      electronRoot: path.join(root, "project", "electron"),
      iconPath: path.join(root, "missing.icns"),
      sign: false,
      version: "0.3.22",
    }),
    /is missing; run npm run build:icon/,
  );
  assert.throws(
    () => materializeDevApp({
      destDir,
      electronDist,
      iconPath,
      sign: false,
      version: "0.3.22",
    }),
    /electronRoot is required/,
  );
});

test("only a dist-main file newer than the running Electron is a new build", () => {
  const startedAt = 1_000;
  const emittedAt = () => 900;
  // The bug this guards: macOS replays one compile's events while Electron
  // reads dist-main, and every replay used to restart the app again.
  assert.equal(isNewerMainBuild("main/main.js", startedAt, emittedAt), false);
  assert.equal(isNewerMainBuild("main/main.js", startedAt, () => 1_001), true);
  assert.equal(isNewerMainBuild("main/main.js", startedAt, () => startedAt), false);
  assert.equal(isNewerMainBuild("main/main.js.map", startedAt, () => 1_001), false);
  assert.equal(isNewerMainBuild("tsconfig.main.tsbuildinfo", startedAt, () => 1_001), false);
  // A file deleted between the event and the stat must not restart anything.
  assert.equal(isNewerMainBuild("main/main.js", startedAt, () => null), false);
  assert.equal(isNewerMainBuild(undefined, startedAt, () => 1_001), false);
});

// dev.mjs starts Vite, tsc and Electron the moment it is imported, so its
// decisions live in dev-support.mjs and are run below; only how dev.mjs orders
// them is read as text, the way this file already reads make-icon.mjs.
const supervisorSource = () =>
  readFileSync(new URL("./dev.mjs", import.meta.url), "utf8");

function withEnvironment(values, run) {
  const saved = Object.fromEntries(Object.keys(values).map(key => [key, process.env[key]]));
  Object.assign(process.env, values);
  try {
    return run();
  } finally {
    for (const [key, value] of Object.entries(saved)) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
  }
}

test("the supervisor resolves cargo before spawning it and names both places", () => {
  const root = tempRoot();
  const shadow = path.join(root, "shadow");
  const plain = path.join(root, "plain");
  const tools = path.join(root, "tools");
  const home = path.join(root, "home");
  // A directory can carry the execute bit; only an executable file is the tool.
  mkdirSync(path.join(shadow, "cargo"), { recursive: true });
  chmodSync(path.join(shadow, "cargo"), 0o755);
  mkdirSync(plain, { recursive: true });
  writeFileSync(path.join(plain, "cargo"), "not executable");
  chmodSync(path.join(plain, "cargo"), 0o644);
  mkdirSync(tools, { recursive: true });
  writeFileSync(path.join(tools, "cargo"), "#!/bin/sh\n");
  chmodSync(path.join(tools, "cargo"), 0o755);
  const onPath = [shadow, "", plain, tools].join(path.delimiter);
  withEnvironment({ PATH: onPath, HOME: home }, () => {
    assert.equal(findExecutable("cargo"), path.join(tools, "cargo"));
    assert.equal(resolveRustTool("cargo"), path.join(tools, "cargo"));
  });
  // `nohup npm run dev` has no $HOME/.cargo/bin on PATH: rustup's directory is
  // searched after PATH.
  const rustup = path.join(home, ".cargo", "bin");
  mkdirSync(rustup, { recursive: true });
  writeFileSync(path.join(rustup, "cargo"), "#!/bin/sh\n");
  chmodSync(path.join(rustup, "cargo"), 0o755);
  withEnvironment({ PATH: [shadow, plain].join(path.delimiter), HOME: home }, () => {
    assert.equal(findExecutable("cargo"), path.join(rustup, "cargo"));
  });
  // Spawning the bare name failed with ENOENT, which says nothing about what
  // to install or where; the refusal names both places and rustup.
  withEnvironment({ PATH: plain, HOME: home }, () => {
    assert.equal(findExecutable("rustfmt"), null);
    assert.throws(() => resolveRustTool("rustfmt"), error =>
      error.message === `rustfmt is not executable anywhere on PATH and ${
        path.join(rustup, "rustfmt")} is missing; install the Rust toolchain with rustup, `
        + `or put ${rustup} on PATH before starting the supervisor`);
  });
  const source = supervisorSource();
  assert.doesNotMatch(source, /runChecked\("cargo"/u);
  assert.match(source, /resolveRustTool\("cargo"\)/u);
});

test("a second supervisor loses the dev port before it builds anything", async () => {
  // A transient launchd KeepAlive job respawned `npm run dev` 4,637 times on
  // 2026-09-04 while a healthy supervisor held 5199. Every one of those starts
  // ran cargo and swift before --strictPort told it it had already lost.
  const server = net.createServer();
  await new Promise(resolve => { server.listen(0, "127.0.0.1", resolve); });
  const { port } = server.address();
  assert.equal(await devPortIsBusy("127.0.0.1", port), true);
  await new Promise(resolve => { server.close(resolve); });
  // A port that refuses or never answers is a free port: neither may abort.
  assert.equal(await devPortIsBusy("127.0.0.1", port), false);
  assert.equal(await devPortIsBusy("192.0.2.1", 9, 50), false);
  // The check earns its keep only by running before the two builds.
  const source = supervisorSource();
  const checked = source.indexOf("if (await devPortIsBusy(devHost, devPort))");
  assert.ok(checked !== -1);
  assert.ok(checked < source.indexOf("another supervisor already owns"));
  assert.ok(checked < source.indexOf("resolveRustTool(\"cargo\")"));
  assert.ok(checked < source.indexOf("await buildVoiceHelper();"));
});

test("the voice helper is rebuilt only when its own sources changed", () => {
  // The 2026-09-04 symptom: "Building for debugging… Build complete!" every
  // 10-30 s for hours behind a stable Electron pid, because every restarted
  // supervisor paid for a swift build before it could find out it was the
  // second one. The build now needs a reason, and prints the one it has.
  const root = tempRoot();
  const voice = path.join(root, "interface-voice");
  mkdirSync(path.join(voice, "Sources", "Voice"), { recursive: true });
  writeFileSync(path.join(voice, "Package.swift"), "// swift-tools-version:5.9\n");
  const source = path.join(voice, "Sources", "Voice", "main.swift");
  writeFileSync(source, "print(1)\n");
  let toolchain = ["Xcode 26", "swift 6.2"];
  const stamp = () => voiceSourceStamp(voice, () => toolchain);
  const first = stamp();
  assert.equal(typeof first, "string");
  assert.equal(stamp(), first);
  // A touched source, an edited one, and a toolchain upgrade each change it.
  utimesSync(source, new Date(1_000_000), new Date(2_000_000));
  const touched = stamp();
  assert.notEqual(touched, first);
  writeFileSync(source, "print(12)\n");
  utimesSync(source, new Date(1_000_000), new Date(2_000_000));
  assert.notEqual(stamp(), touched);
  const edited = stamp();
  toolchain = ["Xcode 27", "swift 6.3"];
  assert.notEqual(stamp(), edited);
  // An unreadable tree or toolchain is null, and null means build.
  assert.equal(voiceSourceStamp(voice, () => { throw new Error("no xcrun"); }), null);
  assert.equal(voiceSourceStamp(path.join(root, "missing"), () => toolchain), null);

  const installedPath = path.join(root, "debug", "interface-voice");
  const stampPath = path.join(root, "debug", ".interface-voice-dev-stamp");
  const reason = current => voiceBuildReason({ installedPath, stampPath, stamp: current });
  assert.equal(reason(edited), `${installedPath} is missing`);
  mkdirSync(path.dirname(installedPath), { recursive: true });
  writeFileSync(installedPath, "helper");
  assert.equal(reason(null), "the voice sources could not be read");
  assert.equal(reason(edited), "no previous voice build is recorded");
  writeFileSync(stampPath, edited);
  assert.equal(reason(edited), null);
  assert.equal(reason(stamp()), "apps/interface-voice sources or the Swift toolchain changed");

  // How dev.mjs uses them: once per supervisor, stamped only after the copy,
  // and the watcher polls the host binary file beside which the helper lands.
  const supervisor = supervisorSource();
  assert.equal(supervisor.match(/async function buildVoiceHelper\(\)/gu)?.length, 1);
  assert.equal(supervisor.match(/^ {2}await buildVoiceHelper\(\);$/gmu)?.length, 1);
  assert.ok(supervisor.indexOf("fs.copyFileSync(") < supervisor.indexOf("fs.writeFileSync(voiceStampPath"));
  assert.match(supervisor, /fs\.watchFile\(hostBinary, \{ interval: 1_000 \}/u);
  assert.doesNotMatch(supervisor, /fs\.watch\(path\.join\(workspaceRoot, "target"/u);
});

test("the icon the stamp hashes comes from one rasterizer, never a fallback", () => {
  // materializeDevApp rebuilds whenever iconSha256 changes, so a second
  // rasterizer producing different bytes would restamp the bundle by accident.
  const makeIcon = readFileSync(new URL("./make-icon.mjs", import.meta.url), "utf8");
  assert.match(makeIcon, /throw new Error\(`icon rasterizer needs \$\{chrome\}/u);
  assert.doesNotMatch(makeIcon, /"-s", "format", "png"/u);
});
