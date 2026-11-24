/**
 * Drag-and-drop of files and images into a composer. The payload is paths
 * only: Interface never reads dropped bytes here. Finder files resolve through
 * preload's pathForFile.
 */

import type { PastedFile } from "./prompt-paste";

export interface PromptDropPath {
  path: string;
  directory?: boolean;
}

const IMAGE_EXTENSIONS = new Set([
  ".avif", ".bmp", ".gif", ".heic", ".heif", ".ico", ".jpeg", ".jpg", ".png",
  ".tif", ".tiff", ".webp",
]);

/**
 * A path the drag source is about to take back. macOS hands a drag from the
 * screenshot thumbnail, the Simulator, Mail, Photos or Safari a REAL file under
 * the per-user temp tree, then deletes it seconds after the drop — so an image
 * landing here has to be copied by its bytes, not by its path.
 */
const VOLATILE_DROP_PATH = /^\/(private\/)?(var\/folders|tmp)\//;

export function promptDropAccepts(types: readonly string[]): boolean {
  return types.includes("Files");
}

export function promptDropEffect(types: readonly string[]): "copy" | "none" {
  return promptDropAccepts(types) ? "copy" : "none";
}

export function nextDropHoverCount(
  count: number,
  type: "dragenter" | "dragleave" | "drop" | "dragend",
): number {
  if (type === "dragenter") return count + 1;
  if (type === "dragleave") return Math.max(0, count - 1);
  return 0;
}

/**
 * A drop split into files that already live on disk and images whose bytes are
 * the only lasting copy. An image dragged out of a page has no path for
 * pathForFile to find; a screenshot dragged from its own capture thumbnail has
 * one, but under the temp tree its source erases moments later. Both are dead
 * paths by the time an agent opens them, so a caller that can store bytes turns
 * them into a real path the way a paste does. A non-image at a volatile path
 * has no byte route to take and stays a path.
 */
export function splitDroppedFiles<T extends { name: string; type?: string }>(
  files: readonly T[],
  pathForFile: (file: { name: string }) => string,
): { paths: string[]; images: T[] } {
  const paths: string[] = [];
  const images: T[] = [];
  const seen = new Set<string>();
  for (const file of files) {
    const resolved = pathForFile(file).trim();
    const image = (typeof file.type === "string" && file.type.startsWith("image/"))
      || isPromptDropImageName(file.name);
    if (resolved !== "" && !(image && VOLATILE_DROP_PATH.test(resolved))) {
      if (seen.has(resolved)) continue;
      seen.add(resolved);
      paths.push(resolved);
      continue;
    }
    if (image) images.push(file);
  }
  return { paths, images };
}

export function isPromptDropImageName(name: string): boolean {
  const cut = name.lastIndexOf(".");
  if (cut < 0) return false;
  return IMAGE_EXTENSIONS.has(name.slice(cut).toLowerCase());
}

/** A drag event as a prompt surface receives it. */
export interface PromptDropEvent {
  dataTransfer: DataTransfer | null;
  preventDefault(): void;
}

/** The five drag handlers a prompt surface spreads onto its drop target. */
export interface PromptDropZone {
  onDragEnter(event: PromptDropEvent): void;
  onDragOver(event: PromptDropEvent): void;
  onDragLeave(event: PromptDropEvent): void;
  onDrop(event: PromptDropEvent): void;
  onDragEnd(): void;
}

/**
 * The drop zone both prompt surfaces share. A drag carrying files is claimed
 * only while `enabled` and while the caller can both resolve and attach paths;
 * anything else passes through untouched. `setHover` is the surface's own
 * enter/leave counter, which draws its drop indicator while above zero.
 *
 * An image with no path, or one whose path is a temp copy its source deletes
 * right after the drop, takes the paste route: its bytes are the only lasting
 * copy, and granting the dying path attaches nothing.
 */
export function promptDropZone(options: {
  enabled: boolean;
  pathForFile: ((file: { name: string }) => string) | undefined;
  onDroppedPaths: ((paths: readonly PromptDropPath[]) => void) | undefined;
  onPastedImages: ((files: readonly PastedFile[]) => unknown) | undefined;
  setHover: (next: number | ((count: number) => number)) => void;
}): PromptDropZone {
  const { pathForFile, onDroppedPaths, onPastedImages, setHover } = options;
  const enabled = options.enabled && onDroppedPaths !== undefined && pathForFile !== undefined;
  const accepts = (event: PromptDropEvent): event is PromptDropEvent & {
    dataTransfer: DataTransfer;
  } => enabled && event.dataTransfer !== null
    && promptDropAccepts(Array.from(event.dataTransfer.types));
  return {
    onDragEnter: event => {
      if (!accepts(event)) return;
      event.preventDefault();
      setHover(count => nextDropHoverCount(count, "dragenter"));
    },
    onDragOver: event => {
      if (!accepts(event)) return;
      event.preventDefault();
      event.dataTransfer.dropEffect = promptDropEffect(Array.from(event.dataTransfer.types));
    },
    onDragLeave: event => {
      if (!accepts(event)) return;
      setHover(count => nextDropHoverCount(count, "dragleave"));
    },
    onDrop: event => {
      if (!enabled || event.dataTransfer === null) return;
      const types = Array.from(event.dataTransfer.types);
      if (!promptDropAccepts(types)) return;
      event.preventDefault();
      setHover(0);
      const { paths, images } = splitDroppedFiles(
        Array.from(event.dataTransfer.files), pathForFile);
      if (images.length > 0) void onPastedImages?.(images);
      if (paths.length > 0
          || (images.length === 0 && event.dataTransfer.files.length > 0)) {
        onDroppedPaths(paths.map(path => ({ path })));
      }
    },
    onDragEnd: () => { setHover(0); },
  };
}
