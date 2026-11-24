/**
 * What main tells the renderer about Browser pages (main/browser-pane.ts):
 * a link that wants a new tab, a key the page took that the pane acts on
 * (find, the address), and a download's progress. A main-only channel built
 * like the application menu's: the Rust host never sees it, nothing is
 * persisted, and App.tsx alone subscribes. `page` is the guest's webContents
 * id; the renderer finds the page by it (`<webview>.getWebContentsId()`).
 *
 * The pure key and zoom rules here are shared, so a chord behaves the same
 * whether the page or the pane's address bar has the keyboard.
 */

export const BROWSER_EVENT_CHANNEL = "iface:browserEvent";

export type BrowserDownloadState = "started" | "saved" | "failed";

export type BrowserEventV1 =
  | { readonly version: 1; readonly kind: "open-tab"; readonly page: number; readonly url: string;
      readonly background: boolean }
  | { readonly version: 1; readonly kind: "find"; readonly page: number }
  | { readonly version: 1; readonly kind: "address"; readonly page: number }
  | { readonly version: 1; readonly kind: "download"; readonly page: number;
      readonly state: BrowserDownloadState; readonly name: string };

const MAX_URL_LENGTH = 8_192;
const MAX_NAME_LENGTH = 255;
const MAX_PAGE_ID = 2 ** 31 - 1;

function webUrl(value: unknown): string | null {
  if (typeof value !== "string" || value.length === 0 || value.length > MAX_URL_LENGTH) return null;
  let url: URL;
  try { url = new URL(value); } catch { return null; }
  if (url.protocol !== "http:" && url.protocol !== "https:") return null;
  if (url.hostname.length === 0 || url.username.length > 0 || url.password.length > 0) return null;
  return url.href;
}

function fileName(value: unknown): string | null {
  if (typeof value !== "string" || value.length === 0 || value.length > MAX_NAME_LENGTH) return null;
  return /[/\\\u0000-\u001f\u007f]/u.test(value) ? null : value;
}

/** An event from main, or null for anything that is not exactly one. */
export function parseBrowserEvent(value: unknown): BrowserEventV1 | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
  const record = value as Record<string, unknown>;
  const page = record.page;
  if (record.version !== 1 || typeof page !== "number" || !Number.isInteger(page)
      || page <= 0 || page > MAX_PAGE_ID) return null;
  const keys = Object.keys(record).length;
  switch (record.kind) {
    case "open-tab": {
      const url = webUrl(record.url);
      if (url === null || typeof record.background !== "boolean" || keys !== 5) return null;
      return { version: 1, kind: "open-tab", page, url, background: record.background };
    }
    case "find": case "address":
      return keys === 3 ? { version: 1, kind: record.kind, page } : null;
    case "download": {
      const name = fileName(record.name);
      const state = record.state;
      if (name === null || keys !== 5
          || (state !== "started" && state !== "saved" && state !== "failed")) return null;
      return { version: 1, kind: "download", page, state, name };
    }
    default: return null;
  }
}

/** The quiet line a download leaves in its pane. */
export function browserDownloadNotice(state: BrowserDownloadState, name: string): string {
  if (state === "started") return `Downloading ${name}…`;
  if (state === "saved") return `Saved ${name} to Downloads`;
  return `${name} could not be downloaded.`;
}

/** Chrome's zoom steps, as factors. */
export const BROWSER_ZOOM_FACTORS: readonly number[] = [
  0.25, 0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2, 2.5, 3, 4, 5,
];

/** The next zoom factor, one Chrome step in or out, or back to 100%. */
export function browserZoomFactor(current: number, direction: "in" | "out" | "reset"): number {
  if (direction === "reset" || !Number.isFinite(current)) return 1;
  const steps = BROWSER_ZOOM_FACTORS;
  const next = direction === "in"
    ? steps.find(step => step > current + 0.001)
    : [...steps].reverse().find(step => step < current - 0.001);
  return next ?? (direction === "in" ? steps[steps.length - 1]! : steps[0]!);
}

export type BrowserKeyAction =
  | "zoom-in" | "zoom-out" | "zoom-reset" | "reload" | "hard-reload"
  | "back" | "forward" | "find" | "address";

/** The parts of a key event the page chords read (a DOM KeyboardEvent or Electron's Input). */
export interface BrowserKeyInput {
  readonly key: string;
  readonly meta: boolean;
  readonly control: boolean;
  readonly alt: boolean;
  readonly shift: boolean;
}

/**
 * The chords a Browser tab claims, as in Chrome: ⌘+ / ⌘= / ⌘− / ⌘0 zoom,
 * ⌘R reload (⌘⇧R without the cache), ⌘[ and ⌘] back and forward, ⌘F find
 * and ⌘L the address. Nothing else: ⌘W, ⌘T, ⌘Q, ⌘⇧[ ⌘⇧] and the app's own
 * chords are left alone.
 */
export function browserKeyAction(input: BrowserKeyInput): BrowserKeyAction | null {
  if (!input.meta || input.control || input.alt) return null;
  const key = input.key.toLowerCase();
  if (key === "=" || key === "+") return "zoom-in";
  if (input.shift) return key === "r" ? "hard-reload" : null;
  switch (key) {
    case "-": return "zoom-out";
    case "0": return "zoom-reset";
    case "r": return "reload";
    case "[": return "back";
    case "]": return "forward";
    case "f": return "find";
    case "l": return "address";
    default: return null;
  }
}
