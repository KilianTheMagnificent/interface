/**
 * Screenshots attached to a prompt. Two facts shape everything here.
 *
 * A PASTED image has no path — it is clipboard bytes — so the main process
 * writes it to a real file and the renderer then treats it as an ordinary
 * attached path. That is deliberate: an attached path is something the agent
 * can open with its own tools, so the screenshot is genuinely usable. A
 * renderer-only preview that never became a file would look right and do
 * nothing.
 *
 * A DROPPED image has a path but no bytes in the renderer (see prompt-drop.ts —
 * Interface never reads dropped bytes there). Both cases therefore get their
 * thumbnail the same way: main decodes, downscales, and returns a bounded
 * `data:` URL. The renderer never sees full image bytes and never needs a
 * `file:` image source, so the window CSP stays `img-src 'self' data:`.
 *
 * Nothing here decodes anything. These are the rules both sides must agree on,
 * kept pure so they can be tested without Electron.
 */

export const PROMPT_IMAGE_LIMITS = {
  /** Refused BEFORE decode, so a crafted header cannot make us allocate first. */
  maxSourceBytes: 16 * 1024 * 1024,
  /**
   * Longest edge of the generated thumbnail. Large enough to stay sharp on a
   * 2x display at the ~160px the chip and transcript actually draw, small
   * enough that the encoded URL is a chip-sized string rather than the whole
   * screenshot smuggled into every transcript row.
   */
  thumbnailMaxEdge: 320,
  /** Ceiling on the encoded URL itself; a thumbnail over this is dropped, not truncated. */
  maxPreviewChars: 512_000,
  /**
   * How many pictures ONE turn may draw. It is the composer's own attachment
   * limit (DEFAULT_PROMPT_CONTEXT_LIMIT), so a replayed turn can never draw
   * more images than a sent one was allowed to carry, and a session file
   * claiming a thousand images on a single message costs the renderer eight.
   * Images past it are dropped rather than drawn smaller: this bounds the
   * NUMBER of pictures, and maxPreviewChars bounds each one's size.
   */
  maxTurnImages: 8,
} as const;

/**
 * The renderer's boundary for a preview URL, applied wherever one enters a
 * prompt-context item. It must be a bounded base64 `data:` image and nothing
 * else — not a remote URL, not `javascript:`, and deliberately not SVG, which
 * is a script-bearing document format that has no business being a thumbnail.
 * The CSP already refuses a remote fetch; this refuses the string.
 */
const PREVIEW_PATTERN = /^data:image\/(?:png|jpeg|webp);base64,[A-Za-z0-9+/]+={0,2}$/;

export function isPromptImagePreviewUrl(value: unknown): value is string {
  return typeof value === "string"
    && value.length <= PROMPT_IMAGE_LIMITS.maxPreviewChars
    && PREVIEW_PATTERN.test(value);
}

/**
 * One attached image as a transcript turn draws it. Display only: what the
 * engine received for this attachment was its path, never these bytes.
 */
export interface PromptTurnImage {
  /** A validated `data:` URL — see isPromptImagePreviewUrl. */
  src: string;
  /** The attachment's own label, which becomes the image's accessible name. */
  label: string;
}

/**
 * A non-drawable image placeholder inside `user.message.text`, with provenance
 * supplied by the adapter that flattened the provider's structured turn.
 * `textLine` is zero-based in that exact text value. The optional label is a
 * basename only; compact clients can replace those exact lines without guessing
 * whether user-authored Markdown happens to resemble an attachment placeholder.
 */
export interface PromptImageReference {
  textLine: number;
  textLineCount: number;
  label?: string;
}
