import { execFile } from "node:child_process";
import { randomUUID } from "node:crypto";
import { lstatSync, mkdirSync } from "node:fs";
import { lstat, unlink } from "node:fs/promises";
import { createRequire } from "node:module";
import { basename, join } from "node:path";
import type {
  BrowserWindow, BrowserWindowConstructorOptions, ContextMenuParams, DownloadItem, HandlerDetails, MenuItemConstructorOptions,
  Rectangle, Session, WebContents, WebPreferences,
} from "electron";
import { browserKeyAction, browserZoomFactor, type BrowserEventV1, type BrowserKeyAction } from "../shared/browser-events.js";

/**
 * The only session a Browser pane's pages run in. It is never the app's own
 * session, so a page shares no cookies, storage or permission grants with the
 * renderer, and the renderer's CSP and permission policy stay untouched.
 */
export const BROWSER_PANE_PARTITION = "persist:browser-pane";

const MAX_BROWSER_URL_LENGTH = 8_192;

/** A web page with a host and no embedded credentials, or null. */
export function browserPaneUrl(raw: string): string | null {
  if (raw.length === 0 || raw.length > MAX_BROWSER_URL_LENGTH) return null;
  let url: URL;
  try { url = new URL(raw); } catch { return null; }
  if (url.protocol !== "http:" && url.protocol !== "https:") return null;
  if (url.hostname.length === 0 || url.username.length > 0 || url.password.length > 0) return null;
  return url.href;
}

/**
 * Pins a `<webview>`'s preferences before it attaches, whatever its markup
 * asked for: no preload (so no `window.iface`), no Node, a sandboxed and
 * context-isolated renderer. Only the Browser pane's session and a web page
 * may attach; anything else is refused.
 */
export function hardenBrowserPaneAttach(
  webPreferences: WebPreferences, params: Record<string, string>,
): boolean {
  if (params.partition !== BROWSER_PANE_PARTITION || browserPaneUrl(params.src ?? "") === null) {
    return false;
  }
  delete webPreferences.preload;
  delete (webPreferences as { preloadURL?: string }).preloadURL;
  webPreferences.nodeIntegration = false;
  webPreferences.nodeIntegrationInSubFrames = false;
  webPreferences.nodeIntegrationInWorker = false;
  webPreferences.contextIsolation = true;
  webPreferences.sandbox = true;
  webPreferences.webSecurity = true;
  webPreferences.allowRunningInsecureContent = false;
  webPreferences.experimentalFeatures = false;
  webPreferences.webviewTag = false;
  // Chromium's own PDF viewer shows a PDF in the page instead of saving it.
  webPreferences.plugins = true;
  // A page's fullscreen fills the app window, never a new Space (the app
  // window's own preference, set in main.ts, is the one Electron reads).
  webPreferences.disableHtmlFullscreenWindowResize = true;
  return true;
}

/** Chrome's user agent without Electron's and the app's tokens, which some sites refuse. */
export function browserPaneUserAgent(agent: string): string {
  return agent.replace(/\s(?:Electron|interface-electron|Interface(?: Dev)?)\/\S+/g, "");
}


const MAX_SCHEME_LENGTH = 32;
/** Schemes that are the web's, the browser's own, or unsafe network-mount / system-scripting handlers. */
const NOT_EXTERNAL = new Set([
  "http", "https", "file", "javascript", "data", "blob", "about", "chrome", "chrome-extension",
  "chrome-untrusted", "devtools", "view-source", "filesystem", "ws", "wss", "ftp",
  "smb", "afp", "nfs", "cifs", "webdav", "dav", "davs", "applescript", "x-apple.systempreferences", "prefs",
]);

/**
 * The scheme of a link meant for another app (`mailto:`, `zoommtg:`…), or
 * null for anything the web or the browser handles itself.
 */
export function browserExternalScheme(raw: string): string | null {
  if (raw.length === 0 || raw.length > MAX_BROWSER_URL_LENGTH || /[\u0000-\u001f\u007f\s]/u.test(raw)) return null;
  const scheme = /^([a-z][a-z0-9+.-]*):/iu.exec(raw)?.[1]?.toLowerCase();
  if (scheme === undefined || scheme.length > MAX_SCHEME_LENGTH || NOT_EXTERNAL.has(scheme)) return null;
  return scheme;
}

/** The web origin a site is known by, or null for anything else. */
export function browserSite(raw: string): string | null {
  const url = browserPaneUrl(raw);
  return url === null ? null : new URL(url).origin;
}

export type BrowserOpenAction =
  | { readonly kind: "tab"; readonly url: string; readonly background: boolean }
  | { readonly kind: "popup" }
  | { readonly kind: "external"; readonly url: string }
  | { readonly kind: "refuse" };

/**
 * Where a link that wants a new tab or window goes, Chrome's way:
 * `target=_blank` and `window.open` without a size open a tab in front;
 * ⌘-click and middle-click one behind; a `window.open` with features (the
 * sign-in popups) a small window that keeps `window.opener`. A link for
 * another app asks first. Anything that is not a web page is refused.
 */
export function browserOpenAction(details: Pick<HandlerDetails, "url" | "disposition" | "features">): BrowserOpenAction {
  const url = browserPaneUrl(details.url);
  if (url === null) {
    return browserExternalScheme(details.url) === null ? { kind: "refuse" } : { kind: "external", url: details.url };
  }
  if (details.disposition === "background-tab") return { kind: "tab", url, background: true };
  if (details.disposition === "new-window" && details.features.trim() !== "") return { kind: "popup" };
  return { kind: "tab", url, background: false };
}

/**
 * What a popup's page runs with, pinned exactly as a page is (no preload, no
 * Node, sandboxed, context-isolated): Electron applies these preferences to
 * the popup's page before `browserPopupWindowOptions` builds its window.
 * Electron opens it in the opener's session, which keeps `window.opener`
 * working.
 */
export function browserPopupOptions(parent: BrowserWindow | null, show: boolean): BrowserWindowConstructorOptions {
  return {
    ...(parent === null ? {} : { parent }),
    show,
    autoHideMenuBar: true,
    backgroundColor: "#ffffff",
    transparent: false,
    webPreferences: {
      nodeIntegration: false,
      nodeIntegrationInSubFrames: false,
      nodeIntegrationInWorker: false,
      contextIsolation: true,
      sandbox: true,
      webSecurity: true,
      allowRunningInsecureContent: false,
      experimentalFeatures: false,
      webviewTag: false,
      plugins: true,
      disableHtmlFullscreenWindowResize: true,
    },
  };
}

/** The smallest popup window: room for its title, which names the site. */
export const MIN_BROWSER_POPUP = { width: 320, height: 240 } as const;
/** A popup that names no size gets Electron's (and Chrome's) usual one. */
const DEFAULT_BROWSER_POPUP = { width: 800, height: 600 } as const;

/**
 * Where a popup's window goes and how big it is. The page may ask
 * (`window.open`'s width, height, left and top, or `resizeTo`/`moveTo` later),
 * but the window never leaves the work area of the screen it is on, is never
 * larger than it, and never smaller than `MIN_BROWSER_POPUP`. With no place
 * asked for, it is centred on `near` (the app window), else on the screen.
 */
export function browserPopupBounds(requested: Partial<Record<"x" | "y" | "width" | "height", unknown>>,
    area: Rectangle, near: Rectangle | null): Rectangle {
  const finite = (value: unknown): value is number => typeof value === "number" && Number.isFinite(value);
  const size = (value: unknown, fallback: number, least: number, most: number): number =>
    Math.max(Math.min(least, most), Math.min(finite(value) ? Math.round(value) : fallback, most));
  const width = size(requested.width, DEFAULT_BROWSER_POPUP.width, MIN_BROWSER_POPUP.width, area.width);
  const height = size(requested.height, DEFAULT_BROWSER_POPUP.height, MIN_BROWSER_POPUP.height, area.height);
  const centre = near ?? area;
  const place = (value: unknown, middle: number, start: number, extent: number, length: number): number =>
    Math.min(Math.max(finite(value) ? Math.round(value) : Math.round(middle - length / 2), start), start + extent - length);
  return {
    x: place(requested.x, centre.x + centre.width / 2, area.x, area.width, width),
    y: place(requested.y, centre.y + centre.height / 2, area.y, area.height, height),
    width, height,
  };
}

/**
 * A popup's whole window, built here rather than from the page's
 * `window.open` features: Electron lets those set the frame, always-on-top,
 * closable, movable, focus, transparency, opacity, size and more, so none of
 * them is read, and a key Electron adds later cannot reach the window either.
 * It is always an ordinary titled window (the title names the site), closable,
 * movable, opaque, focusable and never on top, a child of the app window.
 */
export function browserPopupWindowOptions(parent: BrowserWindow | null, show: boolean, title: string,
    bounds: Rectangle): BrowserWindowConstructorOptions {
  return {
    ...(parent === null ? {} : { parent }),
    show,
    title,
    ...bounds,
    minWidth: Math.min(MIN_BROWSER_POPUP.width, bounds.width),
    minHeight: Math.min(MIN_BROWSER_POPUP.height, bounds.height),
    frame: true,
    titleBarStyle: "default",
    transparent: false,
    opacity: 1,
    hasShadow: true,
    closable: true,
    movable: true,
    resizable: true,
    minimizable: true,
    maximizable: true,
    focusable: true,
    alwaysOnTop: false,
    modal: false,
    kiosk: false,
    fullscreen: false,
    skipTaskbar: false,
    hiddenInMissionControl: false,
    enableLargerThanScreen: false,
    useContentSize: false,
    acceptFirstMouse: false,
    autoHideMenuBar: true,
    backgroundColor: "#ffffff",
  };
}

const loadModule = createRequire(import.meta.url);
/**
 * Electron's main-process API, loaded only when a popup or a screen is first
 * needed, so this module still imports where Electron is not running (tests).
 */
function electronMain(): typeof import("electron") {
  return loadModule("electron") as typeof import("electron");
}

/**
 * How long a press in a page lets it open one tab or window: Chrome's
 * transient activation. A script with no press opens nothing, as Chrome's
 * popup blocker refuses it.
 */
export const BROWSER_ACTIVATION_MS = 5_000;
/** Popup windows one tab (with every popup it opened) may hold open at once. */
export const MAX_BROWSER_POPUPS = 2;

/**
 * A page's last press (a mouse button or a key, Escape aside), spent by the
 * one tab or window it opens. Times are `Date.now()` milliseconds.
 */
export class BrowserActivation {
  private at: number | null = null;

  press(now: number): void { this.at = now; }

  /** True once per press, within BROWSER_ACTIVATION_MS of it. */
  consume(now: number): boolean {
    const at = this.at;
    if (at === null || now < at || now - at > BROWSER_ACTIVATION_MS) return false;
    this.at = null;
    return true;
  }
}

// Controls, and the characters that reorder or hide text.
const TITLE_NOISE = /[\u0000-\u001f\u007f-\u009f\u00ad\u061c\u200b-\u200f\u202a-\u202e\u2060-\u2064\u2066-\u206f\ufeff]/gu;
const MAX_TITLE_CHARS = 120;

/**
 * A popup window's title: the site it is on first, then the page's own
 * title, so a page cannot pass itself off as the app or another site.
 */
export function browserPopupTitle(url: string, title: string): string {
  const site = browserSite(url);
  const host = site === null ? "Web page" : new URL(site).host;
  const own = Array.from(title.replace(TITLE_NOISE, "").replace(/\s+/gu, " ").trim()).slice(0, MAX_TITLE_CHARS).join("");
  return own === "" || own === url || browserSite(own) === site ? host : `${host} — ${own}`;
}

export type BrowserMenuAction =
  | { readonly kind: "open-tab"; readonly url: string; readonly background: boolean }
  | { readonly kind: "copy-text"; readonly text: string }
  | { readonly kind: "copy-image"; readonly x: number; readonly y: number }
  | { readonly kind: "download"; readonly url: string }
  | { readonly kind: "edit"; readonly command: "cut" | "copy" | "paste" | "selectAll" }
  | { readonly kind: "spelling"; readonly word: string }
  | { readonly kind: "back" | "forward" | "reload" }
  | { readonly kind: "inspect"; readonly x: number; readonly y: number };

export type BrowserMenuItem =
  | { readonly kind: "separator" }
  | { readonly kind: "item"; readonly label: string; readonly enabled: boolean; readonly action: BrowserMenuAction };

export type BrowserMenuParams = Pick<ContextMenuParams, "x" | "y" | "linkURL" | "srcURL" | "mediaType"
  | "isEditable" | "selectionText" | "misspelledWord" | "dictionarySuggestions" | "editFlags">;

const SEARCH_LABEL_CHARS = 32;
const MAX_SPELLING_SUGGESTIONS = 5;

/** Chrome's "Search Google for “…”", the selection cut short as Chrome does. */
export function browserSearchLabel(selection: string): string {
  const text = selection.replace(/\s+/gu, " ").trim();
  const chars = Array.from(text);
  return `Search Google for “${chars.length > SEARCH_LABEL_CHARS
    ? `${chars.slice(0, SEARCH_LABEL_CHARS - 1).join("")}…` : text}”`;
}

/**
 * The right-click menu for what is under the pointer, in Chrome's order and
 * with only what applies: a link, an image, a text field (spelling first),
 * selected text, then Back, Forward and Reload, and Inspect last.
 */
export function browserContextMenu(params: BrowserMenuParams,
    history: { readonly back: boolean; readonly forward: boolean }): BrowserMenuItem[] {
  const sections: BrowserMenuItem[][] = [];
  const item = (label: string, action: BrowserMenuAction, enabled = true): BrowserMenuItem =>
    ({ kind: "item", label, enabled, action });
  const link = browserPaneUrl(params.linkURL);
  if (params.linkURL !== "") {
    sections.push([
      ...(link === null ? [] : [item("Open Link in New Tab", { kind: "open-tab", url: link, background: true })]),
      item("Copy Link Address", { kind: "copy-text", text: params.linkURL }),
    ]);
  }
  if (params.mediaType === "image") {
    const image = browserPaneUrl(params.srcURL);
    sections.push([
      ...(image === null ? [] : [
        item("Open Image in New Tab", { kind: "open-tab", url: image, background: true }),
        item("Save Image As…", { kind: "download", url: image }),
      ]),
      item("Copy Image", { kind: "copy-image", x: params.x, y: params.y }),
    ]);
  }
  const selection = params.selectionText.trim();
  if (params.isEditable) {
    if (params.misspelledWord !== "" && params.dictionarySuggestions.length > 0) {
      sections.push(params.dictionarySuggestions.slice(0, MAX_SPELLING_SUGGESTIONS)
        .map(word => item(word, { kind: "spelling", word })));
    }
    sections.push([
      item("Cut", { kind: "edit", command: "cut" }, params.editFlags.canCut),
      item("Copy", { kind: "edit", command: "copy" }, params.editFlags.canCopy),
      item("Paste", { kind: "edit", command: "paste" }, params.editFlags.canPaste),
      item("Select All", { kind: "edit", command: "selectAll" }, params.editFlags.canSelectAll),
    ]);
  } else if (selection !== "") {
    const search = browserPaneUrl(`https://www.google.com/search?q=${encodeURIComponent(selection)}`);
    sections.push([
      item("Copy", { kind: "edit", command: "copy" }),
      ...(search === null ? [] : [item(browserSearchLabel(selection), { kind: "open-tab", url: search, background: false })]),
    ]);
  }
  sections.push([
    item("Back", { kind: "back" }, history.back),
    item("Forward", { kind: "forward" }, history.forward),
    item("Reload", { kind: "reload" }),
  ]);
  sections.push([item("Inspect", { kind: "inspect", x: params.x, y: params.y })]);
  return sections.flatMap((section, index) => index === 0 ? section : [{ kind: "separator" as const }, ...section]);
}

// \u0022 is the double quote, spelled so a plain scan of this file reads no string here.
const UNSAFE_NAME = /[/\\:*?\u0022<>|\u0000-\u001f\u007f]/gu;
const MAX_NAME_CHARS = 200;
const DOUBLE_EXTENSION = /\.tar\.(?:gz|bz2|xz|zst|z)$/iu;

/** A download's file name, made safe for the Downloads folder. */
export function browserDownloadName(suggested: string): string {
  let name = suggested.replace(UNSAFE_NAME, "_").replace(/^[\s.]+|[\s.]+$/gu, "");
  if (name.length === 0) name = "download";
  if (name.length > MAX_NAME_CHARS) {
    const { stem, extension } = splitName(name);
    name = `${stem.slice(0, Math.max(1, MAX_NAME_CHARS - extension.length))}${extension}`;
  }
  return name;
}

function splitName(name: string): { stem: string; extension: string } {
  const double = DOUBLE_EXTENSION.exec(name);
  const at = double !== null && double.index > 0 ? double.index : name.lastIndexOf(".");
  return at > 0 ? { stem: name.slice(0, at), extension: name.slice(at) } : { stem: name, extension: "" };
}

/** Chrome's naming: `name.ext`, else `name (1).ext`, `name (2).ext`… never a taken one. */
export function browserDownloadPath(folder: string, name: string, taken: (path: string) => boolean): string {
  const { stem, extension } = splitName(name);
  for (let copy = 0; copy < 10_000; copy++) {
    const candidate = join(folder, copy === 0 ? name : `${stem} (${copy.toString()})${extension}`);
    if (!taken(candidate)) return candidate;
  }
  return join(folder, `${stem} (${Date.now().toString()})${extension}`);
}

/**
 * True when a path entry already exists on disk — including a dangling symlink,
 * which `existsSync` reports as missing and `setSavePath` would follow. Any
 * non-ENOENT lookup failure fails closed as taken.
 */
export function browserPathExists(candidate: string): boolean {
  try {
    lstatSync(candidate);
    return true;
  } catch (error) {
    return (error as NodeJS.ErrnoException).code !== "ENOENT";
  }
}

const PROMPT_DOWNLOAD_EXTENSIONS = new Set([
  ".command", ".sh", ".zsh", ".terminal", ".app", ".pkg", ".dmg",
  ".webloc", ".inetloc", ".fileloc",
]);

/** Executable, installer and macOS location-file downloads require an explicit Save As choice. */
export function browserDownloadRequiresSaveDialog(filename: string): boolean {
  const clean = browserDownloadName(filename).toLowerCase();
  const dot = clean.lastIndexOf(".");
  return dot >= 0 && PROMPT_DOWNLOAD_EXTENSIONS.has(clean.slice(dot));
}

/** macOS's quarantine attribute: Gatekeeper checks a file that carries it before it first opens. */
export const BROWSER_QUARANTINE_ATTRIBUTE = "com.apple.quarantine";
/** What Finder's Get Info shows as its Where from line: the download's address, then the page's. */
export const BROWSER_WHERE_FROMS_ATTRIBUTE = "com.apple.metadata:kMDItemWhereFroms";
/**
 * The flags macOS itself writes for a web download from an app that is not
 * sandboxed (`LSQuarantineTypeWebDownload`), as on a Chrome or Firefox download.
 */
const QUARANTINE_WEB_DOWNLOAD = "0081";
const QUARANTINE_AGENT = "Interface";

/**
 * The quarantine attribute's value, in macOS's own format: flags, the time in
 * hexadecimal seconds, the app that downloaded it, and the event's id.
 */
export function browserQuarantineValue(now: number, id: string): string {
  const seconds = Math.max(0, Math.floor(now / 1000)).toString(16).padStart(8, "0");
  return `${QUARANTINE_WEB_DOWNLOAD};${seconds};${QUARANTINE_AGENT};${id}`;
}

/**
 * The addresses a download is recorded as coming from, Chrome's way: the
 * file's own address, then the page's, web addresses only and without any
 * user name or password in them.
 */
export function browserDownloadOrigin(download: string, page: string): string[] {
  const clean = (raw: string): string | null => {
    let url: URL;
    try { url = new URL(raw); } catch { return null; }
    if (url.protocol !== "http:" && url.protocol !== "https:") return null;
    url.username = "";
    url.password = "";
    return url.href.length > MAX_BROWSER_URL_LENGTH ? null : url.href;
  };
  const urls: string[] = [];
  for (const raw of [download, page]) {
    const url = clean(raw);
    if (url !== null && !urls.includes(url)) urls.push(url);
  }
  return urls;
}

/** A binary property list holding an array of strings: the form macOS stores the Where from line in. */
export function browserWhereFroms(urls: readonly string[]): Buffer {
  const count = (marker: number, length: number): Buffer => {
    if (length < 15) return Buffer.of(marker | length);
    const bytes = length < 0x100 ? 1 : length < 0x10000 ? 2 : 4;
    const value = Buffer.alloc(bytes);
    value.writeUIntBE(length, 0, bytes);
    return Buffer.concat([Buffer.of(marker | 0x0f, 0x10 | Math.log2(bytes)), value]);
  };
  const text = (value: string): Buffer => {
    if (/^[\u0000-\u007f]*$/u.test(value)) return Buffer.concat([count(0x50, value.length), Buffer.from(value, "latin1")]);
    const units = Buffer.from(value, "utf16le").swap16();
    return Buffer.concat([count(0x60, units.length / 2), units]);
  };
  const strings = urls.slice(0, 14);
  const objects = [Buffer.concat([count(0xa0, strings.length), Buffer.from(strings.map((_url, index) => index + 1))]),
    ...strings.map(text)];
  const offsets: number[] = [];
  let at = 8;
  for (const object of objects) { offsets.push(at); at += object.length; }
  const offsetSize = at < 0x100 ? 1 : at < 0x10000 ? 2 : 4;
  const table = Buffer.alloc(offsets.length * offsetSize);
  offsets.forEach((offset, index) => { table.writeUIntBE(offset, index * offsetSize, offsetSize); });
  const trailer = Buffer.alloc(32);
  trailer.writeUInt8(offsetSize, 6);
  trailer.writeUInt8(1, 7);
  trailer.writeBigUInt64BE(BigInt(objects.length), 8);
  trailer.writeBigUInt64BE(0n, 16);
  trailer.writeBigUInt64BE(BigInt(at), 24);
  return Buffer.concat([Buffer.from("bplist00", "latin1"), ...objects, table, trailer]);
}

const XATTR = "/usr/bin/xattr";
const XATTR_TIMEOUT_MS = 10_000;

function xattr(args: readonly string[]): Promise<string> {
  return new Promise((resolve, reject) => {
    execFile(XATTR, args, { timeout: XATTR_TIMEOUT_MS, encoding: "utf8" }, (error, stdout) => {
      if (error === null) resolve(stdout); else reject(error);
    });
  });
}

/**
 * Marks a finished download as come from the web, as Safari and Chrome do, so
 * Gatekeeper checks it before it first opens: the quarantine attribute (read
 * back to be sure it holds) and, where it can, the Where from line. Rejects when the
 * quarantine cannot be written, so the caller never calls the file saved.
 * Only the download is marked, never the app's own files or the Rust host's.
 */
export async function quarantineBrowserDownload(path: string, origin: readonly string[]): Promise<void> {
  // Gatekeeper and its attribute are macOS's; elsewhere there is nothing to write.
  if (process.platform !== "darwin") return;
  if (!(await lstat(path)).isFile()) throw new Error("A download is not a plain file.");
  const value = browserQuarantineValue(Date.now(), randomUUID().toUpperCase());
  await xattr(["-w", BROWSER_QUARANTINE_ATTRIBUTE, value, path]);
  if ((await xattr(["-p", BROWSER_QUARANTINE_ATTRIBUTE, path])).trim() !== value) {
    throw new Error("The quarantine attribute did not hold.");
  }
  if (origin.length === 0) return;
  // The Where from line is a record for the owner; the quarantine above is what protects.
  await xattr(["-wx", BROWSER_WHERE_FROMS_ATTRIBUTE, browserWhereFroms(origin).toString("hex"), path])
    .catch(() => undefined);
}

/**
 * What a page may ask the owner for. `external:<scheme>` opens another app;
 * `download` saves a file the owner did not click for.
 */
export type BrowserAsk = "camera" | "microphone" | "notifications" | "clipboard-read" | "download" | `external:${string}`;

export type BrowserPermissionDecision =
  | { readonly kind: "allow" }
  | { readonly kind: "refuse" }
  | { readonly kind: "ask"; readonly asks: readonly BrowserAsk[] };

/**
 * One request from a page. Camera, microphone, notifications and reading
 * the clipboard ask the owner; fullscreen, pointer lock and writing to the
 * clipboard (a site's Copy button) are allowed as Chrome allows them; opening another app asks with its scheme named;
 * everything else (location, screen capture, MIDI, devices…) is refused.
 */
export function browserPermissionRequest(permission: string,
    details: { readonly mediaTypes?: readonly string[]; readonly externalURL?: string }): BrowserPermissionDecision {
  switch (permission) {
    case "fullscreen": case "pointerLock": case "clipboard-sanitized-write": return { kind: "allow" };
    case "notifications": case "clipboard-read": return { kind: "ask", asks: [permission] };
    case "media": {
      const types = details.mediaTypes ?? [];
      if (types.length === 0 || types.some(type => type !== "video" && type !== "audio")) return { kind: "refuse" };
      const asks: BrowserAsk[] = [];
      if (types.includes("video")) asks.push("camera");
      if (types.includes("audio")) asks.push("microphone");
      return { kind: "ask", asks };
    }
    case "openExternal": {
      const scheme = browserExternalScheme(details.externalURL ?? "");
      return scheme === null ? { kind: "refuse" } : { kind: "ask", asks: [`external:${scheme}`] };
    }
    default: return { kind: "refuse" };
  }
}

/** Whether a page already holds a permission: exactly what was granted, nothing more. */
export function browserPermissionCheck(permission: string, mediaType: string | undefined,
    granted: (ask: BrowserAsk) => boolean): boolean {
  switch (permission) {
    case "fullscreen": case "pointerLock": case "clipboard-sanitized-write": return true;
    case "notifications": case "clipboard-read": return granted(permission);
    case "media":
      return mediaType === "video" ? granted("camera") : mediaType === "audio" ? granted("microphone") : false;
    default: return false;
  }
}

export interface BrowserQuestion {
  /** The site's host, as the owner reads it. */
  readonly site: string;
  readonly asks: readonly BrowserAsk[];
  readonly message: string;
  readonly detail: string;
}

const MAX_EXTERNAL_PREVIEW_CHARS = 120;

/** A cleaned, bounded preview of an external destination URL for the confirmation sheet. */
export function browserExternalPreview(raw: string): string {
  const clean = raw.replace(TITLE_NOISE, "").replace(/\s+/gu, " ").trim();
  const chars = Array.from(clean);
  return chars.length > MAX_EXTERNAL_PREVIEW_CHARS
    ? `${chars.slice(0, MAX_EXTERNAL_PREVIEW_CHARS - 1).join("")}…`
    : clean;
}

/**
 * The sheet's words: “meet.google.com” wants to use your camera and
 * microphone. `subject` is what it is about: another app's link (shown as its
 * destination) or a download's file name.
 */
export function browserQuestion(site: string, asks: readonly BrowserAsk[], subject?: string): BrowserQuestion {
  const external = asks.find(ask => ask.startsWith("external:"));
  const download = external === undefined && asks.includes("download");
  const wants = (): string => {
    if (external !== undefined) return `open “${external.slice("external:".length)}:” links in another app`;
    if (download) return "download files without a click";
    if (asks.includes("camera") && asks.includes("microphone")) return "use your camera and microphone";
    if (asks.includes("camera")) return "use your camera";
    if (asks.includes("microphone")) return "use your microphone";
    if (asks.includes("notifications")) return "show notifications";
    return "see text and images you copy";
  };
  const label = external !== undefined ? "Destination" : download ? "File" : null;
  const preview = label !== null && subject !== undefined ? browserExternalPreview(subject) : "";
  const hold = "Your answer holds for this site until the app quits.";
  return {
    site, asks, message: `“${site}” wants to ${wants()}`,
    detail: preview === "" ? hold : `${label}: ${preview}\n\n${hold}`,
  };
}

/**
 * The owner's answers, per site and permission, for this app run only:
 * kept in memory, never stored. A site asks once; a request already being
 * asked waits for that answer instead of asking again.
 */
export class BrowserGrants {
  private readonly answers = new Map<string, boolean>();
  private readonly pending = new Map<string, Promise<boolean>>();

  granted(site: string, ask: BrowserAsk): boolean {
    return this.answers.get(`${site}\n${ask}`) === true;
  }

  ask(site: string, asks: readonly BrowserAsk[], question: (open: readonly BrowserAsk[]) => Promise<boolean>): Promise<boolean> {
    const answer = (ask: BrowserAsk): boolean | undefined => this.answers.get(`${site}\n${ask}`);
    if (asks.length === 0 || asks.some(ask => answer(ask) === false)) return Promise.resolve(false);
    const open = asks.filter(ask => answer(ask) === undefined);
    if (open.length === 0) return Promise.resolve(true);
    const key = `${site}\n${open.join("\n")}`;
    const asking = this.pending.get(key);
    if (asking !== undefined) return asking;
    const asked = question(open).then(allow => allow === true, () => false).then(allow => {
      for (const ask of open) this.answers.set(`${site}\n${ask}`, allow);
      this.pending.delete(key);
      return allow;
    });
    this.pending.set(key, asked);
    return asked;
  }
}

/** What the guards need from the app and the system; tests and hands-free runs give their own. */
export interface BrowserPaneHooks {
  /** The folder downloads are saved in. Tests and hands-free runs give a temp folder. */
  readonly downloads: () => string;
  /** Asks the owner (a sheet on the app window); true allows. Tests answer it. */
  readonly ask: (question: BrowserQuestion) => Promise<boolean>;
  /** Shows a page's right-click menu. */
  readonly showMenu: (template: MenuItemConstructorOptions[], contents: WebContents) => void;
  readonly openExternal: (url: string) => Promise<void>;
  readonly copyText: (text: string) => void;
  /** Tells the app window's renderer (`BROWSER_EVENT_CHANNEL`). */
  readonly send: (event: BrowserEventV1) => void;
  /** The app window: a popup's parent. */
  readonly window: () => BrowserWindow | null;
  /** Popups appear on screen; false in tests and hands-free runs. */
  readonly showPopups: boolean;
  /** Prompts for a destination before saving an executable, installer or location file; null cancels. */
  readonly chooseSavePath?: (defaultPath: string, name: string) => string | null;
  /** A finished download (the Dock's Downloads stack bounces). */
  readonly downloadFinished?: (path: string) => void;
  /**
   * Marks a finished download as come from the web before it is called saved;
   * a rejection fails it. `quarantineBrowserDownload` unless a test gives its own.
   */
  readonly quarantine?: (path: string, origin: readonly string[]) => Promise<void>;
  /** Makes a popup's window from the options chosen here; Electron's `BrowserWindow` unless a test gives its own. */
  readonly popupWindow?: (options: BrowserWindowConstructorOptions) => BrowserWindow;
  /** The work area of the screen showing `near` (or the main screen); Electron's `screen` unless a test gives its own. */
  readonly workArea?: (near: Rectangle | null) => Rectangle;
}

/** What a page's downloads are judged by: did the owner start one, or the page on its own? */
interface PageDownloads {
  /** A press in the page, spent by one download (apart from the one tab or window it may open). */
  readonly press: BrowserActivation;
  /** The address the app is loading in the page (address bar, reload, a tab's first page), until the page navigates itself. */
  loading: string | null;
  /** Addresses main itself is downloading in the page (the right-click menu's Save Image As…). */
  readonly own: string[];
}
const MAX_OWN_DOWNLOADS = 8;

/**
 * Browser panes embedded by the app window. Pages navigate only to the web;
 * links that want a new tab open one beside the page, a sign-in popup opens
 * a pinned child window, downloads save to the Downloads folder, flagged for
 * Gatekeeper, and a page asks the owner before it uses the camera,
 * microphone, notifications, the clipboard or another app, or downloads
 * anything without a click. Local files are not served.
 */
export function installBrowserPaneGuards(host: WebContents, pages: Session, hooks: BrowserPaneHooks): void {
  const grants = new BrowserGrants();
  /** Every page and popup, by the tab page whose pane it belongs to. */
  const pageOf = new WeakMap<WebContents, number>();
  /** Popup windows open, by the tab page they belong to. */
  const popupsOf = new Map<number, number>();
  const downloadsOf = new WeakMap<WebContents, PageDownloads>();
  // Hooks are read only when used: a lane may install the guards with none.
  const quarantine = (path: string, origin: readonly string[]): Promise<void> =>
    (hooks.quarantine ?? quarantineBrowserDownload)(path, origin);
  const popupWindow = (options: BrowserWindowConstructorOptions): BrowserWindow =>
    hooks.popupWindow?.(options) ?? new (electronMain().BrowserWindow)(options);
  const workArea = (near: Rectangle | null): Rectangle => {
    if (hooks.workArea !== undefined) return hooks.workArea(near);
    const { screen } = electronMain();
    return (near === null ? screen.getPrimaryDisplay() : screen.getDisplayMatching(near)).workArea;
  };
  const ask = (site: string | null, asks: readonly BrowserAsk[], subject?: string): Promise<boolean> => {
    if (site === null) return Promise.resolve(false);
    const label = new URL(site).host;
    return grants.ask(site, asks, open => hooks.ask(browserQuestion(label, open, subject)));
  };
  pages.setPermissionRequestHandler((_contents, permission, decide, details) => {
    const requestDetails = details as { mediaTypes?: readonly string[]; externalURL?: string };
    const decision = browserPermissionRequest(permission, requestDetails);
    if (decision.kind !== "ask") { decide(decision.kind === "allow"); return; }
    void ask(browserSite(details.requestingUrl ?? ""), decision.asks, requestDetails.externalURL)
      .then(decide, () => { decide(false); });
  });
  pages.setPermissionCheckHandler((_contents, permission, origin, details) => {
    const site = browserSite(origin);
    return site !== null && browserPermissionCheck(permission, details.mediaType, asked => grants.granted(site, asked));
  });
  pages.setDevicePermissionHandler(() => false);
  pages.setUserAgent(browserPaneUserAgent(pages.getUserAgent()));
  pages.protocol.handle("file", () => new Response(null, { status: 403 }));

  const openOutside = async (contents: WebContents, url: string, activation: BrowserActivation): Promise<void> => {
    const scheme = browserExternalScheme(url);
    if (scheme === null || !activation.consume(Date.now())) return;
    if (!await ask(browserSite(contents.getURL()), [`external:${scheme}`], url)) return;
    await hooks.openExternal(url).catch(() => undefined);
  };
  const send = (event: BrowserEventV1): void => {
    try { hooks.send(event); } catch { /* the window is gone */ }
  };

  const runMenu = (contents: WebContents, page: number, action: BrowserMenuAction): void => {
    switch (action.kind) {
      case "open-tab": send({ version: 1, kind: "open-tab", page, url: action.url, background: action.background }); break;
      case "copy-text": hooks.copyText(action.text); break;
      case "copy-image": contents.copyImageAt(action.x, action.y); break;
      case "download": contents.downloadURL(action.url); break;
      case "edit": contents[action.command](); break;
      case "spelling": contents.replaceMisspelling(action.word); break;
      case "back": contents.navigationHistory.goBack(); break;
      case "forward": contents.navigationHistory.goForward(); break;
      case "reload": contents.reload(); break;
      case "inspect": contents.inspectElement(action.x, action.y); break;
    }
  };
  const runKey = (contents: WebContents, page: number, action: BrowserKeyAction): void => {
    switch (action) {
      case "zoom-in": case "zoom-out": case "zoom-reset":
        contents.setZoomFactor(browserZoomFactor(contents.getZoomFactor(),
          action === "zoom-in" ? "in" : action === "zoom-out" ? "out" : "reset"));
        break;
      case "reload": contents.reload(); break;
      case "hard-reload": contents.reloadIgnoringCache(); break;
      case "back": if (contents.navigationHistory.canGoBack()) contents.navigationHistory.goBack(); break;
      case "forward": if (contents.navigationHistory.canGoForward()) contents.navigationHistory.goForward(); break;
      // The pane's find field and address bar belong to the tab, not a popup.
      case "find": case "address": if (contents.id === page) send({ version: 1, kind: action, page }); break;
    }
  };

  /** The same guards on a tab's page and on every popup it opens. */
  const guard = (contents: WebContents, page: number): void => {
    pageOf.set(contents, page);
    // A click, a tap or a key in the page (Escape and the chords taken
    // below aside) lets it open one tab or window, as in Chrome, and start
    // one download without asking.
    const activation = new BrowserActivation();
    const downloads: PageDownloads = { press: new BrowserActivation(), loading: null, own: [] };
    downloadsOf.set(contents, downloads);
    const pressed = (): void => {
      const now = Date.now();
      activation.press(now);
      downloads.press.press(now);
    };
    contents.on("input-event", (_event, input) => {
      if (input.type === "mouseDown" || input.type === "touchEnd" || input.type === "gestureTap") pressed();
    });
    // Downloads main itself starts in the page (the right-click menu's Save
    // Image As…) are the owner's: each address is noted, so it never asks.
    const startDownload = (contents as Partial<Pick<WebContents, "downloadURL">>).downloadURL;
    if (typeof startDownload === "function") {
      contents.downloadURL = (url, options) => {
        downloads.own.push(url);
        if (downloads.own.length > MAX_OWN_DOWNLOADS) downloads.own.shift();
        startDownload.call(contents, url, options);
      };
    }
    // The app's own loads (the address bar, a reload, a tab's first page)
    // have no initiating frame; the page's own do, and pass will-navigate.
    contents.on("did-start-navigation", details => {
      if (!details.isMainFrame || details.isSameDocument) return;
      downloads.loading = details.initiator == null ? details.url : null;
    });
    contents.on("did-navigate", () => { downloads.loading = null; });

    /**
     * A popup's window, however it was made: counted against its tab, titled
     * with its site, kept on its screen when it moves or resizes itself, and
     * guarded like its opener.
     */
    const adopt = (popup: BrowserWindow, opened: string): void => {
      // Electron opens a popup in its opener's session; anything else closes.
      if (popup.webContents.session !== pages) { popup.destroy(); return; }
      popupsOf.set(page, (popupsOf.get(page) ?? 0) + 1);
      popup.once("closed", () => {
        const open = (popupsOf.get(page) ?? 1) - 1;
        if (open > 0) popupsOf.set(page, open); else popupsOf.delete(page);
      });
      // Its title leads with the site it is on, whatever the page calls itself.
      let url = opened;
      let title = "";
      const retitle = (): void => { popup.setTitle(browserPopupTitle(url, title)); };
      popup.on("page-title-updated", (event, next) => { event.preventDefault(); title = next; retitle(); });
      popup.webContents.on("did-navigate", (_event, next) => { url = next; retitle(); });
      popup.webContents.on("did-navigate-in-page", (_event, next, isMainFrame) => {
        if (isMainFrame) { url = next; retitle(); }
      });
      // window.resizeTo and moveTo keep to the bounds window.open got (the
      // page asks for its content's bounds; the window's are close enough).
      popup.webContents.on("content-bounds-updated", (event, bounds) => {
        event.preventDefault();
        try { popup.setBounds(browserPopupBounds(bounds, workArea(popup.getBounds()), null)); } catch { /* closed */ }
      });
      retitle();
      guard(popup.webContents, page);
    };
    /**
     * Builds a popup's window from `browserPopupWindowOptions` alone: of what
     * the page asked for, only the size and place are read, and clamped.
     * Electron has already pinned the popup's page (`browserPopupOptions`).
     */
    const openPopup = (options: BrowserWindowConstructorOptions, url: string): WebContents => {
      const guest = (options as { webContents?: WebContents }).webContents;
      try {
        const parent = hooks.window();
        const owner = parent === null || parent.isDestroyed() ? null : parent;
        const near = owner === null ? null : owner.getBounds();
        const bounds = browserPopupBounds(options, workArea(near), near);
        const popup = popupWindow({ ...browserPopupWindowOptions(owner, hooks.showPopups, browserPopupTitle(url, ""), bounds),
          webContents: guest } as BrowserWindowConstructorOptions);
        adopt(popup, url);
        return popup.webContents;
      } catch {
        // No window could be made: the popup's page closes rather than run unseen.
        try { if (guest !== undefined && !guest.isDestroyed()) guest.close(); } catch { /* already gone */ }
        return guest as WebContents;
      }
    };
    contents.setWindowOpenHandler(details => {
      const action = browserOpenAction(details);
      // Another app's link still asks the owner first; a tab or a window
      // needs a fresh press, and a tab holds only so many popups.
      if ((action.kind === "tab" || action.kind === "popup") && !activation.consume(Date.now())) return { action: "deny" };
      if (action.kind === "popup") {
        if ((popupsOf.get(page) ?? 0) >= MAX_BROWSER_POPUPS) return { action: "deny" };
        return {
          action: "allow",
          overrideBrowserWindowOptions: browserPopupOptions(hooks.window(), hooks.showPopups),
          createWindow: options => openPopup(options, details.url),
        };
      }
      if (action.kind === "tab") send({ version: 1, kind: "open-tab", page, url: action.url, background: action.background });
      else if (action.kind === "external") void openOutside(contents, action.url, activation);
      return { action: "deny" };
    });
    // Electron emits this only for a window it made itself, which the
    // createWindow above never leaves it to; such a window is still adopted.
    contents.on("did-create-window", (popup, details) => { adopt(popup, details.url); });
    contents.on("will-navigate", (event: { url: string; isMainFrame?: boolean; preventDefault(): void }) => {
      if (event.isMainFrame !== false) downloads.loading = null;
      if (event.isMainFrame === false || browserPaneUrl(event.url) !== null) return;
      event.preventDefault();
      void openOutside(contents, event.url, activation);
    });
    contents.on("will-redirect", (event: { url: string; isMainFrame?: boolean; preventDefault(): void }) => {
      if (event.isMainFrame === false || browserPaneUrl(event.url) !== null) return;
      event.preventDefault();
    });
    contents.on("context-menu", (_event, params) => {
      const history = { back: contents.navigationHistory.canGoBack(), forward: contents.navigationHistory.canGoForward() };
      hooks.showMenu(browserContextMenu(params, history).map(entry => entry.kind === "separator"
        ? { type: "separator" as const }
        : { label: entry.label, enabled: entry.enabled, click: () => { runMenu(contents, page, entry.action); } }),
      contents);
    });
    let fullscreen = false;
    contents.on("enter-html-full-screen", () => { fullscreen = true; });
    contents.on("leave-html-full-screen", () => { fullscreen = false; });
    // Keys in a page never reach the app window: the page's chords act here,
    // and keep the app menu's Reload and zoom from acting on the whole app.
    contents.on("before-input-event", (event, input) => {
      if (input.type !== "keyDown") return;
      if (fullscreen && input.key === "Escape" && !input.meta && !input.control && !input.alt && !input.shift) {
        event.preventDefault();
        void contents.executeJavaScript("document.exitFullscreen().catch(() => undefined)").catch(() => undefined);
        return;
      }
      const action = browserKeyAction(input);
      if (action === null) {
        if (input.key !== "Escape" && !input.isAutoRepeat) pressed();
        return;
      }
      event.preventDefault();
      runKey(contents, page, action);
    });
  };

  host.on("will-attach-webview", (event, webPreferences, params) => {
    if (!hardenBrowserPaneAttach(webPreferences, params)) event.preventDefault();
  });
  host.on("did-attach-webview", (_event, page) => { guard(page, page.id); });

  /**
   * Whether the owner started a download rather than the page on its own:
   * Chromium saw a press start it; the page has no document yet, so nothing
   * in it could (the address is one the app loaded: a tab opened from a
   * pressed link, or the owner's own); main is downloading, or the app
   * loading, that very address; or the page was pressed just before (one
   * download per press, as for windows). A download from anything but a
   * guarded page is the page's.
   */
  const ownerStarted = (contents: WebContents, item: DownloadItem, pageUrl: string | null): boolean => {
    const downloads = downloadsOf.get(contents);
    if (downloads === undefined || pageUrl === null) return false;
    const known = item as Partial<Pick<DownloadItem, "hasUserGesture" | "getURLChain" | "getURL">>;
    if (known.hasUserGesture?.() === true || pageUrl === "") return true;
    const first = known.getURLChain?.()[0] ?? known.getURL?.() ?? "";
    const own = downloads.own.indexOf(first);
    if (own >= 0) { downloads.own.splice(own, 1); return true; }
    if (downloads.loading !== null && downloads.loading === first) { downloads.loading = null; return true; }
    return downloads.press.consume(Date.now());
  };
  const quietly = (act: () => void): void => { try { act(); } catch { /* already gone */ } };

  // Ordinary files save straight into Downloads with Chrome's naming, never
  // overwriting files or dangling symlinks; executable/installer extensions
  // require an explicit Save As choice. Names being written are held so two at once differ.
  // One the page starts without a click asks first (an installer or script is
  // refused unless its site was allowed), and every finished one is flagged
  // for Gatekeeper before it is called saved.
  const writing = new Set<string>();
  pages.on("will-download", (event, item, contents) => {
    const folder = hooks.downloads();
    try { mkdirSync(folder, { recursive: true }); } catch { /* the save fails and says so */ }
    const safeName = browserDownloadName(item.getFilename());
    const defaultPath = browserDownloadPath(folder, safeName,
      candidate => writing.has(candidate) || browserPathExists(candidate));
    const page = pageOf.get(contents);
    const refuse = (): void => {
      (event as { preventDefault?: () => void }).preventDefault?.();
      quietly(() => { (item as { cancel?: () => void }).cancel?.(); });
      if (page !== undefined) send({ version: 1, kind: "download", page, state: "failed", name: basename(defaultPath) });
    };
    let pageUrl: string | null = null;
    try { pageUrl = contents.getURL(); } catch { /* no page: the download is refused below */ }
    const site = browserSite(pageUrl ?? "");
    const asking = !ownerStarted(contents, item, pageUrl) && !(site !== null && grants.granted(site, "download"));
    if (asking && (site === null || browserDownloadRequiresSaveDialog(safeName))) { refuse(); return; }
    let path = defaultPath;
    if (browserDownloadRequiresSaveDialog(safeName)) {
      const chosen = hooks.chooseSavePath?.(defaultPath, safeName) ?? null;
      if (chosen === null || chosen.trim() === "") { refuse(); return; }
      path = chosen;
    }
    writing.add(path);
    item.setSavePath(path);
    const name = basename(path);
    const tell = (state: "started" | "saved" | "failed"): void => {
      if (page !== undefined) send({ version: 1, kind: "download", page, state, name });
    };
    tell("started");
    let finished = false;
    // Held until the owner answers; a Block cancels it, which removes the partial file.
    if (asking) quietly(() => { item.pause(); });
    const allowed = asking ? ask(site, ["download"], name) : Promise.resolve(true);
    const finish = (state: string): void => {
      if (finished) return;
      finished = true;
      if (state !== "completed") { writing.delete(path); tell("failed"); return; }
      // Saved only if allowed and once flagged as come from the web, so
      // Gatekeeper checks it before it first opens; otherwise the file goes.
      const download = (item as Partial<Pick<DownloadItem, "getURL">>).getURL?.() ?? "";
      void allowed
        .then(allow => allow ? quarantine(path, browserDownloadOrigin(download, pageUrl ?? ""))
          : Promise.reject(new Error("The owner blocked this download.")))
        .then(() => {
          writing.delete(path);
          quietly(() => { hooks.downloadFinished?.(path); });
          tell("saved");
        }, () => unlink(path).catch(() => undefined).then(() => {
          writing.delete(path);
          tell("failed");
        }));
    };
    void allowed.then(allow => {
      if (!asking || finished) return;
      if (allow) quietly(() => { item.resume(); });
      else { finish("blocked"); quietly(() => { item.cancel(); }); }
    });
    // A dropped connection leaves the item interrupted, and Electron never
    // calls it done: it fails here, and cancelling it removes the partial
    // file. Not from inside Chromium's own update notice, which it forbids.
    item.on("updated", (_updated, state) => {
      if (state !== "interrupted") return;
      finish(state);
      setImmediate(() => { try { item.cancel(); } catch { /* already gone */ } });
    });
    item.once("done", (_done, state) => { finish(state); });
  });
}
