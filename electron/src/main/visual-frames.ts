import { randomBytes } from "node:crypto";
import {
  VISUAL_FRAME_RESPONSE_CSP, VISUAL_FRAME_SCHEME, visualFrameToken, visualFrameUrl,
} from "../shared/visual-frame.js";

/**
 * The documents visual frames show (T3 section 8): the renderer registers a
 * wrapped page and gets back an unguessable `iface-visual://frame/<token>`
 * URL; main serves it with the visual CSP and sandbox as response headers.
 * Memory only, bounded: the oldest registration goes first. Rules and the
 * fence: shared/visual-frame.ts.
 */
export class VisualFrames {
  private readonly documents = new Map<string, string>();
  private bytes = 0;

  constructor(
    private readonly maxDocuments = 48,
    private readonly maxBytes = 48 * 1024 * 1024,
    private readonly token: () => string = () => randomBytes(16).toString("hex"),
  ) {}

  register(html: string): string {
    const token = this.token();
    this.documents.set(token, html);
    this.bytes += html.length * 2;
    while (this.documents.size > this.maxDocuments || this.bytes > this.maxBytes) {
      const oldest = this.documents.keys().next();
      if (oldest.done === true || oldest.value === token) break;
      this.forget(oldest.value);
    }
    return visualFrameUrl(token);
  }

  release(url: string): void {
    const token = visualFrameToken(url);
    if (token !== null) this.forget(token);
  }

  /** What main answers for a request to `url`: the document, or 404. */
  respond(url: string, method = "GET"): Response {
    const token = visualFrameToken(url);
    const html = token === null ? undefined : this.documents.get(token);
    if (method !== "GET" || html === undefined) {
      return new Response(null, { status: 404, headers: { "cache-control": "no-store" } });
    }
    return new Response(html, {
      status: 200,
      headers: {
        "content-type": "text/html; charset=utf-8",
        "content-security-policy": VISUAL_FRAME_RESPONSE_CSP,
        "x-content-type-options": "nosniff",
        "referrer-policy": "no-referrer",
        "cache-control": "no-store",
      },
    });
  }

  get size(): number { return this.documents.size; }

  private forget(token: string): void {
    const html = this.documents.get(token);
    if (html === undefined) return;
    this.documents.delete(token);
    this.bytes -= html.length * 2;
  }
}

/** The protocol surface main installs on the app window's session. */
export interface VisualFrameProtocol {
  handle(scheme: string, handler: (request: Request) => Response | Promise<Response>): void;
}

/** Serves `frames` on `iface-visual:` in `protocol` (the app's own session). */
export function installVisualFrameProtocol(protocol: VisualFrameProtocol, frames: VisualFrames): void {
  protocol.handle(VISUAL_FRAME_SCHEME, request => frames.respond(request.url, request.method));
}

/**
 * Whether the app window may let a frame navigate to `url`: only a subframe
 * (never the window itself), only to a visual URL, and only a frame that is
 * not showing a visual already (a new frame starts empty or `about:blank`).
 * A page inside a visual can therefore never move its frame anywhere, not
 * even to another visual.
 */
export function isVisualFrameNavigation(details: {
  url: string; isMainFrame: boolean; frame?: { url: string } | null;
}): boolean {
  if (details.isMainFrame || visualFrameToken(details.url) === null) return false;
  const current = details.frame?.url ?? "";
  return current === "" || current === "about:blank";
}

/** A navigation event as the window guard reads it. */
export interface GuardedNavigation {
  url: string;
  isMainFrame?: boolean;
  frame?: { url: string } | null;
  preventDefault(): void;
}

/**
 * The app window's navigation guard: a new frame may load a visual
 * ([`isVisualFrameNavigation`]); anything else must be `allowed` (the
 * renderer's own document) or is prevented. A frame already gone reads as
 * not a visual load.
 */
export function rendererNavigationGuard(allowed: (url: string) => boolean): (event: GuardedNavigation) => void {
  return event => {
    let visual = false;
    try {
      visual = isVisualFrameNavigation({
        url: event.url, isMainFrame: event.isMainFrame ?? true, frame: event.frame ?? null,
      });
    } catch {
      visual = false;
    }
    if (visual) return;
    if (!allowed(event.url)) event.preventDefault();
  };
}
