"use strict";

const { readFile } = require("node:fs/promises");
const path = require("node:path");

const SENTINEL = "dL7pKGdnNz796PbbjQWNKmHXBZaB9tsX";
const WIRE_VERSION_V1 = 1;
const STATE_DISABLED = 0x30;
const STATE_ENABLED = 0x31;
const STATE_REMOVED = 0x72;
const STATE_INHERIT = 0x90;

const V1_FUSE_ORDER = [
  "runAsNode",
  "enableCookieEncryption",
  "enableNodeOptionsEnvironmentVariable",
  "enableNodeCliInspectArguments",
  "enableEmbeddedAsarIntegrityValidation",
  "onlyLoadAppFromAsar",
  "loadBrowserProcessSpecificV8Snapshot",
  "grantFileProtocolExtraPrivileges",
];

const REQUIRED_FUSES = {
  runAsNode: false,
  enableCookieEncryption: true,
  enableNodeOptionsEnvironmentVariable: false,
  enableNodeCliInspectArguments: false,
  enableEmbeddedAsarIntegrityValidation: true,
  onlyLoadAppFromAsar: true,
};

function fuseBinaryPath(appPath) {
  if (typeof appPath !== "string" || !appPath.endsWith(".app")) {
    throw new TypeError("Electron fuse verification expects a macOS app bundle.");
  }
  return path.join(
    appPath, "Contents", "Frameworks", "Electron Framework.framework", "Electron Framework",
  );
}

function readWireAt(binary, sentinelOffset) {
  const position = sentinelOffset + SENTINEL.length;
  if (position + 1 >= binary.length) {
    throw new Error("Electron fuse wire is truncated: no version or length byte.");
  }
  const version = binary[position];
  if (version !== WIRE_VERSION_V1) {
    throw new Error(
      `Electron fuse wire version ${version} is not the V1 layout this check `
      + "understands. Re-derive the fuse offsets before trusting this build.",
    );
  }
  const length = binary[position + 1];
  const end = position + 2 + length;
  if (end > binary.length) {
    throw new Error(`Electron fuse wire claims ${length} fuses but the binary ends first.`);
  }
  return Array.from(binary.subarray(position + 2, end));
}

function readFuseWires(binary) {
  const first = binary.indexOf(SENTINEL);
  if (first === -1) {
    throw new Error(
      "No Electron fuse sentinel in the packaged binary. Fuses require "
      + "Electron 12 or newer; this bundle cannot be verified.",
    );
  }
  const last = binary.lastIndexOf(SENTINEL);
  const offsets = first === last ? [first] : [first, last];
  return offsets.map(offset => readWireAt(binary, offset));
}

function describeState(state) {
  if (state === STATE_ENABLED) return "enabled";
  if (state === STATE_DISABLED) return "disabled";
  if (state === STATE_REMOVED) return "marked removed by this Electron";
  if (state === STATE_INHERIT) return "left unconfigured (never flipped)";
  return `an unrecognized state byte 0x${state.toString(16).padStart(2, "0")}`;
}

function assertFuses(wire, required = REQUIRED_FUSES) {
  const failures = [];
  for (const [name, expected] of Object.entries(required)) {
    const index = V1_FUSE_ORDER.indexOf(name);
    if (index === -1) {
      throw new Error(`Unknown Electron fuse "${name}" in the required fuse table.`);
    }
    if (index >= wire.length) {
      failures.push(`${name} is fuse ${index} but this Electron's wire holds only ${wire.length}`);
      continue;
    }
    const state = wire[index];
    if (state === (expected ? STATE_ENABLED : STATE_DISABLED)) continue;
    failures.push(
      `${name} must be ${expected ? "enabled" : "disabled"} but is ${describeState(state)}`,
    );
  }
  if (failures.length > 0) {
    throw new Error(
      `Packaged Electron fuses are unsafe:\n  - ${failures.join("\n  - ")}\n`
      + "The bundle was not shipped. Check `electronFuses` in electron-builder.yml.",
    );
  }
}

function packagedAppPath(context) {
  const platformName = context?.electronPlatformName;
  if (platformName !== "darwin" && platformName !== "mas") {
    throw new Error(
      `Electron fuse verification only knows macOS bundles, not "${platformName}". `
      + "Extend scripts/verify-fuses.cjs before packaging for that platform.",
    );
  }
  const productFilename = context?.packager?.appInfo?.productFilename;
  if (typeof productFilename !== "string" || productFilename.length === 0) {
    throw new TypeError("Electron fuse verification could not resolve the product filename.");
  }
  return path.join(context.appOutDir, `${productFilename}.app`);
}

async function verifyFuses(context) {
  await verifyPackagedApp(packagedAppPath(context));
}

async function verifyPackagedApp(appPath) {
  const binary = await readFile(fuseBinaryPath(appPath));
  const wires = readFuseWires(binary);
  for (const wire of wires) assertFuses(wire);
  const slices = wires.length === 1 ? "" : ` in all ${wires.length} architecture slices`;
  process.stdout.write(`  • verified Electron fuses in the signed bundle${slices}\n`);
}

module.exports = verifyFuses;
module.exports.SENTINEL = SENTINEL;
module.exports.REQUIRED_FUSES = REQUIRED_FUSES;
module.exports.V1_FUSE_ORDER = V1_FUSE_ORDER;
module.exports.assertFuses = assertFuses;
module.exports.fuseBinaryPath = fuseBinaryPath;
module.exports.packagedAppPath = packagedAppPath;
module.exports.readFuseWires = readFuseWires;
module.exports.verifyPackagedApp = verifyPackagedApp;

if (require.main === module) {
  verifyPackagedApp(process.argv[2]).catch(error => {
    process.stderr.write(`${error.message}\n`);
    process.exitCode = 1;
  });
}
