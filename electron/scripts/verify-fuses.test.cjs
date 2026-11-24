"use strict";

const assert = require("node:assert/strict");
const { existsSync, readFileSync } = require("node:fs");
const { mkdir, mkdtemp, rm, writeFile } = require("node:fs/promises");
const { tmpdir } = require("node:os");
const path = require("node:path");
const { afterEach, test } = require("node:test");
const { appBuilderLibRequire } = require("./builder-require.cjs");
const verifyFuses = require("./verify-fuses.cjs");

const {
  REQUIRED_FUSES, SENTINEL, V1_FUSE_ORDER,
  assertFuses, fuseBinaryPath, packagedAppPath, readFuseWires, verifyPackagedApp,
} = verifyFuses;
const DISABLED = 0x30;
const ENABLED = 0x31;
const REMOVED = 0x72;
const INHERIT = 0x90;
const REPO_ROOT = path.resolve(__dirname, "..");
const BUILDER_CONFIG = path.join(REPO_ROOT, "electron-builder.yml");
const STOCK_ELECTRON = path.join(
  REPO_ROOT, "node_modules", "electron", "dist", "Electron.app",
  "Contents", "Frameworks", "Electron Framework.framework", "Electron Framework",
);
const roots = [];

afterEach(async () => {
  await Promise.all(roots.splice(0).map(root => rm(root, { recursive: true, force: true })));
});

function readDeclaredFuses(source) {
  const lines = source.split("\n");
  const start = lines.indexOf("electronFuses:");
  assert.notEqual(start, -1, "electron-builder.yml declares no electronFuses block");
  const declared = {};
  for (let index = start + 1; index < lines.length; index += 1) {
    const line = lines[index];
    if (line.trim() === "" || !line.startsWith("  ")) break;
    if (line.trim().startsWith("#")) continue;
    const entry = /^ {2}([A-Za-z][A-Za-z0-9]*): (true|false)$/.exec(line);
    assert.notEqual(entry, null, `electronFuses entry is not a plain boolean: ${line}`);
    declared[entry[1]] = entry[2] === "true";
  }
  return declared;
}

function fakeBinary(states, { version = 1, sentinels = 1, length = states.length } = {}) {
  const padding = Buffer.alloc(24, 0x41);
  const block = Buffer.concat([
    Buffer.from(SENTINEL, "utf8"), Buffer.from([version, length]), Buffer.from(states),
  ]);
  const parts = [padding];
  for (let index = 0; index < sentinels; index += 1) parts.push(block, padding);
  return Buffer.concat(parts);
}

function safeStates() {
  return V1_FUSE_ORDER.map(name => (
    name in REQUIRED_FUSES ? (REQUIRED_FUSES[name] ? ENABLED : DISABLED) : DISABLED
  ));
}

async function fakeBundle(binary) {
  const root = await mkdtemp(path.join(tmpdir(), "interface-rust-fuses-test-"));
  roots.push(root);
  const appOutDir = path.join(root, "mac-arm64");
  const appPath = path.join(appOutDir, "Interface.app");
  const binaryPath = fuseBinaryPath(appPath);
  await mkdir(path.dirname(binaryPath), { recursive: true });
  await writeFile(binaryPath, binary);
  return { appOutDir, appPath };
}

function afterSignContext(appOutDir, overrides = {}) {
  return {
    appOutDir,
    electronPlatformName: "darwin",
    packager: { appInfo: { productFilename: "Interface" } },
    ...overrides,
  };
}

test("builder config declares exactly the fuses enforced after signing", () => {
  const source = readFileSync(BUILDER_CONFIG, "utf8");
  assert.deepEqual(readDeclaredFuses(source), REQUIRED_FUSES);
  assert.match(source, /^afterSign: scripts\/verify-fuses\.cjs$/m);
  assert.equal(REQUIRED_FUSES.enableCookieEncryption, true);
  assert.equal(REQUIRED_FUSES.enableEmbeddedAsarIntegrityValidation, true);
  assert.match(source, /^ {2}enableCookieEncryption: true$/m);
  assert.match(source, /^ {2}enableEmbeddedAsarIntegrityValidation: true$/m);
});

test("installed electron-builder accepts every declared fuse key", () => {
  const scheme = appBuilderLibRequire()("./scheme.json");
  assert.ok(scheme.properties.electronFuses);
  assert.equal(scheme.additionalProperties, false);
  const fuseSchema = scheme.definitions.FuseOptionsV1;
  assert.equal(fuseSchema.additionalProperties, false);
  for (const name of Object.keys(REQUIRED_FUSES)) {
    assert.equal(fuseSchema.properties[name]?.type, "boolean");
  }
});

test("stock Electron is unsafe and the exact fused wire passes", {
  skip: existsSync(STOCK_ELECTRON) ? false : "electron devDependency is not installed",
}, () => {
  const wires = readFuseWires(readFileSync(STOCK_ELECTRON));
  assert.equal(wires[0][V1_FUSE_ORDER.indexOf("runAsNode")], ENABLED);
  assert.throws(() => assertFuses(wires[0]), /runAsNode must be disabled/);
  const fused = [...wires[0]];
  for (const [name, expected] of Object.entries(REQUIRED_FUSES)) {
    fused[V1_FUSE_ORDER.indexOf(name)] = expected ? ENABLED : DISABLED;
  }
  assert.doesNotThrow(() => assertFuses(fused));
});

test("unreadable, ambiguous, and unflipped wire states fail closed", () => {
  assert.throws(() => readFuseWires(Buffer.alloc(64, 0x41)), /No Electron fuse sentinel/);
  assert.throws(
    () => readFuseWires(fakeBinary(safeStates(), { version: 2 })),
    /fuse wire version 2 is not the V1 layout/,
  );
  assert.throws(
    () => readFuseWires(fakeBinary(safeStates(), { length: 99 })),
    /claims 99 fuses but the binary ends first/,
  );
  for (const [state, expected] of [
    [REMOVED, /marked removed by this Electron/],
    [INHERIT, /left unconfigured \(never flipped\)/],
    [0x7f, /unrecognized state byte 0x7f/],
  ]) {
    const states = safeStates();
    states[V1_FUSE_ORDER.indexOf("runAsNode")] = state;
    assert.throws(() => assertFuses(states), expected);
  }
});

test("every architecture slice and the final signed bundle are checked", async () => {
  const safe = fakeBinary(safeStates());
  const unsafeStates = safeStates();
  unsafeStates[V1_FUSE_ORDER.indexOf("runAsNode")] = ENABLED;
  const mixed = Buffer.concat([safe, fakeBinary(unsafeStates)]);
  const wires = readFuseWires(mixed);
  assert.equal(wires.length, 2);
  assert.throws(() => {
    for (const wire of wires) assertFuses(wire);
  }, /runAsNode must be disabled/);

  const fused = await fakeBundle(fakeBinary(safeStates()));
  await assert.doesNotReject(verifyFuses(afterSignContext(fused.appOutDir)));
  await assert.doesNotReject(verifyPackagedApp(fused.appPath));
  await assert.rejects(
    verifyFuses(afterSignContext(fused.appOutDir, { electronPlatformName: "win32" })),
    /only knows macOS bundles/,
  );
  assert.equal(packagedAppPath(afterSignContext("/out")), "/out/Interface.app");
});
