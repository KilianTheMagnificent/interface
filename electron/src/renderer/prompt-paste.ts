/**
 * Images arriving by paste. This is the one attachment route with no path at
 * all: prompt-drop.ts can resolve a dropped file to its location on disk, but a
 * pasted screenshot exists only as clipboard bytes until Interface writes it
 * somewhere. So this module DOES hand bytes onward — straight to the main
 * process, which writes the file and decides whether the bytes were an image.
 *
 * The `type` read here only decides what to OFFER. It is the clipboard's own
 * label, it is attacker-influenceable, and nothing downstream trusts it: main
 * sniffs the magic bytes and then requires a real decode before a byte lands on
 * disk. Filtering on it here just avoids shipping a copied PDF to a handler
 * that is only going to refuse it.
 */

import { PROMPT_CONTEXT_VALIDATION_LIMITS } from "../shared/ipc";
import { attachedScreenshotPaths, draftAfterScreenshotAttach } from "./prompt-attach";

export interface PastedFile {
  type: string;
  name?: string;
  arrayBuffer(): Promise<ArrayBuffer>;
}

export interface PasteSource {
  types: readonly string[];
  files?: ArrayLike<PastedFile>;
}

/**
 * The images to attach from this paste, or none.
 *
 * TEXT WINS, always. Copying a spreadsheet cell, a rich snippet, or most things
 * out of a browser puts an image on the clipboard BESIDE the text. Swallowing
 * those pastes to attach a picture of the words the user meant to paste would
 * be maddening and hard to even diagnose, so any clipboard carrying real text
 * is left entirely to the textarea. What remains — a screenshot, an image file
 * copied in Finder, an image copied from a page — has no text half.
 *
 * Bounded by the same item ceiling the drop grant uses, so a clipboard holding
 * a hundred files cannot make a hundred round trips before main refuses them.
 */
export function pastedImageFiles(source: PasteSource): PastedFile[] {
  if (source.types.includes("text/plain")) return [];
  const files = source.files;
  if (files === undefined) return [];
  const out: PastedFile[] = [];
  for (let index = 0; index < files.length; index++) {
    const file = files[index];
    if (typeof file?.type !== "string" || !file.type.startsWith("image/")) continue;
    out.push(file);
    if (out.length === PROMPT_CONTEXT_VALIDATION_LIMITS.maxItems) break;
  }
  return out;
}

/**
 * What a paste should do, for both composers. `null` means "not ours": the
 * textarea pastes it exactly as it always has, which is the answer for ordinary
 * text, for a clipboard with no images, and for a composer that cannot accept
 * attachments right now. A non-empty array means Interface is claiming this
 * paste, and the caller must preventDefault before attaching it.
 *
 * The two composers share this rather than each carrying their own copy: a
 * paste that attaches in one prompt box and silently does nothing in the other
 * is the kind of difference nobody reports and everybody notices.
 */
export function promptPasteAction(source: PasteSource | null,
    enabled: boolean): PastedFile[] | null {
  if (!enabled || source === null) return null;
  const images = pastedImageFiles(source);
  return images.length === 0 ? null : images;
}

/** A paste event as a prompt textarea receives it. */
export interface DraftPasteEvent {
  clipboardData: { types: ArrayLike<string>; files: ArrayLike<PastedFile> };
  preventDefault(): void;
}

/**
 * A paste into either prompt surface. A clipboard that is images and nothing
 * else is claimed (see promptPasteAction) and handed to `onPastedImages`;
 * anything with text on it pastes as text, untouched. The saved paths that
 * come back go at the END of a non-empty draft, not at the caret, and the
 * caret follows them there. `isCurrent` lets a surface that can move to
 * another session drop paths that arrive after it did.
 */
export function pasteImagesIntoDraft(event: DraftPasteEvent, options: {
  /** False while the surface cannot take an attachment (locked, launching). */
  enabled: boolean;
  onPastedImages: ((files: readonly PastedFile[]) =>
    void | Promise<readonly string[] | void>) | undefined;
  /** Apply `update` to the draft the paths belong in. */
  updateDraft: (update: (draft: string) => string) => void;
  textarea: { readonly current: HTMLTextAreaElement | null };
  isCurrent?: () => boolean;
}): void {
  const images = promptPasteAction(
    { types: Array.from(event.clipboardData.types), files: event.clipboardData.files },
    options.onPastedImages !== undefined && options.enabled);
  if (images === null) return;
  event.preventDefault();
  void Promise.resolve(options.onPastedImages?.(images)).then(result => {
    if (options.isCurrent?.() === false) return;
    const paths = attachedScreenshotPaths(result);
    if (paths.length === 0) return;
    options.updateDraft(current => draftAfterScreenshotAttach(current, paths));
    window.requestAnimationFrame(() => {
      const node = options.textarea.current;
      if (node === null) return;
      const end = node.value.length;
      node.setSelectionRange(end, end);
    });
  });
}
