import type { SessionKey } from "../shared/engine.js";
import { threadVisualOf } from "../shared/thread-journal.js";
import {
  svgVisualPage, wrapVisualDocument, type VisualColorScheme, type VisualFrameRequest,
} from "../shared/visual-frame.js";

/**
 * Visual replies in the renderer (T3 section 8): what a card asks for and
 * the words it shows. The page reaches the screen only through
 * shared/visual-frame.ts's wrapper and main's sandboxed `iface-visual:` frame.
 */

/** A page ready to show: the frame URL main serves it at, and its source. */
export interface LoadedVisual {
  url: string;
  html: string;
}

/** What a visual card may ask for. App.tsx builds the one instance. */
export interface VisualFrameLoader {
  /** The page of `key`'s visual `id`, framed for `scheme`; null when the
   * thread no longer keeps it. Rejects when it could not be read. */
  load(key: SessionKey, id: string, scheme: VisualColorScheme): Promise<LoadedVisual | null>;
  /** The card no longer shows `url`. */
  release(url: string): void;
}

/** The two window.iface calls a loader makes, as App passes them in. */
export interface VisualFrameApi {
  threadJournal(command: { kind: "visual"; key: SessionKey; id: string }): Promise<unknown>;
  visualFrame(request: VisualFrameRequest): Promise<string | null>;
}

/** The loader over `api`: read the page from Rust, wrap it, register the
 * wrapped document with main. */
export function visualFrameLoader(api: VisualFrameApi): VisualFrameLoader {
  return {
    async load(key, id, scheme) {
      const html = threadVisualOf(await api.threadJournal({ kind: "visual", key, id }), id);
      if (html === null) return null;
      const url = await api.visualFrame({ kind: "register", html: wrapVisualDocument(html, scheme) });
      if (typeof url !== "string") throw new Error("The visual frame was not registered.");
      return { url, html };
    },
    release(url) {
      void api.visualFrame({ kind: "release", url }).catch(() => undefined);
    },
  };
}

/** A previewed `.html` or `.svg` file framed the same way (Output preview's
 * Render). Answers the URL; release it like a card's. */
export async function previewFrameUrl(
  api: Pick<VisualFrameApi, "visualFrame">, text: string, kind: "html" | "svg", scheme: VisualColorScheme,
): Promise<string> {
  const page = kind === "svg" ? svgVisualPage(text) : text;
  const url = await api.visualFrame({ kind: "register", html: wrapVisualDocument(page, scheme) });
  if (typeof url !== "string") throw new Error("The preview frame was not registered.");
  return url;
}

/** Whether a previewed file can be rendered: `.html`, `.htm` or `.svg`. */
export function renderableKind(path: string): "html" | "svg" | null {
  const lower = path.toLowerCase();
  if (lower.endsWith(".html") || lower.endsWith(".htm")) return "html";
  if (lower.endsWith(".svg")) return "svg";
  return null;
}

/** The app's colour scheme now (appearance.ts paints it on the root). */
export function appColorScheme(root: { style: { colorScheme: string } } | null): VisualColorScheme {
  return root?.style.colorScheme === "dark" ? "dark" : "light";
}

/** The card's size line: "1.8 KB". */
export function visualSizeLabel(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "";
  if (bytes < 1000) return `${bytes} B`;
  return `${(bytes / 1000).toFixed(bytes < 10_000 ? 1 : 0)} KB`;
}

/** What a card says when it has no page to show. */
export const VISUAL_MISSING_TEXT = "This visual is no longer kept.";
export const VISUAL_FAILED_TEXT = "The visual could not be shown.";
