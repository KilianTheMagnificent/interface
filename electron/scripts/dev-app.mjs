import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  copyFileSync, existsSync, lstatSync, mkdirSync, readFileSync, realpathSync, rmSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { appBuilderLibRequire } from "./builder-require.cjs";

export const DEV_APP_DISPLAY_NAME = "Interface Dev";
export const DEV_APP_BUNDLE_ID = "com.andreromero.interface.dev";
export const DEV_APP_FOLDER_USAGE =
  "Interface Dev opens the project folders your agent sessions run in, so it can show their files and the changes an agent proposes.";
export const DEV_APP_MICROPHONE_USAGE =
  "Interface uses the microphone for the voice assistant.";
export const DEV_APP_CAMERA_USAGE =
  "Interface lets a web page in its Browser use the camera when you allow it.";
export const DEV_APP_COPYRIGHT = "Copyright © 2026 Andre Romero";
export const DEV_APP_INSTALL_PATH = "/Applications/Interface Dev.app";
export const PRODUCTION_APP_PATH = "/Applications/Interface.app";
export const DEV_APP_LAUNCHER_REVISION = 6;

const STAMP_NAME = "stamp.json";
// The same stamp travels inside the bundle, written before the ad-hoc signature
// seals Resources, so the shared installed copy can be compared with it.
const BUNDLE_STAMP = path.join("Contents", "Resources", "interface-dev-stamp.json");

// macOS re-delivers a compile's FSEvents for as long as something keeps reading
// the emitted files, and a restarted Electron reads all of `dist-main` — so
// treating every watch event as a fresh build restarts the app forever off a
// single `tsc` emit. Only a file written after the running Electron started is
// a new build. `.endsWith(".js")` also drops `.tsbuildinfo` and the source maps.
export function isNewerMainBuild(name, electronStartedAt, emittedAt) {
  if (typeof name !== "string" || !name.endsWith(".js")) return false;
  const mtimeMs = emittedAt(name);
  return mtimeMs !== null && mtimeMs > electronStartedAt;
}

function run(command, args) {
  return execFileSync(command, args, {
    stdio: ["ignore", "pipe", "pipe"],
  }).toString();
}

function sha256File(file) {
  return createHash("sha256").update(readFileSync(file)).digest("hex");
}

function setPlistString(plist, key, value) {
  try {
    run("/usr/bin/plutil", ["-replace", key, "-string", value, plist]);
  } catch {
    run("/usr/bin/plutil", ["-insert", key, "-string", value, plist]);
  }
}

function copyAppBundle(source, dest) {
  rmSync(dest, { recursive: true, force: true });
  mkdirSync(path.dirname(dest), { recursive: true });
  try {
    run("/bin/cp", ["-cR", source, dest]);
  } catch {
    run("/bin/cp", ["-R", source, dest]);
  }
}

function stripCopiedMetadata(appPath) {
  try {
    run("/usr/bin/xattr", ["-cr", appPath]);
  } catch {
    // A missing xattr tool must not block the wrapper; Gatekeeper still sees a local clone.
  }
}

function applyIdentity(appPath, options) {
  const plist = path.join(appPath, "Contents", "Info.plist");
  if (!existsSync(plist)) {
    throw new Error(`dev-app: ${plist} is missing`);
  }
  const resources = path.join(appPath, "Contents", "Resources");
  mkdirSync(resources, { recursive: true });
  const iconDest = path.join(resources, "icon.icns");
  copyFileSync(options.iconPath, iconDest);
  const stockIcon = path.join(resources, "electron.icns");
  if (existsSync(stockIcon)) copyFileSync(options.iconPath, stockIcon);

  // Leave CFBundleExecutable and Contents/MacOS/Electron alone. Renaming the
  // stock stub makes codesign refuse the bundle (SIP `com.apple.provenance`
  // detritus). Dock/Cmd-Tab read CFBundleDisplayName.
  setPlistString(plist, "CFBundleName", options.displayName);
  setPlistString(plist, "CFBundleDisplayName", options.displayName);
  setPlistString(plist, "CFBundleIdentifier", options.bundleId);
  setPlistString(plist, "CFBundleIconFile", "icon.icns");
  setPlistString(plist, "CFBundleShortVersionString", options.version);
  setPlistString(plist, "CFBundleVersion", options.version);
  setPlistString(plist, "NSHumanReadableCopyright", DEV_APP_COPYRIGHT);
  setPlistString(plist, "NSDesktopFolderUsageDescription", DEV_APP_FOLDER_USAGE);
  setPlistString(plist, "NSDocumentsFolderUsageDescription", DEV_APP_FOLDER_USAGE);
  setPlistString(plist, "NSDownloadsFolderUsageDescription", DEV_APP_FOLDER_USAGE);
  setPlistString(plist, "NSMicrophoneUsageDescription", DEV_APP_MICROPHONE_USAGE);
  setPlistString(plist, "NSCameraUsageDescription", DEV_APP_CAMERA_USAGE);
}

/**
 * Dock / Spotlight launch Electron with no argv, which would otherwise show
 * stock default_app.asar. Replace that with a loader that sets the isolated
 * Iface Dev profile and imports the repo's unpackaged main process.
 *
 * `hostBinary` is the Rust host `dev.mjs` just built. A Dock launch inherits no
 * environment, so the path is baked in here and exported as the main process's
 * own override. Without it the main process falls back to
 * `<checkout>/target/debug`, which is not where cargo builds when
 * `CARGO_TARGET_DIR` or a user-level cargo config moves the target directory —
 * leaving either nothing there or, the failure that says nothing, a stale
 * binary.
 */
function defaultAppMainSource(electronRoot, hostBinary = null) {
  const bakedRoot = JSON.stringify(path.resolve(electronRoot));
  const bakedHost = hostBinary === null
    ? `path.join(electronRoot, "..", "target", "debug", "interface-host")`
    : JSON.stringify(path.resolve(hostBinary));
  return `'use strict';
const { app, dialog } = require("electron");
const { existsSync, mkdirSync } = require("node:fs");
const { homedir } = require("node:os");
const path = require("node:path");
const { pathToFileURL } = require("node:url");

const electronRoot = ${bakedRoot};
const profile = process.env.INTERFACE_DEV_PROFILE
  || path.join(homedir(), "Library", "Application Support", "Iface Dev");
mkdirSync(profile, { recursive: true, mode: 0o700 });
process.env.INTERFACE_HOST_TEST_DATA_ROOT = profile;
process.env.INTERFACE_HOST_TEST_HOME = homedir();
if (process.env.INTERFACE_DEV_SUPERVISED !== "1") {
  delete process.env.VITE_DEV_SERVER_URL;
}
app.setPath("userData", profile);
app.setPath("sessionData", profile);

const main = path.join(electronRoot, "dist-main", "main", "main.js");
const host = process.env.INTERFACE_RUST_HOST || ${bakedHost};
process.env.INTERFACE_RUST_HOST = host;
if (!existsSync(main) || !existsSync(host)) {
  app.whenReady().then(() => {
    dialog.showErrorBox(
      "Interface Dev",
      "Missing a local build. From the Interface-Rust repo run:\\n\\nnpm --prefix electron run dev",
    );
    app.quit();
  });
} else {
  import(pathToFileURL(main).href).catch((error) => {
    dialog.showErrorBox(
      "Interface Dev",
      error instanceof Error ? error.message : String(error),
    );
    app.quit();
  });
}
`;
}

function packDefaultAppAsar(dir, asarPath) {
  let asarCli;
  try {
    asarCli = appBuilderLibRequire().resolve("@electron/asar/bin/asar.js");
  } catch {
    throw new Error("dev-app: electron-builder's @electron/asar is missing; "
      + "install electron dependencies first");
  }
  run(process.execPath, [asarCli, "pack", dir, asarPath]);
}

function applyDockLauncher(appPath, electronRoot, hostBinary = null) {
  const resources = path.join(appPath, "Contents", "Resources");
  const plist = path.join(appPath, "Contents", "Info.plist");
  if (!existsSync(resources) || !existsSync(plist)) {
    throw new Error(`dev-app: ${appPath} is missing Contents/Resources or Info.plist`);
  }
  const asarPath = path.join(resources, "default_app.asar");
  rmSync(asarPath, { force: true });
  rmSync(path.join(resources, "default_app.asar.unpacked"), { recursive: true, force: true });
  const dir = path.join(resources, "default_app");
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  writeFileSync(path.join(dir, "package.json"), `${JSON.stringify({
    name: "interface-dev",
    productName: DEV_APP_DISPLAY_NAME,
    main: "./main.js",
  }, null, 2)}\n`);
  writeFileSync(path.join(dir, "main.js"), defaultAppMainSource(electronRoot, hostBinary));
  // The Electron stub exits immediately if Resources/default_app.asar is missing,
  // even when an app path is passed on argv.
  packDefaultAppAsar(dir, asarPath);
  try {
    run("/usr/bin/plutil", ["-remove", "ElectronAsarIntegrity", plist]);
  } catch { /* the stock key is absent from some fixtures */ }
}

function signApp(appPath) {
  run("/usr/bin/codesign", [
    "--force", "--deep", "--sign", "-", "--timestamp=none", appPath,
  ]);
}

export function electronExecutable(appPath) {
  return path.join(appPath, "Contents", "MacOS", "Electron");
}

export function destAppPath(destDir, displayName = DEV_APP_DISPLAY_NAME) {
  return path.join(destDir, `${displayName}.app`);
}

function resolvedPath(target) {
  try {
    return realpathSync(target);
  } catch {
    return path.resolve(target);
  }
}

/** Refuse anything that could replace or nest inside the installed product. */
export function assertSafeInstallDestination(destApp) {
  const resolved = path.resolve(destApp);
  const production = resolvedPath(PRODUCTION_APP_PATH);
  if (resolved === production || resolved.startsWith(`${production}${path.sep}`)) {
    throw new Error("dev-app: refusing to replace Interface.app");
  }
  if (existsSync(resolved) && lstatSync(resolved).isSymbolicLink()) {
    const real = realpathSync(resolved);
    if (real === production || real.startsWith(`${production}${path.sep}`)) {
      throw new Error("dev-app: refusing to replace Interface.app");
    }
  }
  const expectedName = `${DEV_APP_DISPLAY_NAME}.app`;
  if (path.basename(resolved) !== expectedName) {
    throw new Error(`dev-app: install dest must be named ${expectedName}`);
  }
}

/**
 * Copy a materialized Interface Dev.app into place (default `/Applications`).
 * Never overwrites `/Applications/Interface.app`.
 */
export function installDevApp(sourceApp, destApp = DEV_APP_INSTALL_PATH) {
  assertSafeInstallDestination(destApp);
  if (!existsSync(electronExecutable(sourceApp))) {
    throw new Error(`dev-app: ${sourceApp} is not a materialized Interface Dev.app`);
  }
  copyAppBundle(sourceApp, destApp);
  const executablePath = electronExecutable(destApp);
  if (!existsSync(executablePath)) {
    throw new Error(`dev-app: installed bundle is missing ${executablePath}`);
  }
  try {
    run("/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister", [
      "-f", destApp,
    ]);
  } catch {
    // Launch Services registration is best-effort; the bundle is already on disk.
  }
  return { appPath: destApp, executablePath };
}

export function bundleStampPath(appPath) {
  return path.join(appPath, BUNDLE_STAMP);
}

/**
 * `/Applications/Interface Dev.app` is shared by every checkout, while the
 * materialize stamp is this checkout's own. Another checkout may have installed
 * its bundle, with its own baked electronRoot and host, since this one did; a
 * Dock launch after the supervisor quits would then run that checkout.
 */
export function installedDevAppMatches(sourceApp, destApp = DEV_APP_INSTALL_PATH) {
  try {
    return readFileSync(bundleStampPath(sourceApp)).equals(readFileSync(bundleStampPath(destApp)));
  } catch {
    return false;
  }
}

function stampMatches(stampPath, stamp) {
  try {
    return JSON.stringify(JSON.parse(readFileSync(stampPath, "utf8")))
      === JSON.stringify(stamp);
  } catch {
    return false;
  }
}

/**
 * Clone stock Electron.app into a named Interface Dev.app so the Dock, Cmd-Tab
 * and TCC dialogs match the installed product instead of "Electron". Helpers
 * stay stock: Electron finds them by path, and renaming those Mach-Os is the
 * packaged-app pipeline's job.
 */
export function materializeDevApp(options) {
  const displayName = options.displayName ?? DEV_APP_DISPLAY_NAME;
  const bundleId = options.bundleId ?? DEV_APP_BUNDLE_ID;
  const sign = options.sign !== false;
  const electronRoot = options.electronRoot;
  if (typeof electronRoot !== "string" || electronRoot.trim().length === 0) {
    throw new Error("dev-app: electronRoot is required");
  }
  const hostBinary = options.hostBinary ?? null;
  const sourceApp = path.join(options.electronDist, "Electron.app");
  const versionFile = path.join(options.electronDist, "version");
  if (!existsSync(sourceApp)) {
    throw new Error(`dev-app: ${sourceApp} is missing; install electron first`);
  }
  if (!existsSync(options.iconPath)) {
    throw new Error(`dev-app: ${options.iconPath} is missing; run npm run build:icon`);
  }
  if (!existsSync(versionFile)) {
    throw new Error(`dev-app: ${versionFile} is missing`);
  }

  const destDir = options.destDir;
  const appPath = destAppPath(destDir, displayName);
  const executablePath = electronExecutable(appPath);
  const stampPath = path.join(destDir, STAMP_NAME);
  // stampMatches compares JSON text, so this key order is part of the stamp:
  // reordering it re-materializes every checkout's bundle once.
  const stamp = {
    bundleId,
    displayName,
    electronVersion: readFileSync(versionFile, "utf8").trim(),
    iconSha256: sha256File(options.iconPath),
    electronRoot: path.resolve(electronRoot),
    // The launcher bakes this path in, so a moved build directory has to
    // re-materialize the bundle rather than reuse one pointing at the old one.
    hostBinary: hostBinary === null ? null : path.resolve(hostBinary),
    launcherRevision: DEV_APP_LAUNCHER_REVISION,
    signed: sign,
    sourceApp,
    version: options.version,
  };

  if (existsSync(executablePath) && stampMatches(stampPath, stamp)
      && stampMatches(bundleStampPath(appPath), stamp)) {
    return { appPath, executablePath, reused: true };
  }

  mkdirSync(destDir, { recursive: true });
  copyAppBundle(sourceApp, appPath);
  applyIdentity(appPath, {
    bundleId,
    displayName,
    iconPath: options.iconPath,
    version: options.version,
  });
  applyDockLauncher(appPath, electronRoot, hostBinary);
  if (!existsSync(executablePath)) {
    throw new Error(`dev-app: cloned bundle is missing ${executablePath}`);
  }
  stripCopiedMetadata(appPath);
  const stampText = `${JSON.stringify(stamp, null, 2)}\n`;
  writeFileSync(bundleStampPath(appPath), stampText);
  if (sign) signApp(appPath);
  writeFileSync(stampPath, stampText);
  return { appPath, executablePath, reused: false };
}
