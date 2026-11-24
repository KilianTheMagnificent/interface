/**
 * The renderer half of turning an authorized path into a chip. Both attachment
 * routes converge here — a drop resolves paths and a paste has main write them
 * — so the two produce identical chips and neither can quietly grow behaviour
 * the other lacks.
 *
 * These are the parts App.tsx cannot test by rendering: the backend is injected
 * so both live here as ordinary functions rather than as closures inside a
 * component nobody can mount in this test environment.
 */

import type { TerminalLaunchProfileId } from "../shared/ipc";
import { PROMPT_IMAGE_LIMITS } from "../shared/prompt-images";
import type { PromptContextItem } from "./prompt-context";
import { isPromptDropImageName } from "./prompt-drop";
import type { PastedFile } from "./prompt-paste";

/** A path the main process has authorized, on its way to becoming a chip. */
export interface GrantedAttachment {
  path: string;
  kind: "file" | "folder";
  previewUrl?: string;
}

/**
 * Fill in a thumbnail for the images among these grants BEFORE any chip is
 * drawn, so an image attachment never flickers from a generic icon into a
 * picture. A grant that already carries one is a paste — main had to decode
 * those bytes to admit them at all — and skips the round trip. A thumbnail
 * that cannot be produced leaves an ordinary file chip, which is exactly what
 * every non-image attachment already is: the attachment still works, it just
 * has no picture.
 */
export async function withAttachmentThumbnails(
  readThumbnail: (path: string) => Promise<string | null>,
  granted: readonly GrantedAttachment[],
): Promise<GrantedAttachment[]> {
  return Promise.all(granted.map(async entry => {
    if (entry.previewUrl !== undefined || entry.kind !== "file"
        || !isPromptDropImageName(entry.path)) {
      return entry;
    }
    const previewUrl = await readThumbnail(entry.path).catch(() => null);
    return previewUrl === null ? entry : { ...entry, previewUrl };
  }));
}

/**
 * The chips for a set of authorized paths, pictures resolved. This is the whole
 * of "an attachment became something the user can see", and it lives here
 * rather than inside App.tsx because App cannot be mounted in this test
 * environment — an image that silently lost its thumbnail on the way to a chip
 * would otherwise be invisible to every test.
 */
export async function attachmentContextItems(
  deps: {
    readThumbnail: (path: string) => Promise<string | null>;
    /** Fresh renderer-local id per chip. */
    mintId: (kind: string) => string;
    /** Display name for a path — normally its last segment. */
    leaf: (path: string) => string;
  },
  granted: readonly GrantedAttachment[],
): Promise<PromptContextItem[]> {
  const drawn = await withAttachmentThumbnails(deps.readThumbnail, granted);
  return drawn.map(entry => ({
    id: deps.mintId(entry.kind),
    kind: entry.kind,
    label: deps.leaf(entry.path),
    value: entry.path,
    detail: entry.path,
    ...(entry.previewUrl === undefined ? {} : { previewUrl: entry.previewUrl }),
  }));
}

/**
 * Screenshots land at the END of a draft that already has words, never at
 * the caret. An empty box stays empty: the chip is the attachment and the
 * user still has a sentence to write. A non-empty box gets a separating
 * space only when it does not already end in whitespace, so "look at this"
 * plus a shot becomes "look at this /path" rather than a splice mid-word.
 */
export function draftAfterScreenshotAttach(
  draft: string,
  paths: readonly string[],
): string {
  if (paths.length === 0) return draft;
  if (draft.trim() === "") return draft;
  const snippet = paths.join(" ");
  return /\s$/u.test(draft) ? `${draft}${snippet}` : `${draft} ${snippet}`;
}

/**
 * Saved paths coming back from the attach callback. Anything else (void, a
 * rejected request, a caller that only draws chips) means there is nothing
 * to put in the draft.
 */
export function attachedScreenshotPaths(result: unknown): readonly string[] {
  if (!Array.isArray(result)) return [];
  return result.filter((path): path is string => typeof path === "string" && path !== "");
}

/** CSI F: End / cursor to last column. Typed as stdin so the program that
 *  owns the prompt moves its own caret; xterm cannot do that for it. */
export const TERMINAL_CURSOR_TO_LINE_END = "\x1b[F";

/**
 * What to type for a clipboard image. Every agent CLI Interface launches reads
 * the clipboard image itself on Ctrl+V and draws its own placeholder — Claude
 * Code and Codex "[Image #1]", Grok and Qwen theirs — so a pane running one gets
 * the keystroke and never sees a file path. A drop is different: its File bytes
 * are in the drag payload, not necessarily on the clipboard, so Interface must
 * write those bytes and type the resulting path instead (`null`). A shell has
 * no image-paste key and takes the same path route.
 */
export function terminalImagePasteInput(
  profileId: TerminalLaunchProfileId | undefined,
  source: "clipboard" | "drop",
): string | null {
  return source === "clipboard" && profileId !== undefined && profileId !== "shell"
    ? "\x16"
    : null;
}

/**
 * Keystrokes typed before attached paths are pasted. When the current line
 * already has text, go to the end of that line first so a drop or a slow
 * screenshot write cannot splice the path into the middle of the sentence.
 */
export function terminalPathInsertPrefix(lineText: string): string {
  return lineText.trim() === "" ? "" : `${TERMINAL_CURSOR_TO_LINE_END} `;
}

/**
 * Clipboard files as bounded byte payloads. An entry that will not yield its
 * bytes, or one already past the ceiling main would refuse anyway, is dropped
 * here rather than carried across the IPC boundary to be refused on the far
 * side — that would cost the same memory twice for a picture nobody sees.
 */
export async function pastedImagePayloads(
  files: readonly PastedFile[],
): Promise<Uint8Array[]> {
  const payloads: Uint8Array[] = [];
  for (const file of files) {
    try {
      const buffer = await file.arrayBuffer();
      if (buffer.byteLength === 0
          || buffer.byteLength > PROMPT_IMAGE_LIMITS.maxSourceBytes) {
        continue;
      }
      payloads.push(new Uint8Array(buffer));
    } catch {
      // A clipboard entry that will not yield its bytes is not attachable.
    }
  }
  return payloads;
}
