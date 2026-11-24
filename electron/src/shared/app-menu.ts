/**
 * What the application menu asks of the renderer. The menu lives in main
 * (main/app-menu.ts); Settings… opens the renderer's own Settings page, Close
 * Pane closes the selected pane, and a Help item that failed says so (a copy
 * that worked is silent, as copying is everywhere on macOS). A main-only
 * channel: the Rust host never sees it, and nothing is persisted.
 */

export const MENU_COMMAND_CHANNEL = "iface:menuCommand";

export type MenuCommandV1 =
  | { readonly version: 1; readonly command: "settings" }
  | { readonly version: 1; readonly command: "close-pane" }
  | { readonly version: 1; readonly command: "error"; readonly text: string };

const MAX_ERROR_CHARS = 300;

/** A command from main, or null for anything that is not exactly one. */
export function parseMenuCommand(value: unknown): MenuCommandV1 | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
  const record = value as Record<string, unknown>;
  if (record.version !== 1) return null;
  if (record.command === "settings" && Object.keys(record).length === 2) {
    return { version: 1, command: "settings" };
  }
  if (record.command === "close-pane" && Object.keys(record).length === 2) {
    return { version: 1, command: "close-pane" };
  }
  if (record.command === "error" && typeof record.text === "string"
      && record.text.trim() !== "" && record.text.length <= MAX_ERROR_CHARS
      && Object.keys(record).length === 3) {
    return { version: 1, command: "error", text: record.text };
  }
  return null;
}
