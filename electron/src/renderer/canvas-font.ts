/**
 * Canvas fonts that begin with white space (owner, 2026-11-15: "The font is
 * all messed up", in terminal panes).
 *
 * Chromium 150, which 0.3.177 brought in with Electron 43.8.10, refuses a 2D
 * canvas `font` that starts with a space, tab or newline and keeps the font it
 * had: for a fresh canvas, `10px sans-serif`. xterm's WebGL renderer draws its
 * glyphs on such a canvas and writes the font as `${style} ${weight} ${size}px
 * ${family}`, with an empty style for upright text, so every upright glyph in
 * every terminal pane came out in a tiny sans-serif, spread across cells
 * measured for the real font. CSS itself still reads the same string, and
 * trailing or doubled spaces are still accepted; only the leading one is not.
 *
 * So the leading white space is taken off before the canvas sees it, for every
 * canvas in the window, which changes nothing about what the font means.
 */

/** The font as the canvas should be given it: a string loses its leading white space. */
export function canvasFont(value: unknown): unknown {
  return typeof value === "string" ? value.trimStart() : value;
}

const GUARDED = Symbol.for("interface.canvas-font-guard");

/**
 * Makes `prototype`'s `font` setter read past leading white space. Says
 * whether it is guarded now; a prototype without a `font` accessor is left
 * alone, and guarding twice does nothing more.
 */
export function guardCanvasFont(prototype: object | undefined): boolean {
  if (prototype === undefined) return false;
  if ((prototype as Record<symbol, unknown>)[GUARDED] === true) return true;
  const font = Object.getOwnPropertyDescriptor(prototype, "font");
  if (font?.set === undefined || font.configurable !== true) return false;
  const set = font.set;
  Object.defineProperty(prototype, "font", {
    ...font,
    set(this: unknown, value: unknown) { set.call(this, canvasFont(value)); },
  });
  Object.defineProperty(prototype, GUARDED, { value: true });
  return true;
}

/** Guards every 2D canvas this window can make: on-screen and off-screen. */
export function installCanvasFontGuard(scope: typeof globalThis = globalThis): void {
  const canvases = scope as { CanvasRenderingContext2D?: { prototype: object };
    OffscreenCanvasRenderingContext2D?: { prototype: object } };
  guardCanvasFont(canvases.CanvasRenderingContext2D?.prototype);
  guardCanvasFont(canvases.OffscreenCanvasRenderingContext2D?.prototype);
}
