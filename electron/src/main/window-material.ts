import type { BrowserWindow } from "electron";
import type { SurfaceTranslucency } from "../shared/appearance.js";

// Chromium chooses the native compositor's alpha capability at construction.
// Adding vibrancy later to an opaque widget can retain dismissed content behind
// transparent panes. Keep one material backing for the window's lifetime; the
// native background and HTML surfaces mask it completely at zero.
export const MAC_WINDOW_MATERIAL = {
  vibrancy: "under-window" as const,
  visualEffectState: "followWindow" as const,
};
/** Translate the Rust-owned material preference into native window rendering.
 * The HTML background masks the material independently for each surface. */
export function applyWindowMaterial(
  window: Pick<BrowserWindow, "setBackgroundColor">,
  solidColor: string,
  surfaces: SurfaceTranslucency,
  platform: string = process.platform,
): void {
  const glass = platform === "darwin" && (surfaces.terminal > 0 || surfaces.sidebar > 0);
  window.setBackgroundColor(glass ? "#00000000" : solidColor);
}
