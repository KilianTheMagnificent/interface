/**
 * Visual replies' frame (T3 section 8, protocol 61): how an agent-written page
 * is drawn without ever reaching the app, the network, storage or the owner's
 * files. Shared by the renderer (which wraps the page) and main (which serves
 * it). Contract: docs/superpowers/specs/2026-11-14-t3-systems-design.md §8.
 *
 * The fence, outermost first:
 *  1. The frame is `<iframe sandbox="allow-scripts">` — no `allow-same-origin`,
 *     forms, popups, top navigation, modals or downloads — so the page runs at
 *     an opaque origin: no cookies, no storage, no reach into the app.
 *  2. Its document is served by main from the `iface-visual:` scheme with
 *     VISUAL_FRAME_CSP as a response header, and that header also sandboxes it
 *     (`sandbox allow-scripts`), so even a page opened on its own is fenced.
 *     A served (non-local) document does not inherit the app's policy, which
 *     is what lets its inline scripts run; `srcdoc`, `blob:` and `data:`
 *     frames inherit the app's `script-src 'self'` and run no inline script in
 *     a packaged build (measured, Electron 43).
 *  3. The app's CSP frames nothing but `iface-visual:`, and main's navigation
 *     guard lets a frame navigate only from nothing to a visual URL: a link,
 *     a script's `location` change or a refresh inside a visual goes nowhere.
 *  4. The wrapper below puts the same CSP first in the document and disarms
 *     `<base>` and `<meta http-equiv="refresh">`. This is belt and braces,
 *     never the fence: a page can always spell markup a text rewrite misses.
 * No `postMessage` listener exists for a visual: the page cannot talk back.
 */

/** The scheme main serves visual documents from. */
export const VISUAL_FRAME_SCHEME = "iface-visual";

/** What a visual document may load: inline style and script, `data:` images,
 * fonts and media; nothing from anywhere, no forms, no `<base>`. */
export const VISUAL_FRAME_CSP = [
  "default-src 'none'",
  "img-src data:",
  "style-src 'unsafe-inline'",
  "script-src 'unsafe-inline'",
  "font-src data:",
  "media-src data:",
  "base-uri 'none'",
  "form-action 'none'",
].join("; ");

/** The response header: the CSP above, and the same sandbox as the frame. */
export const VISUAL_FRAME_RESPONSE_CSP = `${VISUAL_FRAME_CSP}; sandbox allow-scripts`;

/** The `sandbox` attribute every visual frame carries, exactly. */
export const VISUAL_FRAME_SANDBOX = "allow-scripts";

/** The renderer → main channel that registers a wrapped document. */
export const VISUAL_FRAME_CHANNEL = "iface:visualFrame";

export type VisualColorScheme = "light" | "dark";

/** Register a wrapped document (answers its URL) or release one. */
export type VisualFrameRequest =
  | { kind: "register"; html: string }
  | { kind: "release"; url: string };

const TOKEN = /^[0-9a-f]{32}$/;

/** The URL main serves a registered document at. */
export function visualFrameUrl(token: string): string {
  if (!TOKEN.test(token)) throw new TypeError("A visual frame token is 32 lowercase hex.");
  return `${VISUAL_FRAME_SCHEME}://frame/${token}`;
}

/** The token of a visual frame URL, or null for any other URL. */
export function visualFrameToken(url: string): string | null {
  const prefix = `${VISUAL_FRAME_SCHEME}://frame/`;
  if (!url.startsWith(prefix)) return null;
  const token = url.slice(prefix.length);
  return TOKEN.test(token) ? token : null;
}

/** A well-formed request from the renderer, or null. */
export function parseVisualFrameRequest(value: unknown): VisualFrameRequest | null {
  if (typeof value !== "object" || value === null) return null;
  const request = value as { kind?: unknown; html?: unknown; url?: unknown };
  const keys = Object.keys(value);
  if (request.kind === "register" && typeof request.html === "string"
      && keys.length === 2 && request.html.length <= MAX_WRAPPED_CHARS) {
    return { kind: "register", html: request.html };
  }
  if (request.kind === "release" && typeof request.url === "string" && keys.length === 2
      && visualFrameToken(request.url) !== null) {
    return { kind: "release", url: request.url };
  }
  return null;
}

/** A page is at most 512 KiB of UTF-8 (Rust's bound), so at most that many
 * UTF-16 units; the wrapper adds well under 4 KiB. */
export const MAX_WRAPPED_CHARS = 512 * 1024 + 4096;

/**
 * The document a visual frame shows: the CSP first, then the app's colour
 * scheme as `data-theme` and one inline rule, then the agent's page with its
 * `<base>` and refresh tags disarmed. No app script runs inside the frame.
 *
 * `<base target="_blank">` (no `href`, so `base-uri 'none'` allows it): a
 * plain link click becomes a popup, which the frame's sandbox (no
 * `allow-popups`) drops, so the page stays. Without it the click navigates
 * the frame itself; the app's `frame-src iface-visual:` refuses that before
 * the navigation guard sees it, and Chromium puts a blank error page in the
 * frame in place of the visual.
 */
export function wrapVisualDocument(page: string, scheme: VisualColorScheme): string {
  const theme = scheme === "dark" ? "dark" : "light";
  return "<!doctype html>"
    + `<html data-theme="${theme}"><head>`
    + `<meta http-equiv="Content-Security-Policy" content="${VISUAL_FRAME_CSP}">`
    + '<meta charset="utf-8">'
    + '<base target="_blank">'
    + `<meta name="color-scheme" content="${theme}">`
    + `<style>:root{color-scheme:${theme}}</style>`
    + "</head>"
    + disarm(page);
}

/** An SVG file as a page: a minimal HTML document around it. */
export function svgVisualPage(svg: string): string {
  return "<!doctype html><html><head><style>html,body{margin:0}svg{max-width:100%;height:auto}</style></head><body>"
    + svg.replace(/^﻿?\s*<\?xml[^>]*\?>/i, "")
    + "</body></html>";
}

/** `<base …>` becomes an inert unknown element; a `<meta http-equiv=refresh>`
 * goes. Case-insensitive, whatever the quoting. */
function disarm(page: string): string {
  return page
    .replace(/<meta\b[^>]*?http-equiv\s*=\s*["']?\s*refresh[^>]*>/gi, "")
    .replace(/<(\/?)base\b/gi, "<$1x-base");
}
