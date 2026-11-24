import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

// Match the desktop icon's established rasterizer; no additional packages.
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const chrome = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "interface-phone-icon-"));
const catalog = path.join(root, "Assets.xcassets");
const appIcon = path.join(catalog, "AppIcon.appiconset");
const brand = path.join(catalog, "InterfaceLogo.imageset");
const metadata = { author: "xcode", version: 1 };
const json = (file, value) => fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`);
try {
  const svg = fs.readFileSync(path.join(root, "Artwork/icon.svg"), "utf8");
  const html = path.join(temporary, "icon.html");
  const master = path.join(temporary, "icon.png");
  fs.writeFileSync(html, `<!doctype html><meta charset="utf-8"><style>html,body{margin:0}svg{display:block;width:1024px;height:1024px}</style>${svg}`);
  try {
    execFileSync(chrome, [
    "--headless", "--disable-gpu", "--hide-scrollbars", "--force-device-scale-factor=1",
    "--window-size=1024,1024", "--no-first-run", "--no-default-browser-check",
    `--screenshot=${master}`, `--user-data-dir=${path.join(temporary, "chrome-profile")}`,
    `file://${html}`,
    ], { timeout: 30_000, killSignal: "SIGKILL", stdio: "pipe" });
  } catch (error) {
    // Some Chrome versions finish the screenshot but keep helper processes open.
    if (!fs.existsSync(master)) throw error;
  }
  const properties = execFileSync("/usr/bin/sips", ["-g", "pixelWidth", "-g", "pixelHeight", "-g", "hasAlpha", master], { encoding: "utf8" });
  if (!/pixelWidth: 1024/.test(properties) || !/pixelHeight: 1024/.test(properties) || !/hasAlpha: no/.test(properties)) {
    throw new Error(`Expected an opaque 1024px icon: ${properties}`);
  }
  fs.mkdirSync(appIcon, { recursive: true });
  fs.mkdirSync(brand, { recursive: true });
  fs.copyFileSync(master, path.join(appIcon, "AppIcon.png"));
  json(path.join(catalog, "Contents.json"), { info: metadata });
  json(path.join(appIcon, "Contents.json"), {
    images: [{ filename: "AppIcon.png", idiom: "universal", platform: "ios", size: "1024x1024" }], info: metadata,
  });
  const images = [1, 2, 3].map(scale => {
    const filename = `InterfaceLogo${scale === 1 ? "" : `@${scale}x`}.png`;
    execFileSync("/usr/bin/sips", ["-z", String(72 * scale), String(72 * scale), master, "--out", path.join(brand, filename)], { stdio: "pipe" });
    return { filename, idiom: "universal", scale: `${scale}x` };
  });
  json(path.join(brand, "Contents.json"), { images, info: metadata });
  // Safari uses the same artwork with a baked mask; iOS owns its native mask.
  fs.writeFileSync(path.resolve(root, "../../electron/remote-public/icon.svg"), svg.replace('height="1024" fill="url(#face)"', 'height="1024" rx="224" fill="url(#face)"'));
  // Add to Home Screen needs a PNG: the opaque square, masked by iOS like the app.
  execFileSync("/usr/bin/sips", ["-z", "180", "180", master, "--out", path.resolve(root, "../../electron/remote-public/apple-touch-icon.png")], { stdio: "pipe" });
  console.log("Generated opaque iOS AppIcon, native logo sizes, and shared phone web icons.");
} finally {
  fs.rmSync(temporary, { recursive: true, force: true });
}
