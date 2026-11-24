import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import test from "node:test";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { resolveConfig } from "vite";
import { appBuilderLibRequire } from "./builder-require.cjs";

const { getFileMatchers } = appBuilderLibRequire()("./out/fileMatcher.js");

const packageJsonUrl = new URL("../package.json", import.meta.url);
const builderUrl = new URL("../electron-builder.yml", import.meta.url);
const distMainUrl = new URL("../dist-main/", import.meta.url);
const indexHtmlUrl = new URL("../index.html", import.meta.url);
const distIndexHtmlUrl = new URL("../dist/index.html", import.meta.url);
const remoteHtmlUrl = new URL("../remote.html", import.meta.url);
const distRemoteHtmlUrl = new URL("../dist-remote/remote.html", import.meta.url);

test("release commands are Rust-owned and smoke precedes local feed staging", async () => {
  const packageJson = JSON.parse(await readFile(packageJsonUrl, "utf8"));
  assert.equal(packageJson.scripts.check, "node scripts/check.mjs");
  // The renderer pin contract is worth nothing unless the release suite runs it.
  assert.match(packageJson.scripts["test:release"], /\bscripts\/smoke-pins\.test\.mjs\b/);
  // Likewise the Rust/TypeScript contract parity check.
  assert.match(packageJson.scripts["test:release"], /\bscripts\/contract-parity\.test\.mjs\b/);
  assert.equal(packageJson.scripts["package:mac"],
    "cargo run --manifest-path ../Cargo.toml --package xtask -- package-mac");
  assert.equal(packageJson.scripts["package:smoke"],
    "cargo run --manifest-path ../Cargo.toml --package xtask -- package-mac --smoke");
  assert.equal(packageJson.scripts["stage:update"],
    "cargo run --manifest-path ../Cargo.toml --package xtask -- stage-update --source ../release");
  // Scoping and promotion are xtask's too: npm only forwards to them.
  assert.equal(packageJson.scripts["check:changed"],
    "cargo run --manifest-path ../Cargo.toml --package xtask -- check --changed");
  assert.equal(packageJson.scripts.promote,
    "cargo run --manifest-path ../Cargo.toml --package xtask -- promote");
  assert.equal(packageJson.scripts["release:local"],
    "npm run package:smoke && npm run stage:update");
  assert.equal(packageJson.scripts["release:local:replace"],
    "npm run package:smoke -- --allow-same-version && "
      + "npm run stage:update -- --allow-same-version");
});

test("packaged resources contain exactly the three first-party helper executables", async () => {
  const config = await readFile(builderUrl, "utf8");
  // xtask exports Cargo's actual target relative to the packaging project.
  const targetMacro = String.raw`\$\{env\.INTERFACE_TARGET_DIR\}`;
  const resources = [...config.matchAll(
    new RegExp(`^  - from: (${targetMacro}/release/[^\\n]+)\\n    to: (bin/[^\\n]+)$`, "gm"),
  )].map(match => ({ from: match[1], to: match[2] }));
  assert.deepEqual(resources, [
    { from: "${env.INTERFACE_TARGET_DIR}/release/interface-host", to: "bin/interface-host" },
    { from: "${env.INTERFACE_TARGET_DIR}/release/iface", to: "bin/iface" },
    { from: "${env.INTERFACE_TARGET_DIR}/release/interface-voice", to: "bin/interface-voice" },
  ]);
  assert.doesNotMatch(config, new RegExp(`^  - from: ${targetMacro}/release/?$`, "m"));
  // Hardcoding `../target` ignores an explicitly configured Cargo target.
  assert.doesNotMatch(config, /^ *- from: \.\.\/target\//m);
  // xtask has to be the one that sets it; nothing else runs before the build.
  const packageMac = await readFile(new URL("../../xtask/src/package.rs", import.meta.url), "utf8");
  assert.match(packageMac, /INTERFACE_TARGET_DIR/);
  assert.match(config, /^asar: true$/m);
  assert.match(config, /^afterSign: scripts\/verify-fuses\.cjs$/m);
  assert.match(config, /^  sign: scripts\/sign-mac\.cjs$/m);
  // The helper records audio, so the bundle has to declare why.
  assert.match(config, /^    NSMicrophoneUsageDescription: Interface uses the microphone for the voice assistant\.$/m);
  // Phone access announces the Mac over Bonjour; without this text macOS asks
  // for the local network with no reason given, or refuses without asking.
  assert.match(config, /^    NSLocalNetworkUsageDescription: Interface uses your local network so the iPhone app can reach this Mac when you turn on phone access\.$/m);
  assert.match(config, /^    NSBonjourServices:\n      - _interface\._tcp$/m);
  // A Browser page may ask for the camera (the owner allows it per site).
  assert.match(config, /^    NSCameraUsageDescription: Interface lets a web page in its Browser use the camera when you allow it\.$/m);
});

// `tsc` never deletes what it once emitted, so anything dist-main ever held —
// a renamed module, a `.test.js` from before the exclude — shipped in the
// signed asar until the build started from an empty directory.
test("the packaged main build is emitted into a cleaned dist-main", async () => {
  const packageJson = JSON.parse(await readFile(packageJsonUrl, "utf8"));
  assert.equal(packageJson.scripts.build,
    "vite build && npm run build:remote && "
      + "node -e \"require('node:fs').rmSync('dist-main',{recursive:true,force:true})\" && "
      + "tsc -p tsconfig.main.json");
  const config = await readFile(builderUrl, "utf8");
  assert.match(config, /^  - '!dist-main\/\*\*\/\*\.tsbuildinfo'$/m);
  const emitted = await readdir(distMainUrl, { recursive: true }).catch(() => []);
  assert.deepEqual(emitted.filter(name => name.endsWith(".js") && name.includes(".test")), []);
});

// Vite bundles everything the renderer imports into dist/, while
// electron-builder copies every `dependencies` package into app.asar, and a
// native one into app.asar.unpacked, on every install and update. xterm,
// react and pdfjs-dist (with its optional 28 MB Skia canvas) once shipped
// there beside the bundles that already contained them.
test("only packages the main process imports ship as dependencies", async () => {
  const packageJson = JSON.parse(await readFile(packageJsonUrl, "utf8"));
  const shipped = Object.keys(packageJson.dependencies ?? {}).sort();
  const imported = new Set();
  const specifiers = /(?:\bfrom\s*|\bimport\s*\(\s*|\brequire\s*\(\s*)["']([^"'./][^"']*)["']/gu;
  for (const directory of ["../src/main/", "../src/shared/"]) {
    const root = new URL(directory, import.meta.url);
    for (const name of await readdir(root, { recursive: true })) {
      if (!/\.[cm]?ts$/u.test(name) || /\.test\.[cm]?ts$|\.d\.ts$/u.test(name)) continue;
      for (const [, specifier] of (await readFile(new URL(name, root), "utf8")).matchAll(specifiers)) {
        if (specifier.startsWith("node:") || specifier === "electron") continue;
        const parts = specifier.split("/");
        imported.add(specifier.startsWith("@") ? parts.slice(0, 2).join("/") : parts[0]);
      }
    }
  }
  assert.deepEqual(shipped, [...imported].sort());
  for (const renderer of [
    "@xterm/addon-fit", "@xterm/addon-search", "@xterm/addon-webgl", "@xterm/xterm",
    "pdfjs-dist", "react", "react-dom",
  ]) {
    assert.ok(packageJson.devDependencies?.[renderer], `${renderer} is bundled by Vite`);
  }
});

test("the check runs every suite and builds before the renderer lanes", async () => {
  const { CHECK_PARTS } = await import("./check-parts.mjs");
  const parts = Object.fromEntries(CHECK_PARTS.map(([name, commands]) => [name, commands.map(([label]) => label)]));
  assert.deepEqual(parts, {
    unit: ["npm test"],
    types: ["tsc -p tsconfig.json --noEmit"],
    // The lanes load dist-main, which the build removes and rewrites; the
    // packaging step reuses what the build leaves.
    renderer: ["npm run build", "node scripts/test-renderer-dom.mjs"],
  });
});

test("a reused gate stamp still builds Electron in the packaging checkout", async () => {
  const source = await readFile(new URL("../../xtask/src/package.rs", import.meta.url), "utf8");
  const reusedGate = source.match(/Some\(reason\) => \{([\s\S]*?)\n {8}\}/u)?.[1];
  assert.ok(reusedGate, "the reused-gate branch must prepare local build output");
  assert.match(reusedGate, /run_command\("npm", \["run", "build"\], &electron\)\?;/u);
  // The packaging checkout's own cargo target is resolved once and reused.
  assert.match(source, /let cargo_target = cargo_target_dir\(&root\)\?;/u);
  assert.match(source, /relative_resource_dir\(&electron, &cargo_target\)\?/u);
});

test("electron-builder resolves target macros relative to the packaging checkout", async () => {
  const source = await readFile(builderUrl, "utf8");
  const extraResources = [...source.matchAll(/^  - from: (.+)\n    to: (bin\/[^\n]+)$/gm)]
    .map(([, from, to]) => ({ from, to }));
  const project = "/projects/release checkout/electron";
  const destination = "/private/tmp/package/Interface.app/Contents/Resources";
  for (const target of ["../target", "../../shared build/cache", "."]) {
    const matchers = getFileMatchers({ extraResources }, "extraResources", destination, {
      defaultSrc: project,
      globalOutDir: "/private/tmp/package",
      customBuildOptions: {},
      macroExpander: value => value.replaceAll("${env.INTERFACE_TARGET_DIR}", target),
    });
    assert.equal(matchers.length, 3);
    for (const [index, name] of ["interface-host", "iface", "interface-voice"].entries()) {
      assert.equal(path.resolve(matchers[index].from), path.resolve(project, target, "release", name));
      assert.equal(matchers[index].to, path.join(destination, "bin", name));
    }
  }
});

function extractMetaCsp(html) {
  const match = /<meta\s+http-equiv="Content-Security-Policy"\s+content="([^"]+)"/u.exec(html);
  assert.ok(match, "missing Content-Security-Policy meta tag");
  return match[1];
}

test("built dist/index.html enforces production CSP with no unsafe-inline script-src or dev WebSockets", async () => {
  const projectRoot = fileURLToPath(new URL("..", import.meta.url));
  const rawIndex = await readFile(indexHtmlUrl, "utf8");
  const buildConfig = await resolveConfig({ root: projectRoot, logLevel: "silent" }, "build", "production");
  const plugin = buildConfig.plugins.find(candidate => candidate.name === "interface-production-csp");
  assert.ok(plugin, "vite build must register interface-production-csp");
  const handler = typeof plugin.transformIndexHtml === "function"
    ? plugin.transformIndexHtml
    : plugin.transformIndexHtml?.handler;
  assert.equal(typeof handler, "function");
  const transformed = await handler(rawIndex, { path: "/index.html", filename: path.join(projectRoot, "index.html") });

  const devConfig = await resolveConfig({ root: projectRoot, logLevel: "silent" }, "serve", "development");
  assert.equal(devConfig.plugins.find(candidate => candidate.name === "interface-production-csp"), undefined);

  const checkProductionHtml = (html, label) => {
    const csp = extractMetaCsp(html);
    const directives = csp.split(";").map(entry => entry.trim()).filter(Boolean);
    const scriptSrc = directives.find(entry => entry.startsWith("script-src "));
    const connectSrc = directives.find(entry => entry.startsWith("connect-src "));
    assert.equal(scriptSrc, "script-src 'self'", `${label}: script-src must have no 'unsafe-inline'`);
    assert.equal(connectSrc, "connect-src 'self'", `${label}: connect-src must have no dev WebSockets`);
    assert.doesNotMatch(csp, /script-src[^;]*'unsafe-inline'/u);
    assert.doesNotMatch(csp, /ws:\/\/localhost:\*|ws:\/\/127\.0\.0\.1:\*/u);
  };

  checkProductionHtml(transformed, "vite transformIndexHtml");
  const builtIndex = await readFile(distIndexHtmlUrl, "utf8").catch(() => null);
  if (builtIndex !== null) {
    checkProductionHtml(builtIndex, "dist/index.html");
  }

  for (const [label, html] of [
    ["remote.html", await readFile(remoteHtmlUrl, "utf8")],
    ["dist-remote/remote.html", await readFile(distRemoteHtmlUrl, "utf8").catch(() => null)],
  ]) {
    if (html === null) continue;
    // NativeSessionTransport.swift injects its offline WebKit CSP by matching exact `<head>`.
    assert.match(html, /<head>/u, `${label}: must preserve exact <head> tag`);
    const remoteCsp = extractMetaCsp(html);
    assert.equal(
      remoteCsp,
      "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self' ws: wss:; img-src 'self' data:; font-src 'self' data:; manifest-src 'self'; base-uri 'none'; form-action 'self'",
      label,
    );
  }
});
