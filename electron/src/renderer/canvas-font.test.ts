import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { canvasFont, guardCanvasFont, installCanvasFontGuard } from "./canvas-font";

/* Owner, 2026-11-15: "The font is all messed up" in terminal panes. Chromium
   150 (Electron 43.8.10, 0.3.177) refuses a canvas font that begins with white
   space, and xterm's WebGL renderer writes one for every upright glyph. The
   renderer lane `terminal-caret` checks the real engine; these cover the guard. */

/** A stand-in for a 2D context's prototype: refuses what Chromium 150 refuses. */
function strictCanvas(): { prototype: object; make(): { font: string } } {
  class Context {
    #font = "10px sans-serif";
    get font(): string { return this.#font; }
    set font(value: string) { if (!/^\s/u.test(value)) this.#font = value; }
  }
  return { prototype: Context.prototype, make: () => new Context() };
}

describe("a canvas font that begins with white space", () => {
  it("loses only its leading white space, and anything else is passed through", () => {
    expect(canvasFont(" 400 24px monospace")).toBe("400 24px monospace");
    expect(canvasFont("\t\n 700 24px \"Geist Mono\", monospace ")).toBe("700 24px \"Geist Mono\", monospace ");
    expect(canvasFont("italic 400 24px monospace")).toBe("italic 400 24px monospace");
    expect(canvasFont(undefined)).toBeUndefined();
    expect(canvasFont(12)).toBe(12);
  });

  it("is drawn in the font xterm asked for, not the canvas's 10px default", () => {
    const canvas = strictCanvas();
    const before = canvas.make();
    // What xterm's WebGL atlas writes for upright text: an empty style, then the rest.
    const upright = `${""} ${400} ${12 * 2}px "JetBrainsMono Nerd Font", monospace`;
    before.font = upright;
    expect(before.font).toBe("10px sans-serif");

    expect(guardCanvasFont(canvas.prototype)).toBe(true);
    const after = canvas.make();
    after.font = upright;
    expect(after.font).toBe('400 24px "JetBrainsMono Nerd Font", monospace');
    after.font = "italic 700 24px monospace";
    expect(after.font).toBe("italic 700 24px monospace");
  });

  it("is guarded once, and a prototype without a font setter is left alone", () => {
    const canvas = strictCanvas();
    expect(guardCanvasFont(canvas.prototype)).toBe(true);
    const guarded = Object.getOwnPropertyDescriptor(canvas.prototype, "font")!.set;
    expect(guardCanvasFont(canvas.prototype)).toBe(true);
    expect(Object.getOwnPropertyDescriptor(canvas.prototype, "font")!.set).toBe(guarded);
    expect(guardCanvasFont({})).toBe(false);
    expect(guardCanvasFont(undefined)).toBe(false);
    expect(guardCanvasFont(Object.freeze({ get font() { return ""; } }))).toBe(false);
  });

  it("covers on-screen and off-screen canvases, and does nothing where there are none", () => {
    const onScreen = strictCanvas();
    const offScreen = strictCanvas();
    installCanvasFontGuard({ CanvasRenderingContext2D: onScreen, OffscreenCanvasRenderingContext2D: offScreen } as
      unknown as typeof globalThis);
    for (const canvas of [onScreen, offScreen]) {
      const context = canvas.make();
      context.font = " 400 24px monospace";
      expect(context.font).toBe("400 24px monospace");
    }
    expect(() => installCanvasFontGuard({} as typeof globalThis)).not.toThrow();
  });

  it("is in place before any terminal is made", () => {
    const view = readFileSync(new URL("./components/TerminalView.tsx", import.meta.url), "utf8");
    expect(view).toContain('import { installCanvasFontGuard } from "../canvas-font";');
    // At module scope, so the app and every fixture that mounts a terminal has it.
    expect(view).toMatch(/^installCanvasFontGuard\(\);$/mu);
  });
});
