import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const chrome = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const slices = [
  ["16x16", 16], ["16x16@2x", 32],
  ["32x32", 32], ["32x32@2x", 64],
  ["128x128", 128], ["128x128@2x", 256],
  ["256x256", 256], ["256x256@2x", 512],
  ["512x512", 512], ["512x512@2x", 1024],
];
const icons = [
  { svg: "icon.svg", icns: "icon.icns" },
  // The Dev SVG carries a band-free `.small` drawing for the slices at or below
  // SMALL_SLICE, where the DEV footer is only an unreadable strip.
  { svg: "icon-dev.svg", icns: "icon-dev.icns", small: true },
];
const SMALL_SLICE = 64;
const SMALL_CSS = ".large{display:none!important}.small{display:inline!important}";

function run(command, args, options = {}) {
  return execFileSync(command, args, {
    stdio: ["ignore", "pipe", "pipe"],
    ...options,
  }).toString();
}

function dimensions(file) {
  const output = run("/usr/bin/sips", ["-g", "pixelWidth", "-g", "pixelHeight", file]);
  return {
    width: Number(/pixelWidth:\s*(\d+)/.exec(output)?.[1]),
    height: Number(/pixelHeight:\s*(\d+)/.exec(output)?.[1]),
  };
}

// One rasterizer, always. sips draws these SVGs differently from Chrome, and
// the .icns bytes are hashed into dev-app.mjs's iconSha256 stamp — a silent
// fallback would rebuild every Dev bundle with an icon nobody chose. A machine
// without Chrome must fail here instead.
function rasterize(temporary, svgPath, master, extraCss = "") {
  if (!fs.existsSync(chrome)) {
    throw new Error(`icon rasterizer needs ${chrome}; install Google Chrome and re-run`);
  }
  const html = path.join(temporary, `${path.basename(master, ".png")}.html`);
  fs.writeFileSync(html, `<!doctype html><meta charset="utf-8">
<style>html,body{margin:0;background:transparent}svg{display:block;width:1024px;height:1024px}${extraCss}</style>
${fs.readFileSync(svgPath, "utf8")}`);
  try {
    run(chrome, [
      "--headless", "--disable-gpu", "--hide-scrollbars",
      "--force-device-scale-factor=1", "--default-background-color=00000000",
      "--window-size=1024,1024", "--no-first-run", "--no-default-browser-check",
      `--screenshot=${master}`,
      `--user-data-dir=${path.join(temporary, `chrome-profile-${path.basename(master, ".png")}`)}`,
      `file://${html}`,
    ], { timeout: 30_000, killSignal: "SIGKILL" });
  } catch (error) {
    if (!fs.existsSync(master)) throw error;
  }
  const size = dimensions(master);
  if (size.width !== 1024 || size.height !== 1024) {
    throw new Error(`icon rasterizer produced ${String(size.width)}x${String(size.height)}`);
  }
}

function writeIcon(svgName, icnsName, small) {
  const svgPath = path.join(root, "build", svgName);
  const icnsPath = path.join(root, "build", icnsName);
  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "interface-rust-icon-"));
  try {
    const master = path.join(temporary, "icon-1024.png");
    rasterize(temporary, svgPath, master);
    const smallMaster = small ? path.join(temporary, "icon-small-1024.png") : master;
    if (small) rasterize(temporary, svgPath, smallMaster, SMALL_CSS);
    const iconset = path.join(temporary, "icon.iconset");
    fs.mkdirSync(iconset);
    for (const [suffix, size] of slices) {
      run("/usr/bin/sips", [
        "-z", String(size), String(size), size <= SMALL_SLICE ? smallMaster : master,
        "--out", path.join(iconset, `icon_${suffix}.png`),
      ]);
    }
    run("/usr/bin/iconutil", ["-c", "icns", iconset, "-o", icnsPath]);
    const verification = path.join(temporary, "verify.iconset");
    run("/usr/bin/iconutil", ["-c", "iconset", icnsPath, "-o", verification]);
    const entries = fs.readdirSync(verification).filter(name => name.endsWith(".png"));
    if (entries.length < slices.length) {
      throw new Error(`icon container has only ${String(entries.length)} raster entries`);
    }
    console.log(`wrote ${path.relative(root, icnsPath)} (${String(entries.length)} sizes)`);
  } finally {
    fs.rmSync(temporary, { recursive: true, force: true });
  }
}

for (const icon of icons) {
  writeIcon(icon.svg, icon.icns, icon.small === true);
}
