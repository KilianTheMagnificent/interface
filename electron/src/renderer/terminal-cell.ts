/**
 * Terminal cell width, snapped the way the owner's terminal (Ghostty, inside
 * Rex) snaps it.
 *
 * xterm's WebGL renderer floors a character's advance to whole device pixels,
 * so MonoLisa at 11.5px on a Retina display (14.72 device pixels) is squeezed
 * into 14. Ghostty rounds to the nearest pixel and gets 15. xterm adds its
 * `letterSpacing` option, in device pixels, to the floored width, so the
 * difference between the two roundings is the spacing that makes the cells
 * agree: one pixel when the advance's fraction is a half or more, else none.
 */
export function cellSnapLetterSpacing(advance: number, devicePixelRatio: number): number {
  const device = advance * devicePixelRatio;
  if (!Number.isFinite(device) || device <= 0) return 0;
  return Math.round(device) - Math.floor(device);
}

let measuring: CanvasRenderingContext2D | null | undefined;
const advances = new Map<string, number>();

/** One character's advance in CSS pixels, measured as xterm measures it. */
export function terminalAdvance(fontFamily: string, fontSize: number): number {
  const font = `${fontSize}px ${fontFamily}`;
  const known = advances.get(font);
  if (known !== undefined) return known;
  measuring ??= typeof document === "undefined" ? null : document.createElement("canvas").getContext("2d");
  if (measuring === null) return 0;
  measuring.font = font;
  const advance = measuring.measureText("W").width;
  // A face that is still loading measures as its fallback: do not keep that.
  if (advance > 0 && document.fonts.check(font)) advances.set(font, advance);
  return advance;
}
